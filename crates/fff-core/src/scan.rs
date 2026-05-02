//! Unified scan-phase orchestrator.
//!
//! Every (re)index code path — initial scan, FFI-triggered rescan,
//! watcher overflow rescan — goes through [`ScanJob::run`]. The
//! orchestrator owns the *sequence* of a scan:
//!
//!   1. walk filesystem off-lock
//!   2. swap `sync_data` under a brief write
//!   3. apply git status + frecency off-lock
//!   4. (optional, initial scan only) spawn the filesystem watcher
//!   5. (optional) post-scan: auto-size cache budget, warmup, bigram
//!
//! The picker write lock is held only in step 2 and step 5's index
//! install — both O(µs-ms), never seconds. Every other FFI caller on
//! the nvim main thread keeps running.
//!
//! ## Entry points
//!
//! - [`ScanJob::spawn`] — fire-and-forget from `SharedPicker` state.
//!   Used by the watcher overflow path and by FFI (`scan_files`).
//! - [`ScanJob::spawn_initial`] — same, but takes explicit config for
//!   the very first scan, before the `FilePicker` struct lives inside
//!   the shared handle.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use tracing::{error, info};

use crate::FileSync;
use crate::background_watcher::BackgroundWatcher;
use crate::bigram_filter::BigramOverlay;
use crate::bigram_filter::build_bigram_index;
use crate::error::Error;
use crate::file_picker::{self, FFFMode, warmup_mmaps};
use crate::shared::{SharedFilePicker, SharedFrecency};
use crate::types::ContentCacheBudget;

/// Shared atomic flags surfaced by the picker for the scan worker to
/// signal its progress. Grouped so every callsite passes one value,
/// not four.
#[derive(Clone, Default)]
pub(crate) struct ScanSignals {
    /// Set to `true` while any scan phase is running
    pub(crate) scanning: Arc<AtomicBool>,
    /// Set to `true` once the filesystem watcher has been installed
    pub(crate) watcher_ready: Arc<AtomicBool>,
    /// Indicates that that owning picker was requested to shut down
    pub(crate) cancelled: Arc<AtomicBool>,
    /// Soft lock indicating that the post scan non blocking work is active
    pub(crate) post_scan_busy: Arc<AtomicBool>,
    /// Used to resolve conflicts if multiple rescans were triggered in a queue
    pub(crate) rescan_pending: Arc<AtomicBool>,
}

/// Which optional phases a scan should run.
#[derive(Clone, Copy, Default)]
pub(crate) struct ScanConfig {
    pub(crate) warmup: bool,
    pub(crate) content_indexing: bool,
    pub(crate) watch: bool,
    pub(crate) auto_cache_budget: bool,
    pub(crate) install_watcher: bool,
}

/// A fully-configured scan job ready to run on a background thread.
///
/// Build with [`ScanJob::from_picker`] (reads all state from the
/// current `FilePicker`) or [`ScanJob::initial`] (for the bootstrap
/// scan, before the picker is published to `SharedPicker`).
pub(crate) struct ScanJob {
    shared_picker: SharedFilePicker,
    shared_frecency: SharedFrecency,
    base_path: PathBuf,
    mode: FFFMode,
    signals: ScanSignals,
    config: ScanConfig,
    /// Walker-maintained counter backing `get_scan_progress` on the UI
    /// side. Reset to 0 at scan start, incremented per-file by the
    /// walker. Shared `Arc` so the UI polls the same atomic.
    scanned_files_counter: Arc<AtomicUsize>,
}

impl ScanJob {
    pub fn new(
        shared_picker: &SharedFilePicker,
        shared_frecency: &SharedFrecency,
        install_watcher: bool,
    ) -> Result<Option<Self>, Error> {
        let guard = shared_picker.read()?;
        let picker = guard.as_ref().ok_or(Error::FilePickerMissing)?;

        if picker.is_scan_active() {
            return Ok(None);
        }

        let signals = picker.scan_signals();
        if signals.post_scan_busy.load(Ordering::Acquire) {
            return Ok(None);
        }

        Ok(Some(Self {
            shared_picker: shared_picker.clone(),
            shared_frecency: shared_frecency.clone(),
            base_path: picker.base_path().to_path_buf(),
            mode: picker.mode(),
            signals,
            scanned_files_counter: picker.scanned_files_counter(),
            config: ScanConfig {
                warmup: picker.has_mmap_cache(),
                content_indexing: picker.has_content_indexing(),
                watch: picker.has_watcher(),
                auto_cache_budget: !picker.has_explicit_cache_budget(),
                install_watcher,
            },
        }))
    }

    /// Same as [`new`] but without reading from the picker — caller
    /// supplies the base path / mode / flags directly. Used by the
    /// bootstrap scan before the `FilePicker` is published to
    /// `SharedPicker`.
    pub fn new_initial(
        shared_picker: SharedFilePicker,
        shared_frecency: SharedFrecency,
        base_path: PathBuf,
        mode: FFFMode,
        signals: ScanSignals,
        scanned_files_counter: Arc<AtomicUsize>,
        config: ScanConfig,
    ) -> Self {
        Self {
            shared_picker,
            shared_frecency,
            base_path,
            mode,
            signals,
            scanned_files_counter,
            config,
        }
    }

    /// Spawn the job on a dedicated OS thread. Returns immediately.
    pub fn spawn(self) -> std::thread::JoinHandle<()> {
        self.signals.scanning.store(true, Ordering::Release);
        std::thread::Builder::new()
            .name("fff-scan".into())
            .spawn(move || self.run())
            .expect("failed to spawn fff-scan thread")
    }

    fn run(self) {
        let Self {
            shared_picker,
            shared_frecency,
            base_path,
            mode,
            signals,
            scanned_files_counter,
            config,
        } = self;

        let _scanning = ScanningGuard::new(&signals, config.install_watcher);

        // Reset the UI-visible counter; the walker bumps it per file
        // and `get_scan_progress` reads it without locks.
        scanned_files_counter.store(0, Ordering::Relaxed);

        // 1. Start git discovery and walk filesystem off-lock.
        let git_workdir = FileSync::discover_git_workdir(&base_path);
        let status_handle = git_workdir.clone().map(FileSync::spawn_git_status);
        let sync = match FileSync::walk_filesystem(
            &base_path,
            git_workdir,
            &scanned_files_counter,
            &shared_frecency,
            mode,
        ) {
            Ok(sync) => sync,
            Err(e) => {
                error!(?e, "scan walk failed");
                return;
            }
        };

        if signals.cancelled.load(Ordering::Acquire) {
            info!("walk completed but picker was replaced, discarding results");
            return;
        }

        let git_workdir = sync.git_workdir.clone();

        // 2. Brief write to install the freshly-walked file list.
        if let Ok(mut guard) = shared_picker.write()
            && let Some(picker) = guard.as_mut()
        {
            picker.commit_new_sync(sync);
        } else {
            error!("failed to install scan results into picker");
            return;
        }

        // Files are now searchable — flip the scan signal *early* so
        // UI progress polls see the picker as "ready" while we run the
        // optional post-scan steps in the background.
        signals.scanning.store(false, Ordering::Relaxed);

        // in case we do a rescan, we have to resubscribe a watcher to the new set of directories
        // all the already watched directories are not going to be resubscribed
        if !config.install_watcher && !signals.cancelled.load(Ordering::Acquire) {
            resubscribe_to_new_picker(&shared_picker);
        }

        // 3. Apply git status + frecency off-lock.
        if !signals.cancelled.load(Ordering::Acquire)
            && let Some(status_handle) = status_handle
        {
            file_picker::apply_git_status_and_frecency(
                &shared_picker,
                &shared_frecency,
                status_handle,
                mode,
            );
        }

        // 4. Install filesystem watcher (initial scan only).
        if config.install_watcher && config.watch && !signals.cancelled.load(Ordering::Acquire) {
            let shared_picker: &SharedFilePicker = &shared_picker;
            let shared_frecency: &SharedFrecency = &shared_frecency;
            let base_path: &std::path::Path = &base_path;

            match BackgroundWatcher::new(
                base_path.to_path_buf(),
                git_workdir,
                shared_picker.clone(),
                shared_frecency.clone(),
                mode,
            ) {
                Ok(watcher) => {
                    if let Ok(mut guard) = shared_picker.write()
                        && let Some(picker) = guard.as_mut()
                    {
                        picker.background_watcher = Some(watcher);
                    }
                }
                Err(e) => error!(?e, "failed to initialize background watcher"),
            };
        }

        // 5. Post-scan warmup + bigram build.
        if (config.warmup || config.content_indexing) && !signals.cancelled.load(Ordering::Acquire)
        {
            run_post_scan(&shared_picker, &base_path, &signals, &config);
        }

        // 6. Drain any rescan that arrived while we were busy.
        //
        // `trigger_full_rescan_async` sets `rescan_pending` whenever a
        // caller asks for a rescan while `ScanJob::new` would have
        // returned `Ok(None)` (scan active *or* post-scan busy). We
        // consume the flag with `swap` so concurrent requests that land
        // between the check and the follow-up spawn are still captured
        // by the next invocation.
        if !signals.cancelled.load(Ordering::Acquire)
            && signals.rescan_pending.swap(false, Ordering::AcqRel)
        {
            match Self::new(&shared_picker, &shared_frecency, false) {
                Ok(Some(follow_up)) => {
                    info!("Rescheduling deferred rescan after current scan finished");
                    follow_up.spawn();
                }
                Ok(None) => {
                    // Another scan slipped in between our post-scan exit
                    // and the `new()` call above. That scan will drain
                    // the flag we just cleared — but we re-arm it so it
                    // does.
                    signals.rescan_pending.store(true, Ordering::Release);
                }
                Err(e) => {
                    error!(?e, "Failed to reschedule deferred rescan");
                }
            }
        }
    }
}

/// RAII helper that flips the `scanning` signal on construction and
/// resets it on drop (so early-returns can't leave it stuck on `true`).
/// Also drives the `watcher_ready` signal on the initial-scan path.
struct ScanningGuard<'a> {
    signals: &'a ScanSignals,
    release_watcher_ready_on_drop: bool,
}

impl<'a> ScanningGuard<'a> {
    fn new(signals: &'a ScanSignals, release_watcher_ready_on_drop: bool) -> Self {
        signals.scanning.store(true, Ordering::Relaxed);
        Self {
            signals,
            release_watcher_ready_on_drop,
        }
    }
}

impl Drop for ScanningGuard<'_> {
    fn drop(&mut self) {
        self.signals.scanning.store(false, Ordering::Relaxed);
        if self.release_watcher_ready_on_drop {
            self.signals.watcher_ready.store(true, Ordering::Release);
        }
    }
}

fn run_post_scan(
    shared_picker: &SharedFilePicker,
    base_path: &std::path::Path,
    signals: &ScanSignals,
    config: &ScanConfig,
) {
    let phase_start = std::time::Instant::now();

    // Auto-scale the cache budget before we take the files snapshot —
    // warmup needs the final budget.
    if config.auto_cache_budget
        && !signals.cancelled.load(Ordering::Acquire)
        && let Ok(mut guard) = shared_picker.write()
        && let Some(picker) = guard.as_mut()
        && !picker.has_explicit_cache_budget()
    {
        let (files, _, _) = picker.sync_data_snapshot();
        picker.set_cache_budget(ContentCacheBudget::new_for_repo(files.len()));
    }

    let Some((files, indexable_count, budget, arena, _busy_guard)) = shared_picker
        .read()
        .ok()
        .and_then(|guard| guard.as_ref().map(|p| snapshot_sync_data(p, signals)))
    else {
        return;
    };

    if config.warmup && !signals.cancelled.load(Ordering::Acquire) {
        let t = std::time::Instant::now();
        warmup_mmaps(files, &budget, base_path, arena);
        info!(
            "Warmup completed in {:.2}s (cached {} files, {} bytes)",
            t.elapsed().as_secs_f64(),
            budget.cached_count.load(Ordering::Relaxed),
            budget.cached_bytes.load(Ordering::Relaxed),
        );
    }

    if config.content_indexing && !signals.cancelled.load(Ordering::Acquire) {
        let indexable_files = &files[..indexable_count.min(files.len())];
        let (index, content_binary) =
            build_bigram_index(indexable_files, &budget, base_path, arena);

        if let Ok(mut guard) = shared_picker.write()
            && let Some(picker) = guard.as_mut()
        {
            for &idx in &content_binary {
                if let Some(file) = picker.get_file_mut(idx) {
                    file.set_binary(true);
                }
            }
            picker.set_bigram_index(index, BigramOverlay::new(indexable_count));
        }
    }

    info!(
        "Post-scan phase total: {:.2}s (warmup={}, content_indexing={})",
        phase_start.elapsed().as_secs_f64(),
        config.warmup,
        config.content_indexing,
    );
}

struct PostScanBusyGuard<'a>(&'a AtomicBool);
impl Drop for PostScanBusyGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// Re-registers all the directories at the watcher
#[tracing::instrument(skip_all)]
fn resubscribe_to_new_picker(shared_picker: &SharedFilePicker) {
    let Ok(guard) = shared_picker.read() else {
        return;
    };
    let Some(picker) = guard.as_ref() else {
        return;
    };
    let Some(watcher) = picker.background_watcher.as_ref() else {
        return;
    };

    // Base path first — this is the watch that delivers `Create(Folder)`
    // events for brand-new top-level subdirs. On rescan paths this
    // watch is still alive (the BackgroundWatcher survives rescans), so
    // the call is idempotent. Including it explicitly protects against
    // any future refactor that could drop the initial base-path watch.
    watcher.request_watch_dir(picker.base_path().to_path_buf());

    picker.for_each_dir(|dir: &std::path::Path| {
        watcher.request_watch_dir(dir.to_path_buf());
        std::ops::ControlFlow::Continue(())
    });
}

/// Take a `'static`-lifetime snapshot of `sync_data` pinned by a
/// post-scan busy guard. Concurrent rescans short-circuit while the
/// returned guard is alive, so the raw slice can't be freed from under
/// the warmup + bigram build that consumes it.
fn snapshot_sync_data<'a>(
    picker: &crate::file_picker::FilePicker,
    signals: &'a ScanSignals,
) -> (
    &'static [crate::types::FileItem],
    usize,
    Arc<ContentCacheBudget>,
    crate::simd_path::ArenaPtr,
    PostScanBusyGuard<'a>,
) {
    signals.post_scan_busy.store(true, Ordering::Release);
    let busy = PostScanBusyGuard(&signals.post_scan_busy);

    let (files, indexable_count, arena) = picker.sync_data_snapshot();
    let ptr = files.as_ptr();
    let len = files.len();
    let static_files: &'static [crate::types::FileItem] =
        unsafe { std::slice::from_raw_parts(ptr, len) };
    (
        static_files,
        indexable_count,
        picker.cache_budget_arc(),
        arena,
        busy,
    )
}
