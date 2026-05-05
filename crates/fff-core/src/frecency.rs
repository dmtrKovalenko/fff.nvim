use crate::db_healthcheck::DbHealthChecker;
use crate::error::{Error, Result};
use crate::file_picker::FFFMode;
use crate::git::is_modified_status;
use crate::lmdb::{LmdbStore, is_map_full};
use crate::shared::SharedFrecency;
use heed::types::{Bytes, SerdeBincode};
use heed::{Database, Env};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use std::{collections::VecDeque, path::Path};

const DECAY_CONSTANT: f64 = 0.0693; // ln(2)/10 for 10-day half-life
const SECONDS_PER_DAY: f64 = 86400.0;
const MAX_HISTORY_DAYS: f64 = 30.0; // Only consider accesses within 30 days
const MAX_TIMESTAMPS_PER_FILE: usize = 128;

// AI mode: faster decay since AI sessions are shorter and more intense
const AI_DECAY_CONSTANT: f64 = 0.231; // ln(2)/3 for 3-day half-life
const AI_MAX_HISTORY_DAYS: f64 = 7.0; // Only consider accesses within 7 days

#[derive(Debug)]
pub struct FrecencyTracker {
    env: Env,
    db: Database<Bytes, SerdeBincode<VecDeque<u64>>>,
}

const MODIFICATION_THRESHOLDS: [(i64, u64); 5] = [
    (16, 60 * 2),          // 2 minutes
    (8, 60 * 15),          // 15 minutes
    (4, 60 * 60),          // 1 hour
    (2, 60 * 60 * 24),     // 1 day
    (1, 60 * 60 * 24 * 7), // 1 week
];

// AI mode: compressed thresholds since AI edits happen in rapid bursts
const AI_MODIFICATION_THRESHOLDS: [(i64, u64); 5] = [
    (16, 30),         // 30 seconds
    (8, 60 * 5),      // 5 minutes
    (4, 60 * 15),     // 15 minutes
    (2, 60 * 60),     // 1 hour
    (1, 60 * 60 * 4), // 4 hours
];

impl DbHealthChecker for FrecencyTracker {
    fn get_env(&self) -> &heed::Env {
        &self.env
    }

    fn count_entries(&self) -> Result<Vec<(&'static str, u64)>> {
        let rtxn = self.env.read_txn().map_err(Error::DbStartReadTxn)?;
        let count = self.db.len(&rtxn).map_err(Error::DbRead)?;

        Ok(vec![("absolute_frecency_entries", count)])
    }
}

impl LmdbStore for FrecencyTracker {
    const MAX_DBS: u32 = 0;
    // 10 MiB hard ceiling. Owner's db after years of use is ~560 KiB, so this
    // leaves ~18× headroom while capping runaway growth (see GH issue #437).
    const MAP_SIZE: usize = 10 * 1024 * 1024;
    // Nuke the db when it exceeds 8 MiB on disk — leaves a small margin under
    // MAP_SIZE so we don't hit MDB_MAP_FULL before the open-time erase fires.
    const SIZE_CAP_BYTES: u64 = 8 * 1024 * 1024;
}

impl FrecencyTracker {
    /// Returns the on-disk path of the LMDB environment directory.
    pub fn db_path(&self) -> &Path {
        self.env.path()
    }

    pub fn open(db_path: impl AsRef<Path>) -> Result<Self> {
        let db_path = db_path.as_ref();
        let env = Self::open_env(db_path)?;

        // Try read-only open first — avoids blocking on the LMDB write lock
        // when another process (Neovim, another fff-mcp) already has it.
        // Only fall back to create_database (which needs a write txn) if the
        // database doesn't exist yet.
        let rtxn = env.read_txn().map_err(Error::DbStartReadTxn)?;
        let maybe_db: Option<Database<Bytes, SerdeBincode<VecDeque<u64>>>> =
            env.open_database(&rtxn, None).map_err(Error::DbOpen)?;

        drop(rtxn);

        let db = match maybe_db {
            Some(db) => db,
            None => {
                // First time: create the database (requires write lock).
                let mut wtxn = env.write_txn().map_err(Error::DbStartWriteTxn)?;
                let db = env
                    .create_database(&mut wtxn, None)
                    .map_err(Error::DbCreate)?;
                wtxn.commit().map_err(Error::DbCommit)?;
                db
            }
        };

        Ok(FrecencyTracker { db, env })
    }

    #[deprecated(
        since = "0.7.0",
        note = "LMDB unsafe no-lock mode is no longer supported; use `FrecencyTracker::open` instead. \
                The `_use_unsafe_no_lock` argument is ignored."
    )]
    pub fn new(db_path: impl AsRef<Path>, _use_unsafe_no_lock: bool) -> Result<Self> {
        Self::open(db_path)
    }

    /// Spawns a background thread to purge stale frecency entries and compact the database.
    /// Run it once in a while to purge old pages and keep DB file size reasonable.
    ///
    /// It's okay to not join this thread since it acquires locks for the db access
    ///
    /// ```
    /// use fff_search::frecency::FrecencyTracker;
    /// use fff_search::SharedFrecency;
    /// let shared_frecency: SharedFrecency = Default::default();
    /// let _ = FrecencyTracker::spawn_gc(shared_frecency, "/path/to/frecency_db".into()).ok();
    /// ```
    pub fn spawn_gc(
        shared: SharedFrecency,
        db_path: String,
    ) -> Result<std::thread::JoinHandle<()>> {
        Ok(std::thread::Builder::new()
            .name("fff-frecency-gc".into())
            .spawn(move || Self::run_frecency_gc(shared, db_path))?)
    }

    #[tracing::instrument(skip(shared), fields(db_path = %db_path))]
    fn run_frecency_gc(shared: SharedFrecency, db_path: String) {
        let start = std::time::Instant::now();

        // Purge stale entries under a read lock on the Rust `Option`. LMDB
        // serialises the internal write txn itself, so cross-process contention
        // is handled below the heed layer. In-place compaction is intentionally
        // skipped: it would require deleting `data.mdb`/`lock.mdb` while other
        // processes may still hold the env, risking a split-brain on the same
        // inode. Oversize reclamation happens at open time via
        // `LmdbStore::erase_if_oversized` instead.
        let (deleted, pruned) = {
            let guard = match shared.read() {
                Ok(g) => g,
                Err(e) => {
                    tracing::debug!("Failed to acquire read lock: {e}");
                    return;
                }
            };
            let Some(ref tracker) = *guard else {
                return;
            };
            match tracker.purge_stale_entries() {
                Ok(result) => result,
                Err(e) => {
                    tracing::debug!("Purge failed: {e}");
                    return;
                }
            }
        };

        if deleted > 0 || pruned > 0 {
            tracing::info!(deleted, pruned, elapsed = ?start.elapsed(), "Frecency GC purged entries");
        }

        let data_path = PathBuf::from(&db_path).join("data.mdb");
        let file_size = fs::metadata(&data_path).map(|m| m.len()).unwrap_or(0);
        if file_size > <Self as LmdbStore>::SIZE_CAP_BYTES {
            tracing::warn!(
                size = file_size,
                cap = <Self as LmdbStore>::SIZE_CAP_BYTES,
                "Frecency DB exceeds size cap — will be erased on next open"
            );
        }
    }

    /// Removes entries where all timestamps are older than MAX_HISTORY_DAYS,
    /// and prunes stale timestamps from entries that still have recent ones.
    /// Returns (deleted_count, pruned_count).
    fn purge_stale_entries(&self) -> Result<(usize, usize)> {
        let now = self.get_now();
        let cutoff_time = now.saturating_sub((MAX_HISTORY_DAYS * SECONDS_PER_DAY) as u64);

        // Collect entries to delete or update
        let rtxn = self.env.read_txn().map_err(Error::DbStartReadTxn)?;
        let mut to_delete: Vec<Vec<u8>> = Vec::new();
        let mut to_update: Vec<(Vec<u8>, VecDeque<u64>)> = Vec::new();

        let iter = self.db.iter(&rtxn).map_err(Error::DbRead)?;
        for result in iter {
            let (key, accesses) = result.map_err(Error::DbRead)?;

            // Timestamps are chronologically ordered (oldest at front).
            // Find the first timestamp that is still within the retention window.
            let fresh_start = accesses.iter().position(|&ts| ts >= cutoff_time);
            match fresh_start {
                None => {
                    // All timestamps are stale — delete the entire entry
                    to_delete.push(key.to_vec());
                }
                Some(0) => {
                    // All timestamps are fresh — nothing to do
                }
                Some(start) => {
                    // Some timestamps are stale — keep only the fresh ones
                    let pruned: VecDeque<u64> = accesses.iter().skip(start).copied().collect();
                    to_update.push((key.to_vec(), pruned));
                }
            }
        }
        drop(rtxn);

        if to_delete.is_empty() && to_update.is_empty() {
            return Ok((0, 0));
        }

        // Apply all changes in a single write transaction
        let mut wtxn = self.env.write_txn().map_err(Error::DbStartWriteTxn)?;
        for key in &to_delete {
            self.db.delete(&mut wtxn, key).map_err(Error::DbWrite)?;
        }
        for (key, accesses) in &to_update {
            self.db
                .put(&mut wtxn, key, accesses)
                .map_err(Error::DbWrite)?;
        }
        wtxn.commit().map_err(Error::DbCommit)?;

        Ok((to_delete.len(), to_update.len()))
    }

    fn get_accesses(&self, path: &Path) -> Result<Option<VecDeque<u64>>> {
        let rtxn = self.env.read_txn().map_err(Error::DbStartReadTxn)?;

        let key_hash = Self::path_to_hash_bytes(path)?;
        self.db.get(&rtxn, &key_hash).map_err(Error::DbRead)
    }

    fn get_now(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    fn path_to_hash_bytes(path: &Path) -> Result<[u8; 32]> {
        let Some(key) = path.to_str() else {
            return Err(Error::InvalidPath(path.to_path_buf()));
        };

        Ok(*blake3::hash(key.as_bytes()).as_bytes())
    }

    /// Returns seconds since the most recent tracked access, or `None` if the
    /// file has never been tracked.
    pub fn seconds_since_last_access(&self, path: &Path) -> Result<Option<u64>> {
        let accesses = self.get_accesses(path)?;
        let last = accesses.and_then(|a| a.back().copied());
        Ok(last.map(|ts| self.get_now().saturating_sub(ts)))
    }

    pub fn track_access(&self, path: &Path) -> Result<()> {
        let mut wtxn = self.env.write_txn().map_err(Error::DbStartWriteTxn)?;

        let key_hash = Self::path_to_hash_bytes(path)?;
        let mut accesses = self.get_accesses(path)?.unwrap_or_default();

        let now = self.get_now();
        let cutoff_time = now.saturating_sub((MAX_HISTORY_DAYS * SECONDS_PER_DAY) as u64);

        // Drop stale timestamps from the front while also enforcing the
        // per-file cap. Reserves one slot for the `push_back` below.
        while let Some(&front_time) = accesses.front() {
            if front_time < cutoff_time || accesses.len() >= MAX_TIMESTAMPS_PER_FILE {
                accesses.pop_front();
            } else {
                break;
            }
        }

        accesses.push_back(now);
        tracing::debug!(?path, accesses = accesses.len(), "Tracking access");

        if let Err(e) = self.db.put(&mut wtxn, &key_hash, &accesses) {
            if is_map_full(&e) {
                tracing::error!(
                    ?path,
                    "Frecency DB hit MDB_MAP_FULL; dropping write — db will be \
                     erased on next open via LmdbStore::erase_if_oversized"
                );
                return Ok(());
            }
            return Err(Error::DbWrite(e));
        }

        if let Err(e) = wtxn.commit() {
            if is_map_full(&e) {
                tracing::error!(
                    ?path,
                    "Frecency DB hit MDB_MAP_FULL on commit; dropping write"
                );
                return Ok(());
            }
            return Err(Error::DbCommit(e));
        }

        Ok(())
    }

    pub fn get_access_score(&self, file_path: &Path, mode: FFFMode) -> i64 {
        let accesses = self
            .get_accesses(file_path)
            .ok()
            .flatten()
            .unwrap_or_default();

        if accesses.is_empty() {
            return 0;
        }

        let decay_constant = if mode.is_ai() {
            AI_DECAY_CONSTANT
        } else {
            DECAY_CONSTANT
        };
        let max_history_days = if mode.is_ai() {
            AI_MAX_HISTORY_DAYS
        } else {
            MAX_HISTORY_DAYS
        };

        let now = self.get_now();
        let mut total_frecency = 0.0;

        let cutoff_time = now.saturating_sub((max_history_days * SECONDS_PER_DAY) as u64);

        for &access_time in accesses.iter().rev() {
            if access_time < cutoff_time {
                break; // All remaining entries are older, stop processing
            }

            let days_ago = (now.saturating_sub(access_time) as f64) / SECONDS_PER_DAY;
            let decay_factor = (-decay_constant * days_ago).exp();
            total_frecency += decay_factor;
        }

        let normalized_frecency = if total_frecency <= 10.0 {
            total_frecency
        } else {
            10.0 + (total_frecency - 10.0).sqrt() // Diminishing: >10 accesses grow slowly
        };

        normalized_frecency.round() as i64
    }

    /// Calculating modification score but only if the file is modified in the current git dir
    pub fn get_modification_score(
        &self,
        modified_time: u64,
        git_status: Option<git2::Status>,
        mode: FFFMode,
    ) -> i64 {
        let is_modified_git_status = git_status.is_some_and(is_modified_status);
        if !is_modified_git_status {
            return 0;
        }

        let thresholds = if mode.is_ai() {
            &AI_MODIFICATION_THRESHOLDS
        } else {
            &MODIFICATION_THRESHOLDS
        };

        let now = self.get_now();
        let duration_since = now.saturating_sub(modified_time);

        for i in 0..thresholds.len() {
            let (current_points, current_threshold) = thresholds[i];

            if duration_since <= current_threshold {
                if i == 0 || duration_since == current_threshold {
                    return current_points;
                }

                let (prev_points, prev_threshold) = thresholds[i - 1];

                let time_range = current_threshold - prev_threshold;
                let time_offset = duration_since - prev_threshold;
                let points_diff = prev_points - current_points;

                let interpolated_score =
                    prev_points - (points_diff * time_offset as i64) / time_range as i64;

                return interpolated_score;
            }
        }

        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_picker::FFFMode;

    fn calculate_test_frecency_score(access_timestamps: &[u64], current_time: u64) -> i64 {
        let mut total_frecency = 0.0;

        for &access_time in access_timestamps {
            let days_ago = (current_time.saturating_sub(access_time) as f64) / SECONDS_PER_DAY;
            let decay_factor = (-DECAY_CONSTANT * days_ago).exp();
            total_frecency += decay_factor;
        }

        let normalized_frecency = if total_frecency <= 20.0 {
            total_frecency
        } else {
            20.0 + (total_frecency - 10.0).sqrt()
        };

        normalized_frecency.round() as i64
    }

    #[test]
    fn test_frecency_calculation() {
        let current_time = 1000000000; // Base timestamp

        let score = calculate_test_frecency_score(&[], current_time);
        assert_eq!(score, 0);

        let accesses = [current_time]; // Accessed right now
        let score = calculate_test_frecency_score(&accesses, current_time);
        assert_eq!(score, 1); // 1.0 decay factor = 1

        let ten_days_seconds = 10 * 86400; // 10 days in seconds
        let accesses = [current_time - ten_days_seconds];
        let score = calculate_test_frecency_score(&accesses, current_time);
        assert_eq!(score, 1); // ~0.5 decay factor rounds to 1

        let accesses = [
            current_time,          // Today
            current_time - 86400,  // 1 day ago
            current_time - 172800, // 2 days ago
        ];
        let score = calculate_test_frecency_score(&accesses, current_time);
        assert!(score > 2 && score < 4, "Score: {}", score); // About 3 accesses with decay

        let thirty_days = 30 * 86400;
        let accesses = [current_time - thirty_days]; // 30 days ago
        let score = calculate_test_frecency_score(&accesses, current_time);
        assert!(
            score < 2,
            "Old access should have minimal score, got: {}",
            score
        );

        let recent_frequent = [current_time, current_time - 86400, current_time - 172800];
        let old_single = [current_time - ten_days_seconds];

        let recent_score = calculate_test_frecency_score(&recent_frequent, current_time);
        let old_score = calculate_test_frecency_score(&old_single, current_time);

        assert!(
            recent_score > old_score,
            "Recent frequent access ({}) should score higher than old single access ({})",
            recent_score,
            old_score
        );
    }

    #[test]
    fn test_modification_score_interpolation() {
        let temp_dir = std::env::temp_dir().join("fff_test_interpolation");
        let _ = std::fs::remove_dir_all(&temp_dir);
        let tracker = FrecencyTracker::open(temp_dir.to_str().unwrap()).unwrap();

        let current_time = tracker.get_now();
        let git_status = Some(git2::Status::WT_MODIFIED);

        // At 5 minutes: should interpolate between 16 and 8 points
        let five_minutes_ago = current_time - (5 * 60);
        let score = tracker.get_modification_score(five_minutes_ago, git_status, FFFMode::Neovim);

        // Expected: 16 - (8 * 3 / 13) = 16 - 1 = 15 points
        // (time_offset = 5-2 = 3, time_range = 15-2 = 13, points_diff = 16-8 = 8)
        assert_eq!(score, 15, "5 minutes should interpolate to 15 points");

        let two_minutes_ago = current_time - (2 * 60);
        let score = tracker.get_modification_score(two_minutes_ago, git_status, FFFMode::Neovim);
        assert_eq!(score, 16, "2 minutes should be exactly 16 points");

        let fifteen_minutes_ago = current_time - (15 * 60);
        let score =
            tracker.get_modification_score(fifteen_minutes_ago, git_status, FFFMode::Neovim);
        assert_eq!(score, 8, "15 minutes should be exactly 8 points");

        // At 12 hours: should interpolate between 4 and 2 points
        let twelve_hours_ago = current_time - (12 * 60 * 60);
        let score = tracker.get_modification_score(twelve_hours_ago, git_status, FFFMode::Neovim);
        // Expected: 4 - (2 * 11 / 23) = 4 - 0 = 4 points (integer division)
        // (time_offset = 12-1 = 11 hours, time_range = 24-1 = 23 hours, points_diff = 4-2 = 2)
        assert_eq!(score, 4, "12 hours should interpolate to 4 points");

        // at 18 hours for more significant interpolation
        let eighteen_hours_ago = current_time - (18 * 60 * 60);
        let score = tracker.get_modification_score(eighteen_hours_ago, git_status, FFFMode::Neovim);
        // Expected: 4 - (2 * 17 / 23) = 4 - 1 = 3 points
        assert_eq!(score, 3, "18 hours should interpolate to 3 points");

        let score = tracker.get_modification_score(five_minutes_ago, None, FFFMode::Neovim);
        assert_eq!(score, 0, "No git status should return 0");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
