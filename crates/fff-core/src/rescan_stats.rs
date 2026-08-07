#[cfg(rescan_stats)]
use std::sync::atomic::{AtomicUsize, Ordering};

/// Whether rescan accounting is compiled in.
pub const RESCAN_STATS_ENABLED: bool = cfg!(rescan_stats);

/// Cause recorded for a filesystem rescan request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RescanReason {
    /// Requested through the public API (refresh, directory change).
    Explicit,
    /// The kernel dropped events and asked us to re-read the subtree.
    KernelEventLoss,
    /// A `.gitignore`/`.ignore` changed, so the cached ignore rules are stale.
    IgnoreFileChanged,
    /// A single debounce batch touched more paths than we apply incrementally.
    EventBatchOverflow,
    /// The picker refused an incremental insert/update.
    IndexUpdateRejected,
    /// The post-scan overflow region ran out of slots.
    OverflowCapacity,
}

impl RescanReason {
    pub const ALL: [RescanReason; 6] = [
        RescanReason::Explicit,
        RescanReason::KernelEventLoss,
        RescanReason::IgnoreFileChanged,
        RescanReason::EventBatchOverflow,
        RescanReason::IndexUpdateRejected,
        RescanReason::OverflowCapacity,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            RescanReason::Explicit => "explicit",
            RescanReason::KernelEventLoss => "kernel_event_loss",
            RescanReason::IgnoreFileChanged => "ignore_file_changed",
            RescanReason::EventBatchOverflow => "event_batch_overflow",
            RescanReason::IndexUpdateRejected => "index_update_rejected",
            RescanReason::OverflowCapacity => "overflow_capacity",
        }
    }

    const fn slot(self) -> usize {
        match self {
            RescanReason::Explicit => 0,
            RescanReason::KernelEventLoss => 1,
            RescanReason::IgnoreFileChanged => 2,
            RescanReason::EventBatchOverflow => 3,
            RescanReason::IndexUpdateRejected => 4,
            RescanReason::OverflowCapacity => 5,
        }
    }
}

impl std::fmt::Display for RescanReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Snapshot of rescan requests grouped by reason.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RescanStats {
    pub total: usize,
    /// Requests suppressed during the cooldown.
    pub throttled: usize,
    counts: [usize; RescanReason::ALL.len()],
    throttled_counts: [usize; RescanReason::ALL.len()],
}

impl RescanStats {
    pub fn count(&self, reason: RescanReason) -> usize {
        self.counts[reason.slot()]
    }

    pub fn count_throttled(&self, reason: RescanReason) -> usize {
        self.throttled_counts[reason.slot()]
    }

    /// Admitted requests originating from watcher fallbacks.
    pub fn watcher_triggered(&self) -> usize {
        self.total - self.count(RescanReason::Explicit)
    }

    /// Per-reason delta against an earlier snapshot.
    pub fn since(&self, earlier: &RescanStats) -> RescanStats {
        let mut counts = [0usize; RescanReason::ALL.len()];
        let mut throttled_counts = [0usize; RescanReason::ALL.len()];
        for slot in 0..RescanReason::ALL.len() {
            counts[slot] = self.counts[slot].saturating_sub(earlier.counts[slot]);
            throttled_counts[slot] =
                self.throttled_counts[slot].saturating_sub(earlier.throttled_counts[slot]);
        }

        RescanStats {
            total: self.total.saturating_sub(earlier.total),
            throttled: self.throttled.saturating_sub(earlier.throttled),
            counts,
            throttled_counts,
        }
    }
}

impl std::fmt::Display for RescanStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} rescan(s)", self.total)?;
        let mut first = true;
        for reason in RescanReason::ALL {
            let count = self.count(reason);
            if count == 0 {
                continue;
            }
            f.write_str(if first { " [" } else { ", " })?;
            write!(f, "{reason}={count}")?;
            first = false;
        }
        if !first {
            f.write_str("]")?;
        }
        if self.throttled > 0 {
            write!(f, ", {} throttled", self.throttled)?;
        }
        Ok(())
    }
}

#[cfg(rescan_stats)]
#[derive(Default)]
pub(crate) struct RescanCounters {
    counters: [AtomicUsize; RescanReason::ALL.len()],
    throttled: [AtomicUsize; RescanReason::ALL.len()],
}

#[cfg(rescan_stats)]
impl RescanCounters {
    pub(crate) fn record(&self, reason: RescanReason) {
        self.counters[reason.slot()].fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_throttled(&self, reason: RescanReason) {
        self.throttled[reason.slot()].fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn snapshot(&self) -> RescanStats {
        let mut stats = RescanStats::default();
        for reason in RescanReason::ALL {
            let count = self.counters[reason.slot()].load(Ordering::Relaxed);
            stats.counts[reason.slot()] = count;
            stats.total += count;

            let throttled = self.throttled[reason.slot()].load(Ordering::Relaxed);
            stats.throttled_counts[reason.slot()] = throttled;
            stats.throttled += throttled;
        }
        stats
    }

    pub(crate) fn reset(&self) {
        for counter in self.counters.iter().chain(self.throttled.iter()) {
            counter.store(0, Ordering::Relaxed);
        }
    }
}

// Release builds retain the API without counter storage.
#[cfg(not(rescan_stats))]
#[derive(Default)]
pub(crate) struct RescanCounters;

#[cfg(not(rescan_stats))]
impl RescanCounters {
    pub(crate) fn record(&self, _reason: RescanReason) {}

    pub(crate) fn record_throttled(&self, _reason: RescanReason) {}

    pub(crate) fn snapshot(&self) -> RescanStats {
        RescanStats::default()
    }

    pub(crate) fn reset(&self) {}
}

#[cfg(all(test, rescan_stats))]
mod tests {
    use super::*;

    #[test]
    fn counters_attribute_and_diff_per_reason() {
        let counters = RescanCounters::default();
        counters.record(RescanReason::Explicit);
        let baseline = counters.snapshot();

        counters.record(RescanReason::IgnoreFileChanged);
        counters.record(RescanReason::IgnoreFileChanged);
        counters.record(RescanReason::OverflowCapacity);

        let stats = counters.snapshot();
        assert_eq!(stats.total, 4);
        assert_eq!(stats.watcher_triggered(), 3);

        let delta = stats.since(&baseline);
        assert_eq!(delta.total, 3);
        assert_eq!(delta.count(RescanReason::Explicit), 0);
        assert_eq!(delta.count(RescanReason::IgnoreFileChanged), 2);
        assert_eq!(
            delta.to_string(),
            "3 rescan(s) [ignore_file_changed=2, overflow_capacity=1]"
        );

        counters.reset();
        assert_eq!(counters.snapshot(), RescanStats::default());
    }
}
