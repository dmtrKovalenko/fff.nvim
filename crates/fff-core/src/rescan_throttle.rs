use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use crate::constants::{
    LARGE_INDEX_FILE_COUNT, RESCAN_MIN_INTERVAL, RESCAN_MIN_INTERVAL_LARGE_INDEX,
};

const NEVER: u64 = u64::MAX;

// Drops watcher rescan requests inside the cooldown after the last scan.
// A slightly stale index is fine: the next admitted event rescans everything.
pub(crate) struct RescanThrottle {
    epoch: Instant,
    last_admitted: AtomicU64,
}

impl Default for RescanThrottle {
    fn default() -> Self {
        Self {
            epoch: Instant::now(),
            last_admitted: AtomicU64::new(NEVER),
        }
    }
}

impl RescanThrottle {
    /// Returns `true` if a rescan may start now and records it as the last scan
    pub(crate) fn admit(&self, live_files: usize, has_git_repo: bool) -> bool {
        let min_interval = if !has_git_repo && live_files >= LARGE_INDEX_FILE_COUNT {
            RESCAN_MIN_INTERVAL_LARGE_INDEX
        } else {
            RESCAN_MIN_INTERVAL
        };

        let min_ms = min_interval.as_millis() as u64;
        let now = self.elapsed_ms();

        loop {
            let last = self.last_admitted.load(Ordering::Acquire);
            if last != NEVER && now.saturating_sub(last) < min_ms {
                return false;
            }
            // CAS so two concurrent requests cannot both start a walk.
            if self
                .last_admitted
                .compare_exchange(last, now, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return true;
            }
        }
    }

    /// Records an explicit (unthrottled) scan so watcher requests right after
    /// it are dropped: the index is already fresh.
    pub(crate) fn note_explicit_scan(&self) {
        self.last_admitted
            .store(self.elapsed_ms(), Ordering::Release);
    }

    fn elapsed_ms(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn throttle_at(ms_ago: u64) -> RescanThrottle {
        let now = Instant::now();
        RescanThrottle {
            epoch: now
                .checked_sub(Duration::from_millis(ms_ago))
                .expect("monotonic clock older than the rewind"),
            last_admitted: AtomicU64::new(0),
        }
    }

    #[test]
    fn first_request_is_always_admitted() {
        let throttle = RescanThrottle::default();
        assert!(throttle.admit(100, true));
    }

    #[test]
    fn requests_inside_the_cooldown_are_dropped() {
        let throttle = throttle_at(1_000);
        assert!(!throttle.admit(100, false));
        assert!(!throttle.admit(100, false));
    }

    #[test]
    fn a_large_index_outside_a_git_repo_uses_the_slower_cadence() {
        // A minute is past the normal cooldown but not the large-index one.
        let throttle = throttle_at(60_000);
        assert!(throttle.admit(100, false));

        let throttle = throttle_at(60_000);
        assert!(!throttle.admit(LARGE_INDEX_FILE_COUNT, false));
    }

    #[test]
    fn a_git_repo_keeps_the_normal_cadence_at_any_size() {
        let throttle = throttle_at(60_000);
        assert!(throttle.admit(LARGE_INDEX_FILE_COUNT, true));
    }

    #[test]
    fn cooldown_expiry_admits_again() {
        let throttle = throttle_at(RESCAN_MIN_INTERVAL.as_millis() as u64 + 1);
        assert!(throttle.admit(100, false));
        // Admission rearms the cooldown.
        assert!(!throttle.admit(100, false));
    }

    #[test]
    fn explicit_scan_rearms_the_cooldown() {
        let throttle = RescanThrottle::default();
        throttle.note_explicit_scan();
        assert!(!throttle.admit(100, false));
    }
}
