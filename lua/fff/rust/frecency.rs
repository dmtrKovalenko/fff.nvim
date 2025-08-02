use crate::error::Error;
use crate::file_key::FileKey;
use heed::{
    types::{Bytes, SerdeBincode},
    EnvFlags,
};
use heed::{Database, Env, EnvOpenOptions};
use std::collections::VecDeque;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

const DECAY_CONSTANT: f64 = 0.0693; // ln(2)/10 for 10-day half-life
const SECONDS_PER_DAY: f64 = 86400.0;
const MAX_HISTORY_DAYS: f64 = 30.0; // Only consider accesses within 30 days

#[derive(Debug)]
pub struct FrecencyTracker {
    env: Env,
    db: Database<Bytes, SerdeBincode<VecDeque<u64>>>,
}

const ACCESS_THRESHOLDS: [(i64, u64); 5] = [
    (12, 60 * 2),          // 2 minutes
    (6, 60 * 10),          // 10 minutes
    (4, 60 * 60),          // 1 hour
    (2, 60 * 60 * 24),     // 1 day
    (1, 60 * 60 * 24 * 7), // 1 week
];

impl FrecencyTracker {
    pub fn new(db_path: &str, use_unsafe_no_lock: bool) -> Result<Self, Error> {
        fs::create_dir_all(db_path).map_err(Error::CreateDir)?;
        let env = unsafe {
            let mut opts = EnvOpenOptions::new();
            if use_unsafe_no_lock {
                opts.flags(EnvFlags::NO_LOCK | EnvFlags::NO_SYNC | EnvFlags::NO_META_SYNC);
            }
            opts.open(db_path).map_err(Error::EnvOpen)?
        };
        env.clear_stale_readers()
            .map_err(Error::DbClearStaleReaders)?;

        // we will open the default unnamed database
        let mut wtxn = env.write_txn().map_err(Error::DbStartWriteTxn)?;
        let db = env
            .create_database(&mut wtxn, None)
            .map_err(Error::DbCreate)?;

        Ok(FrecencyTracker {
            db,
            env: env.clone(),
        })
    }

    fn get_accesses(&self, file_key: &FileKey) -> Result<Option<VecDeque<u64>>, Error> {
        let rtxn = self.env.read_txn().map_err(Error::DbStartReadTxn)?;
        let key_hash = Self::path_to_hash_bytes(&file_key.path);
        self.db.get(&rtxn, &key_hash).map_err(Error::DbRead)
    }

    fn get_now(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    fn path_to_hash_bytes(path: &str) -> [u8; 32] {
        *blake3::hash(path.as_bytes()).as_bytes()
    }

    pub fn track_access(&self, file_key: &FileKey) -> Result<(), Error> {
        let mut wtxn = self.env.write_txn().map_err(Error::DbStartWriteTxn)?;

        let key_hash = Self::path_to_hash_bytes(&file_key.path);
        let mut accesses = self.get_accesses(file_key)?.unwrap_or_default();

        let now = self.get_now();
        let cutoff_time = now.saturating_sub((MAX_HISTORY_DAYS * SECONDS_PER_DAY) as u64);
        while let Some(&front_time) = accesses.front() {
            if front_time < cutoff_time {
                accesses.pop_front();
            } else {
                break;
            }
        }

        accesses.push_back(now);
        self.db
            .put(&mut wtxn, &key_hash, &accesses)
            .map_err(Error::DbWrite)?;

        wtxn.commit().map_err(Error::DbCommit)?;

        Ok(())
    }

    pub fn get_access_score(&self, file_key: &FileKey) -> i64 {
        let accesses = self
            .get_accesses(file_key)
            .unwrap_or(None)
            .unwrap_or_default();

        if accesses.is_empty() {
            return 0;
        }

        let now = self.get_now();
        let mut total_frecency = 0.0;

        let cutoff_time = now.saturating_sub((MAX_HISTORY_DAYS * SECONDS_PER_DAY) as u64);

        for &access_time in accesses.iter().rev() {
            if access_time < cutoff_time {
                break; // All remaining entries are older, stop processing
            }

            let days_ago = (now.saturating_sub(access_time) as f64) / SECONDS_PER_DAY;
            let decay_factor = (-DECAY_CONSTANT * days_ago).exp();
            total_frecency += decay_factor;
        }

        let normalized_frecency = if total_frecency <= 10.0 {
            total_frecency
        } else {
            10.0 + (total_frecency - 10.0).sqrt() // Diminishing: >10 accesses grow slowly
        };

        normalized_frecency.round() as i64
    }

    /// Calculate modification frecency score (0-12 points, git-aware)
    pub fn get_modification_score(&self, modified_time: u64, git_status: &str) -> i64 {
        let git_shows_changes = matches!(
            git_status,
            "modified" | "staged_modified" | "untracked" | "staged_new"
        );

        if !git_shows_changes {
            return 0; // No modification score for clean/unchanged files
        }

        let now = self.get_now();
        let duration_since = now.saturating_sub(modified_time);

        for (base_points, threshold_seconds) in ACCESS_THRESHOLDS {
            if duration_since <= threshold_seconds {
                return base_points * 2;
            }
        }

        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
