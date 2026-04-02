use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Duration;

use crate::error::Error;
use crate::file_picker::FilePicker;
use crate::frecency::FrecencyTracker;
use crate::git::GitStatusCache;
use crate::query_tracker::QueryTracker;

/// Thread-safe shared handle to the [`FilePicker`] instance.
///
/// Wraps `Arc<RwLock<Option<FilePicker>>>` with convenience methods.
/// `Clone` gives a new handle to the same picker (Arc clone).
/// `Default` creates an empty handle suitable for `Lazy::new(SharedPicker::default)`.
#[derive(Clone, Default)]
pub struct SharedPicker(pub(crate) Arc<RwLock<Option<FilePicker>>>);

impl std::fmt::Debug for SharedPicker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SharedPicker").field(&"..").finish()
    }
}

impl SharedPicker {
    pub fn read(&self) -> Result<RwLockReadGuard<'_, Option<FilePicker>>, Error> {
        self.0.read().map_err(|_| Error::AcquireItemLock)
    }

    pub fn write(&self) -> Result<RwLockWriteGuard<'_, Option<FilePicker>>, Error> {
        self.0.write().map_err(|_| Error::AcquireItemLock)
    }

    /// Block until the background filesystem scan finishes.
    /// Returns `true` if scan completed, `false` on timeout.
    pub fn wait_for_scan(&self, timeout: Duration) -> bool {
        let signal = {
            let guard = self.0.read().expect("shared picker lock poisoned");
            match guard.as_ref() {
                Some(picker) => picker.scan_signal(),
                None => return true,
            }
        };

        let start = std::time::Instant::now();
        while signal.load(std::sync::atomic::Ordering::Acquire) {
            if start.elapsed() >= timeout {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        true
    }

    /// Block until the background file watcher is ready.
    /// Returns `true` if watcher ready, `false` on timeout.
    pub fn wait_for_watcher(&self, timeout: Duration) -> bool {
        let signal = {
            let guard = self.0.read().expect("shared picker lock poisoned");
            match guard.as_ref() {
                Some(picker) => picker.watcher_signal(),
                None => return true,
            }
        };

        let start = std::time::Instant::now();
        while !signal.load(std::sync::atomic::Ordering::Acquire) {
            if start.elapsed() >= timeout {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        true
    }

    /// Refresh git statuses for all indexed files.
    pub fn refresh_git_status(&self, shared_frecency: &SharedFrecency) -> Result<usize, Error> {
        use git2::StatusOptions;
        use tracing::debug;

        let git_status = {
            let guard = self.read()?;
            let Some(ref picker) = *guard else {
                return Err(Error::FilePickerMissing);
            };

            debug!(
                "Refreshing git statuses for picker: {:?}",
                picker.git_root()
            );

            GitStatusCache::read_git_status(
                picker.git_root(),
                StatusOptions::new()
                    .include_untracked(true)
                    .recurse_untracked_dirs(true)
                    .include_unmodified(true)
                    .exclude_submodules(true),
            )
        };

        let mut guard = self.write()?;
        let picker = guard.as_mut().ok_or(Error::FilePickerMissing)?;

        let statuses_count = if let Some(git_status) = git_status {
            let count = git_status.statuses_len();
            picker.update_git_statuses(git_status, shared_frecency)?;
            count
        } else {
            0
        };

        Ok(statuses_count)
    }
}


/// Thread-safe shared handle to the [`FrecencyTracker`] instance.
#[derive(Clone, Default)]
pub struct SharedFrecency(pub(crate) Arc<RwLock<Option<FrecencyTracker>>>);

impl std::fmt::Debug for SharedFrecency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SharedFrecency").field(&"..").finish()
    }
}

impl SharedFrecency {
    pub fn read(&self) -> Result<RwLockReadGuard<'_, Option<FrecencyTracker>>, Error> {
        self.0.read().map_err(|_| Error::AcquireFrecencyLock)
    }

    pub fn write(&self) -> Result<RwLockWriteGuard<'_, Option<FrecencyTracker>>, Error> {
        self.0.write().map_err(|_| Error::AcquireFrecencyLock)
    }

    /// Initialize the frecency tracker, replacing any existing one.
    pub fn init(&self, tracker: FrecencyTracker) -> Result<(), Error> {
        let mut guard = self.write()?;
        *guard = Some(tracker);
        Ok(())
    }

    /// Spawn a background GC thread for this frecency tracker.
    pub fn spawn_gc(
        &self,
        db_path: String,
        use_unsafe_no_lock: bool,
    ) -> crate::Result<std::thread::JoinHandle<()>> {
        FrecencyTracker::spawn_gc(self.clone(), db_path, use_unsafe_no_lock)
    }
}


/// Thread-safe shared handle to the [`QueryTracker`] instance.
#[derive(Clone, Default)]
pub struct SharedQueryTracker(pub(crate) Arc<RwLock<Option<QueryTracker>>>);

impl std::fmt::Debug for SharedQueryTracker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SharedQueryTracker").field(&"..").finish()
    }
}

impl SharedQueryTracker {
    pub fn read(&self) -> Result<RwLockReadGuard<'_, Option<QueryTracker>>, Error> {
        self.0.read().map_err(|_| Error::AcquireFrecencyLock)
    }

    pub fn write(&self) -> Result<RwLockWriteGuard<'_, Option<QueryTracker>>, Error> {
        self.0.write().map_err(|_| Error::AcquireFrecencyLock)
    }

    /// Initialize the query tracker, replacing any existing one.
    pub fn init(&self, tracker: QueryTracker) -> Result<(), Error> {
        let mut guard = self.write()?;
        *guard = Some(tracker);
        Ok(())
    }
}
