use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use rayon::prelude::*;
use tracing::{error, info};

use crate::FileSync;
use crate::background_watcher::BackgroundWatcher;
use crate::bigram_filter::{build_bigram_index, sniff_binary_for_non_indexable};
use crate::error::Error;
use crate::file_picker::FFFMode;
use crate::git::GitStatusCache;
use crate::parallelism::BACKGROUND_THREAD_POOL;
use crate::shared::{SharedFilePicker, SharedFrecency};
use crate::simd_path::ArenaPtr;
use crate::types::ContentCacheBudget;

#[derive(Clone, Default)]
pub(crate) struct ScanSignals {
    /// Set to `true` while any scan phase is running
    pub(crate) scanning: Arc<AtomicBool>,
    /// Set to `true` once the filesystem watcher has been installed
    pub(crate) watcher_ready: Arc<AtomicBool>,
    /// Indicates that that owning picker was requested to shut down
    pub(crate) cancelled: Arc<AtomicBool>,
    /// Used to resolve conflicts if multiple rescans were triggered in a queue
    pub(crate) rescan_pending: Arc<AtomicBool>,
    /// Set by `post_scan_snapshot`, cleared by `PostScanSnapshot::drop`.
    /// DO NOT set or clear this manually — it is managed exclusively by the
    /// PostScanSnapshot lifecycle.
    pub(crate) post_scan_indexing_active: Arc<AtomicBool>,
}

/// Which optional phases a scan should run.
#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct ScanConfig {
    pub(crate) warmup: bool,
    pub(crate) content_indexing: bool,
    pub(crate) watch: bool,
    pub(crate) auto_cache_budget: bool,
    pub(crate) install_watcher: bool,
    pub(crate) follow_symlinks: bool,
    pub(crate) enable_fs_root_scanning: bool,
    pub(crate) enable_home_dir_scanning: bool,
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
    trace_span: tracing::Span,
}

impl ScanJob {
    pub fn new_rescan(
        shared_picker: &SharedFilePicker,
        shared_frecency: &SharedFrecency,
    ) -> Result<Option<Self>, Error> {
        let guard = shared_picker.read()?;
        let picker = guard.as_ref().ok_or(Error::FilePickerMissing)?;

        if picker.is_scan_active()
            || picker
                .signals
                .post_scan_indexing_active
                .load(Ordering::Acquire)
        {
            return Ok(None);
        }

        let mode = picker.mode();
        let signals = picker.scan_signals();
        let scanned_files_counter = picker.scanned_files_counter();
        let base_path = picker.base_path().to_path_buf();
        let trace_span = picker.trace_span();

        let new_scan_config = ScanConfig {
            warmup: picker.has_mmap_cache(),
            content_indexing: picker.has_content_indexing(),
            watch: picker.has_watcher(),
            auto_cache_budget: !picker.has_explicit_cache_budget(),
            install_watcher: false, // the watcher is independent of rescan, it is not restarting EVER
            follow_symlinks: picker.follows_symlinks(),
            enable_fs_root_scanning: picker.fs_root_scanning_enabled(),
            enable_home_dir_scanning: picker.home_dir_scanning_enabled(),
        };

        drop(guard); // just a sanity check

        Ok(Some(Self {
            mode,
            signals,
            base_path,
            scanned_files_counter,
            config: new_scan_config,
            shared_picker: shared_picker.clone(),
            shared_frecency: shared_frecency.clone(),
            trace_span,
        }))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_initial(
        shared_picker: SharedFilePicker,
        shared_frecency: SharedFrecency,
        base_path: PathBuf,
        mode: FFFMode,
        signals: ScanSignals,
        scanned_files_counter: Arc<AtomicUsize>,
        trace_span: tracing::Span,
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
            trace_span,
        }
    }

    /// Spawn the job on a dedicated OS thread. Returns immediately.
    pub fn spawn(self) -> std::thread::JoinHandle<()> {
        self.signals.scanning.store(true, Ordering::Release);
        let span = self.trace_span.clone();
        std::thread::Builder::new()
            .name("fff-scan".into())
            .spawn(move || {
                let _g = span.enter();
                self.run();
            })
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
            trace_span: _,
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
            git_workdir.clone(),
            &scanned_files_counter,
            &shared_frecency,
            mode,
            config.follow_symlinks,
        ) {
            Ok(sync) => sync,
            Err(e) => {
                error!(?e, "scan walk failed");
                return;
            }
        };

        // 2. Brief write to install the freshly-walked file list.
        if let Ok(mut guard) = shared_picker.write()
            && let Some(picker) = guard.as_mut()
        {
            if signals.cancelled.load(Ordering::Acquire) {
                info!("scan cancelled between walk and commit, discarding");
                return;
            }

            let live_count = sync.live_count;
            picker.commit_new_sync(sync);

            if config.auto_cache_budget && !picker.has_explicit_cache_budget() {
                picker.set_cache_budget(ContentCacheBudget::new_for_repo(live_count));
            }
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
            rescubscribe_watcher_post_scan(&shared_picker);
        }

        let mut snapshot = if !signals.cancelled.load(Ordering::Acquire) {
            shared_picker.read().ok().and_then(|guard| {
                guard
                    .as_ref()
                    .and_then(|picker| unsafe { picker.post_scan_snapshot() })
            })
        } else {
            None
        };

        // 3. Post-scan warmup + bigram build — runs in parallel with the
        // git-status thread to overlap the two expensive phases.
        // Always runs (even with both flags off) so binary-content files
        // with unknown extensions get reclassified before user search hits.
        if !signals.cancelled.load(Ordering::Acquire)
            && let Some(snap) = snapshot.as_ref()
        {
            Self::run_post_scan(&shared_picker, &signals, &config, snap);
        }

        // 4. Join and git status, this HAS to be done after the post scan
        if !signals.cancelled.load(Ordering::Acquire)
            && let Some(status_handle) = status_handle
            && let Some(snapshot) = snapshot.as_mut()
            // THIS DOES WAIT for potentially very long status query
            && let Ok(Some(git_status)) = status_handle.join()
        {
            apply_git_status_and_frecency(git_status, &shared_frecency, mode, snapshot);
        }

        drop(snapshot); // SNAPSHOT SHOULD NOT BE USED AFTER THIS POINT

        // 5. Install filesystem watcher (initial scan only).
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
                config.enable_fs_root_scanning,
                config.enable_home_dir_scanning,
                tracing::Span::current(),
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

        // 6. Drain any rescan that arrived while we were busy.
        // if user initiated a new rescan we had no way to cancel current post scan, so do it again
        if !signals.cancelled.load(Ordering::Acquire)
            && signals.rescan_pending.swap(false, Ordering::AcqRel)
        {
            match Self::new_rescan(&shared_picker, &shared_frecency) {
                Ok(Some(follow_up)) => {
                    info!("Rescheduling deferred rescan after current scan finished");
                    follow_up.spawn();
                }
                Ok(None) => {
                    // this should be practically impossible because we do not have any
                    // queue, but if somehow a new rescan was triggered JUST IN THIS MOMENT
                    // just ignore it because the ongoing one is fresh enough
                    tracing::warn!("Post scan was re-triggered, ignoring");
                }
                Err(e) => {
                    error!(?e, "Failed to reschedule deferred rescan");
                }
            }
        }
    }

    /// THIS IS VERY VERY IMPORTANT THAT ANYTHING INSIDE THIS FUNCTION TO NOT READ ANYTHING CLEARABLE OUTSIDE
    /// this is a very silly off lock implementation that actually matters, and that's why it is crafted
    /// to never read anything from the picker, it can only WRITE information using single instructions
    ///
    /// Things that are safe and immutable - file list, indexes of files, paths, and signals.
    #[tracing::instrument(skip_all, fields(warmup = ?config.warmup, indexing = ?config.content_indexing))]
    fn run_post_scan(
        shared_picker: &SharedFilePicker,
        signals: &ScanSignals,
        config: &ScanConfig,
        unsafe_snapshot: &crate::file_picker::PostScanUnsafeSnapshot,
    ) {
        let Some(arena) = unsafe_snapshot
            .arena // we are never touching overlays so this arena is always correct
            .as_ref()
            .map(|s| s.as_arena_ptr())
        else {
            tracing::error!("Failed to run post scan: arena is invalid");
            return;
        };

        let files: &[crate::types::FileItem] = &unsafe_snapshot.files[..unsafe_snapshot.base_count];
        if signals.cancelled.load(Ordering::Acquire) {
            return;
        }

        if config.content_indexing {
            let indexable_count = unsafe_snapshot.indexable_count.min(files.len());
            let (indexable_files, non_indexable_files) = files.split_at(indexable_count);
            let index = build_bigram_index(indexable_files, &unsafe_snapshot.base_path, arena);

            if let Ok(mut guard) = shared_picker.write()
                && let Some(picker) = guard.as_mut()
            {
                picker.set_bigram_index(index);
            }

            // Bigram only sniffs files <= MAX_INDEXABLE_FILE_SIZE; large
            // unknown-extension binaries slip past it and would otherwise be
            // grep-able as text. Cheap header sniff catches those.
            if !signals.cancelled.load(Ordering::Acquire) {
                sniff_binary_for_non_indexable(
                    non_indexable_files,
                    &unsafe_snapshot.base_path,
                    arena,
                );
            }
        } else {
            // this potentially a long running as we are not parallelizing it but it's okay
            sniff_binary_for_non_indexable(files, &unsafe_snapshot.base_path, arena);
        }

        // TODO Skipped as potentially unsafe - figure this out later
        // if config.warmup && !signals.cancelled.load(Ordering::Acquire) {
        //     warmup_mmaps(files, budget, &unsafe_snapshot.base_path, arena);
        // }
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

/// If the scan encounters new directories created we have to add them to the watch list
/// this is fine because the watcher does deduplicate the entries and doesn't add a lot of
/// garbage notify watchers / fs events streams
#[tracing::instrument(skip_all)]
fn rescubscribe_watcher_post_scan(shared_picker: &SharedFilePicker) {
    let Ok(guard) = shared_picker.read() else {
        return;
    };
    let Some(picker) = guard.as_ref() else {
        return;
    };
    let Some(watcher) = picker.background_watcher.as_ref() else {
        return;
    };

    picker.for_each_dir(|dir: &std::path::Path| {
        watcher.request_watch_dir(dir.to_path_buf());
        std::ops::ControlFlow::Continue(())
    });
}

#[tracing::instrument(
    level = "debug",
    skip_all,
    fields(file_count = tracing::field::Empty, dirty_count = tracing::field::Empty),
)]
fn apply_git_status_and_frecency(
    git_cache: GitStatusCache,
    shared_frecency: &SharedFrecency,
    mode: FFFMode,
    unsafe_snapshot: &mut crate::file_picker::PostScanUnsafeSnapshot,
) {
    let frecency = shared_frecency.read().ok();
    let frecency_ref = frecency.as_ref().and_then(|f| f.as_ref());

    let base_count = unsafe_snapshot.base_count;
    let files: &mut [crate::types::FileItem] = &mut unsafe_snapshot.files[..base_count];
    // Dir frecency goes through per-entry `AtomicI32`; a shared slice is
    // enough and avoids any `&mut` aliasing against the Arc-shared buffer.
    let dirs: &[crate::types::DirItem] = &unsafe_snapshot.dirs;
    let arena = unsafe_snapshot
        .arena
        .as_ref()
        .map(|s| s.as_arena_ptr())
        .unwrap_or(ArenaPtr::null());

    // Reset dir frecency before recomputation.
    for dir in dirs.iter() {
        dir.reset_frecency();
    }

    BACKGROUND_THREAD_POOL.install(|| {
        files.par_iter_mut().for_each(|file| {
            if unsafe_snapshot.cancelled.load(Ordering::Relaxed) {
                return;
            }

            let mut buf = [0u8; crate::simd_path::PATH_BUF_SIZE];
            let absolute_path =
                file.write_absolute_path(arena, &unsafe_snapshot.base_path, &mut buf);

            file.git_status = git_cache.lookup_status(absolute_path);
            if let Some(frecency) = frecency_ref {
                let _ =
                    file.update_frecency_scores(frecency, arena, &unsafe_snapshot.base_path, mode);
            }

            let score = file.access_frecency_score as i32;
            if score > 0 {
                let dir_idx = file.parent_dir_index as usize;
                if let Some(dir) = dirs.get(dir_idx) {
                    dir.update_frecency_if_larger(score);
                }
            }
        });
    });

    let span = tracing::Span::current();
    span.record("dirty_count", git_cache.statuses_len());
}
