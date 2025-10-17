use crate::{error::Error, git::is_modified_status};
use heed::{Database, Env, EnvOpenOptions};
use heed::{
    EnvFlags,
    types::{Bytes, SerdeBincode},
};
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};
use std::{collections::VecDeque, path::Path};

const DECAY_CONSTANT: f64 = 0.0693; // ln(2)/10 for 10-day half-life
const SECONDS_PER_DAY: f64 = 86400.0;
const MAX_HISTORY_DAYS: f64 = 30.0; // Only consider accesses within 30 days

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

    fn get_accesses(&self, path: &Path) -> Result<Option<VecDeque<u64>>, Error> {
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

    fn path_to_hash_bytes(path: &Path) -> Result<[u8; 32], Error> {
        let Some(key) = path.to_str() else {
            return Err(Error::InvalidPath(path.to_path_buf()));
        };

        Ok(*blake3::hash(key.as_bytes()).as_bytes())
    }

    pub fn track_access(&self, path: &Path) -> Result<(), Error> {
        let mut wtxn = self.env.write_txn().map_err(Error::DbStartWriteTxn)?;

        let key_hash = Self::path_to_hash_bytes(path)?;
        let mut accesses = self.get_accesses(path)?.unwrap_or_default();

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
        tracing::debug!(?path, accesses = accesses.len(), "Tracking access");

        self.db
            .put(&mut wtxn, &key_hash, &accesses)
            .map_err(Error::DbWrite)?;

        wtxn.commit().map_err(Error::DbCommit)?;

        Ok(())
    }

    pub fn get_access_score(&self, file_path: &Path) -> i64 {
        let accesses = self
            .get_accesses(file_path)
            .ok()
            .flatten()
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

    /// Calculating modification score but only if the file is modified in the current git dir
    pub fn get_modification_score(
        &self,
        modified_time: u64,
        git_status: Option<git2::Status>,
    ) -> i64 {
        let is_modified_git_status = git_status.is_some_and(is_modified_status);
        if !is_modified_git_status {
            return 0;
        }

        let now = self.get_now();
        let duration_since = now.saturating_sub(modified_time);

        for i in 0..MODIFICATION_THRESHOLDS.len() {
            let (current_points, current_threshold) = MODIFICATION_THRESHOLDS[i];

            if duration_since <= current_threshold {
                if i == 0 || duration_since == current_threshold {
                    return current_points;
                }

                let (prev_points, prev_threshold) = MODIFICATION_THRESHOLDS[i - 1];

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
        let tracker = FrecencyTracker::new(temp_dir.to_str().unwrap(), true).unwrap();

        let current_time = tracker.get_now();
        let git_status = Some(git2::Status::WT_MODIFIED);

        // At 5 minutes: should interpolate between 16 and 8 points
        let five_minutes_ago = current_time - (5 * 60);
        let score = tracker.get_modification_score(five_minutes_ago, git_status);

        // Expected: 16 - (8 * 3 / 13) = 16 - 1 = 15 points
        // (time_offset = 5-2 = 3, time_range = 15-2 = 13, points_diff = 16-8 = 8)
        assert_eq!(score, 15, "5 minutes should interpolate to 15 points");

        let two_minutes_ago = current_time - (2 * 60);
        let score = tracker.get_modification_score(two_minutes_ago, git_status);
        assert_eq!(score, 16, "2 minutes should be exactly 16 points");

        let fifteen_minutes_ago = current_time - (15 * 60);
        let score = tracker.get_modification_score(fifteen_minutes_ago, git_status);
        assert_eq!(score, 8, "15 minutes should be exactly 8 points");

        // At 12 hours: should interpolate between 4 and 2 points
        let twelve_hours_ago = current_time - (12 * 60 * 60);
        let score = tracker.get_modification_score(twelve_hours_ago, git_status);
        // Expected: 4 - (2 * 11 / 23) = 4 - 0 = 4 points (integer division)
        // (time_offset = 12-1 = 11 hours, time_range = 24-1 = 23 hours, points_diff = 4-2 = 2)
        assert_eq!(score, 4, "12 hours should interpolate to 4 points");

        // at 18 hours for more significant interpolation
        let eighteen_hours_ago = current_time - (18 * 60 * 60);
        let score = tracker.get_modification_score(eighteen_hours_ago, git_status);
        // Expected: 4 - (2 * 17 / 23) = 4 - 1 = 3 points
        assert_eq!(score, 3, "18 hours should interpolate to 3 points");

        let score = tracker.get_modification_score(five_minutes_ago, None);
        assert_eq!(score, 0, "No git status should return 0");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
