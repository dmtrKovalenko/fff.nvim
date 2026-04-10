use std::path::PathBuf;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Duration;

use crate::error::Error;
use crate::file_picker::FilePicker;
use crate::frecency::FrecencyTracker;
use crate::git::GitStatusCache;
use crate::query_tracker::QueryTracker;

/// Thread-safe shared handle to the [`FilePicker`] instance.
///
/// Uses `parking_lot::RwLock` which is reader-fair — new readers are not
/// blocked when a writer is waiting, preventing search query stalls during
/// background bigram builds or watcher writes.
///
/// `Clone` gives a new handle to the same picker (Arc clone).
/// `Default` creates an empty handle suitable for `Lazy::new(SharedPicker::default)`.
#[derive(Clone, Default)]
pub struct SharedPicker(pub(crate) Arc<parking_lot::RwLock<Option<FilePicker>>>);

impl std::fmt::Debug for SharedPicker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SharedPicker").field(&"..").finish()
    }
}

impl SharedPicker {
    pub fn read(&self) -> Result<parking_lot::RwLockReadGuard<'_, Option<FilePicker>>, Error> {
        Ok(self.0.read())
    }

    pub fn write(&self) -> Result<parking_lot::RwLockWriteGuard<'_, Option<FilePicker>>, Error> {
        Ok(self.0.write())
    }

    /// Block until the background filesystem scan finishes.
    /// Returns `true` if scan completed, `false` on timeout.
    pub fn wait_for_scan(&self, timeout: Duration) -> bool {
        let signal = {
            let guard = self.0.read();
            match &*guard {
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
            let guard = self.0.read();
            match &*guard {
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
#[derive(Clone)]
pub struct SharedFrecency {
    inner: Arc<RwLock<Option<FrecencyTracker>>>,
    enabled: bool,
}

impl Default for SharedFrecency {
    fn default() -> Self {
        Self {
            inner: Arc::new(RwLock::new(None)),
            enabled: true,
        }
    }
}

impl std::fmt::Debug for SharedFrecency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SharedFrecency").field(&"..").finish()
    }
}

impl SharedFrecency {
    /// Creates a disabled instance that silently ignores all writes.
    pub fn noop() -> Self {
        Self {
            inner: Arc::new(RwLock::new(None)),
            enabled: false,
        }
    }

    pub fn read(&self) -> Result<RwLockReadGuard<'_, Option<FrecencyTracker>>, Error> {
        self.inner.read().map_err(|_| Error::AcquireFrecencyLock)
    }

    pub fn write(&self) -> Result<RwLockWriteGuard<'_, Option<FrecencyTracker>>, Error> {
        self.inner.write().map_err(|_| Error::AcquireFrecencyLock)
    }

    /// Initialize the frecency tracker. No-op if this is a disabled instance.
    pub fn init(&self, tracker: FrecencyTracker) -> Result<(), Error> {
        if !self.enabled {
            return Ok(());
        }
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

    /// Drop the in-memory tracker and delete the on-disk database directory.
    ///
    /// Acquires the write lock, ensuring all readers (including any active mmap
    /// access) are finished before the LMDB environment is closed and the files
    /// are removed.
    ///
    /// Returns `Ok(Some(path))` with the deleted path, or `Ok(None)` if no
    /// tracker was initialized.
    pub fn destroy(&self) -> Result<Option<PathBuf>, Error> {
        let mut guard = self.write()?;
        let Some(tracker) = guard.take() else {
            return Ok(None);
        };
        let db_path = tracker.db_path().to_path_buf();
        // Drop closes the LMDB env and unmaps the files
        drop(tracker);
        drop(guard);
        std::fs::remove_dir_all(&db_path).map_err(|source| Error::RemoveDbDir {
            path: db_path.clone(),
            source,
        })?;
        Ok(Some(db_path))
    }
}

/// Thread-safe shared handle to the [`QueryTracker`] instance.
#[derive(Clone)]
pub struct SharedQueryTracker {
    inner: Arc<RwLock<Option<QueryTracker>>>,
    enabled: bool,
}

impl Default for SharedQueryTracker {
    fn default() -> Self {
        Self {
            inner: Arc::new(RwLock::new(None)),
            enabled: true,
        }
    }
}

impl std::fmt::Debug for SharedQueryTracker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SharedQueryTracker").field(&"..").finish()
    }
}

impl SharedQueryTracker {
    /// Creates a disabled instance that silently ignores all writes.
    pub fn noop() -> Self {
        Self {
            inner: Arc::new(RwLock::new(None)),
            enabled: false,
        }
    }

    pub fn read(&self) -> Result<RwLockReadGuard<'_, Option<QueryTracker>>, Error> {
        self.inner.read().map_err(|_| Error::AcquireFrecencyLock)
    }

    pub fn write(&self) -> Result<RwLockWriteGuard<'_, Option<QueryTracker>>, Error> {
        self.inner.write().map_err(|_| Error::AcquireFrecencyLock)
    }

    /// Initialize the query tracker. No-op if this is a disabled instance.
    pub fn init(&self, tracker: QueryTracker) -> Result<(), Error> {
        if !self.enabled {
            return Ok(());
        }
        let mut guard = self.write()?;
        *guard = Some(tracker);
        Ok(())
    }

    /// Drop the in-memory tracker and delete the on-disk database directory.
    ///
    /// Acquires the write lock, ensuring all readers (including any active mmap
    /// access) are finished before the LMDB environment is closed and the files
    /// are removed.
    ///
    /// Returns `Ok(Some(path))` with the deleted path, or `Ok(None)` if no
    /// tracker was initialized.
    pub fn destroy(&self) -> Result<Option<PathBuf>, Error> {
        let mut guard = self.write()?;
        let Some(tracker) = guard.take() else {
            return Ok(None);
        };
        let db_path = tracker.db_path().to_path_buf();
        drop(tracker);
        drop(guard);
        std::fs::remove_dir_all(&db_path).map_err(|source| Error::RemoveDbDir {
            path: db_path.clone(),
            source,
        })?;
        Ok(Some(db_path))
    }
}
