use crate::db_healthcheck::DbHealthChecker;
use crate::error::Error;
use heed::types::Bytes;
use heed::{Database, Env, EnvOpenOptions};
use heed::{EnvFlags, types::SerdeBincode};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_HISTORY_ENTRIES: usize = 128;

/// Simplified QueryFileEntry without redundant fields
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct QueryMatchEntry {
    pub file_path: PathBuf, // File that was actually opened
    pub open_count: u32,    // Number of times opened with this query
    pub last_opened: u64,   // Unix timestamp
}

/// Entry for query history tracking
#[derive(Debug, Serialize, Deserialize, Clone)]
struct HistoryEntry {
    query: String,
    timestamp: u64,
}

#[derive(Debug)]
pub struct QueryTracker {
    env: Env,
    // Database for (project_path, query) -> QueryMatchEntry mappings
    query_file_db: Database<Bytes, SerdeBincode<QueryMatchEntry>>,
    // Database for project_path -> VecDeque<HistoryEntry> mappings (file picker)
    query_history_db: Database<Bytes, SerdeBincode<VecDeque<HistoryEntry>>>,
    // Database for project_path -> VecDeque<HistoryEntry> mappings (grep)
    grep_query_history_db: Database<Bytes, SerdeBincode<VecDeque<HistoryEntry>>>,
}

impl DbHealthChecker for QueryTracker {
    fn get_env(&self) -> &Env {
        &self.env
    }

    fn count_entries(&self) -> Result<Vec<(&'static str, u64)>, Error> {
        let rtxn = self.env.read_txn().map_err(Error::DbStartReadTxn)?;

        let count_queries = self.query_file_db.len(&rtxn).map_err(Error::DbRead)?;
        let count_histories = self.query_history_db.len(&rtxn).map_err(Error::DbRead)?;
        let count_grep_histories = self
            .grep_query_history_db
            .len(&rtxn)
            .map_err(Error::DbRead)?;

        Ok(vec![
            ("query_file_entries", count_queries),
            ("query_history_entries", count_histories),
            ("grep_query_history_entries", count_grep_histories),
        ])
    }
}

impl QueryTracker {
    pub fn new(db_path: &str, use_unsafe_no_lock: bool) -> Result<Self, Error> {
        fs::create_dir_all(db_path).map_err(Error::CreateDir)?;
        let env = unsafe {
            let mut opts = EnvOpenOptions::new();
            opts.max_dbs(16); // Allow up to 16 databases per environment
            if use_unsafe_no_lock {
                opts.flags(EnvFlags::NO_LOCK | EnvFlags::NO_SYNC | EnvFlags::NO_META_SYNC);
            }
            opts.open(db_path).map_err(Error::EnvOpen)?
        };

        env.clear_stale_readers()
            .map_err(Error::DbClearStaleReaders)?;

        let mut wtxn = env.write_txn().map_err(Error::DbStartWriteTxn)?;

        // Create two named databases
        let query_file_db = env
            .create_database(&mut wtxn, Some("query_file_associations"))
            .map_err(Error::DbCreate)?;
        let query_history_db = env
            .create_database(&mut wtxn, Some("query_history"))
            .map_err(Error::DbCreate)?;
        let grep_query_history_db = env
            .create_database(&mut wtxn, Some("grep_query_history"))
            .map_err(Error::DbCreate)?;

        wtxn.commit().map_err(Error::DbCommit)?;

        Ok(QueryTracker {
            env,
            query_file_db,
            query_history_db,
            grep_query_history_db,
        })
    }

    fn get_now(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    fn create_query_key(project_path: &Path, query: &str) -> Result<[u8; 32], Error> {
        let project_str = project_path
            .to_str()
            .ok_or_else(|| Error::InvalidPath(project_path.to_path_buf()))?;

        let mut hasher = blake3::Hasher::default();
        hasher.update(project_str.as_bytes());
        hasher.update(b"::");
        hasher.update(query.as_bytes());

        Ok(*hasher.finalize().as_bytes())
    }

    fn create_project_key(project_path: &Path) -> Result<[u8; 32], Error> {
        let project_str = project_path
            .to_str()
            .ok_or_else(|| Error::InvalidPath(project_path.to_path_buf()))?;

        Ok(*blake3::hash(project_str.as_bytes()).as_bytes())
    }

    /// Append a query to a history database within an existing write transaction.
    fn append_to_history(
        db: &Database<Bytes, SerdeBincode<VecDeque<HistoryEntry>>>,
        wtxn: &mut heed::RwTxn,
        project_key: &[u8; 32],
        query: &str,
        now: u64,
    ) -> Result<(), Error> {
        let mut history = db
            .get(wtxn, project_key)
            .map_err(Error::DbRead)?
            .unwrap_or_default();

        history.push_back(HistoryEntry {
            query: query.to_string(),
            timestamp: now,
        });
        while history.len() > MAX_HISTORY_ENTRIES {
            history.pop_front();
        }

        db.put(wtxn, project_key, &history)
            .map_err(Error::DbWrite)?;
        Ok(())
    }

    /// Read a query from a history database at a specific offset.
    /// offset=0 returns most recent, offset=1 returns 2nd most recent, etc.
    fn read_history_at_offset(
        db: &Database<Bytes, SerdeBincode<VecDeque<HistoryEntry>>>,
        env: &Env,
        project_key: &[u8; 32],
        offset: usize,
    ) -> Result<Option<String>, Error> {
        let rtxn = env.read_txn().map_err(Error::DbStartReadTxn)?;

        let mut history = db
            .get(&rtxn, project_key)
            .map_err(Error::DbRead)?
            .unwrap_or_default();

        // history is FIFO, last element is most recent
        if history.len() > offset {
            let index = history.len() - 1 - offset;
            let record = history.remove(index);
            Ok(record.map(|r| r.query))
        } else {
            Ok(None)
        }
    }

    pub fn track_query_completion(
        &mut self,
        query: &str,
        project_path: &Path,
        file_path: &Path,
    ) -> Result<(), Error> {
        let now = self.get_now();
        let file_path_buf = file_path.to_path_buf();

        let query_key = Self::create_query_key(project_path, query)?;
        let mut wtxn = self.env.write_txn().map_err(Error::DbStartWriteTxn)?;

        let mut entry = self
            .query_file_db
            .get(&wtxn, &query_key)
            .map_err(Error::DbRead)?
            .unwrap_or_else(|| QueryMatchEntry {
                file_path: file_path_buf.clone(),
                open_count: 0,
                last_opened: now,
            });

        if entry.file_path == file_path_buf {
            tracing::debug!(
                ?query,
                ?file_path,
                "Query completed for same file as last time"
            );

            // Same file - just increment count
            entry.open_count += 1;
        } else {
            tracing::debug!(
                ?query,
                ?file_path,
                "Query completed for different file than last time"
            );

            // Different file - replace and reset count to 1
            entry.file_path = file_path_buf;
            entry.open_count = 1;
        }

        entry.last_opened = now;

        self.query_file_db
            .put(&mut wtxn, &query_key, &entry)
            .map_err(Error::DbWrite)?;

        // Update query history database
        let project_key = Self::create_project_key(project_path)?;
        Self::append_to_history(&self.query_history_db, &mut wtxn, &project_key, query, now)?;

        wtxn.commit().map_err(Error::DbCommit)?;

        tracing::debug!(?query, ?file_path, "Tracked query completion");
        Ok(())
    }

    pub fn get_last_query_entry(
        &self,
        query: &str,
        project_path: &Path,
        min_combo_count: u32,
    ) -> Result<Option<QueryMatchEntry>, Error> {
        let query_key = Self::create_query_key(project_path, query)?;
        tracing::debug!(?query_key, "HASH");
        let rtxn = self.env.read_txn().map_err(Error::DbStartReadTxn)?;

        let last_match = self
            .query_file_db
            .get(&rtxn, &query_key)
            .map_err(Error::DbRead)?;

        Ok(last_match.filter(|entry| entry.open_count >= min_combo_count))
    }

    pub fn get_last_query_path(
        &self,
        query: &str,
        project_path: &Path,
        file_path: &Path,
        combo_boost: i32,
    ) -> Result<i32, Error> {
        let query_key = Self::create_query_key(project_path, query)?;
        tracing::debug!(?query_key, "HASH");
        let rtxn = self.env.read_txn().map_err(Error::DbStartReadTxn)?;

        match self
            .query_file_db
            .get(&rtxn, &query_key)
            .map_err(Error::DbRead)?
        {
            Some(entry) => {
                // Check if the file path matches and return boost
                if entry.file_path == file_path && entry.open_count >= 2 {
                    Ok(combo_boost)
                } else {
                    Ok(0)
                }
            }
            None => Ok(0), // Query not found
        }
    }

    /// Get query from file picker history at a specific offset.
    /// offset=0 returns most recent query, offset=1 returns 2nd most recent, etc.
    pub fn get_historical_query(
        &self,
        project_path: &Path,
        offset: usize,
    ) -> Result<Option<String>, Error> {
        let project_key = Self::create_project_key(project_path)?;
        Self::read_history_at_offset(&self.query_history_db, &self.env, &project_key, offset)
    }

    /// Track a grep query in the grep-specific history.
    /// Only records query history (no file association tracking needed for grep).
    pub fn track_grep_query(&mut self, query: &str, project_path: &Path) -> Result<(), Error> {
        let now = self.get_now();
        let project_key = Self::create_project_key(project_path)?;
        let mut wtxn = self.env.write_txn().map_err(Error::DbStartWriteTxn)?;

        Self::append_to_history(
            &self.grep_query_history_db,
            &mut wtxn,
            &project_key,
            query,
            now,
        )?;

        wtxn.commit().map_err(Error::DbCommit)?;

        tracing::debug!(?query, "Tracked grep query");
        Ok(())
    }

    /// Get grep query from history at a specific offset.
    /// offset=0 returns most recent grep query, offset=1 returns 2nd most recent, etc.
    pub fn get_historical_grep_query(
        &self,
        project_path: &Path,
        offset: usize,
    ) -> Result<Option<String>, Error> {
        let project_key = Self::create_project_key(project_path)?;
        Self::read_history_at_offset(&self.grep_query_history_db, &self.env, &project_key, offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_query_tracking() {
        let temp_dir = env::temp_dir().join("fff_test_query_tracking_new");
        let _ = std::fs::remove_dir_all(&temp_dir);

        let mut tracker = QueryTracker::new(temp_dir.to_str().unwrap(), true).unwrap();

        let project_path = PathBuf::from("/test/project");
        let file_path = PathBuf::from("/test/project/src/main.rs");

        // First completion
        tracker
            .track_query_completion("main", &project_path, &file_path)
            .unwrap();
        let boost = tracker
            .get_last_query_path("main", &project_path, &file_path, 10000)
            .unwrap();
        assert_eq!(boost, 0, "First completion should not boost");

        // Second completion - should boost now
        tracker
            .track_query_completion("main", &project_path, &file_path)
            .unwrap();
        let boost = tracker
            .get_last_query_path("main", &project_path, &file_path, 10000)
            .unwrap();
        assert_eq!(boost, 10000, "Second completion should boost");

        // Different file for same query - should reset count and no boost
        let other_file = PathBuf::from("/test/project/src/lib.rs");
        tracker
            .track_query_completion("main", &project_path, &other_file)
            .unwrap();
        let boost = tracker
            .get_last_query_path("main", &project_path, &other_file, 10000)
            .unwrap();
        assert_eq!(boost, 0, "Different file should reset boost");

        // Original file should no longer get boost (replaced by new file)
        let boost = tracker
            .get_last_query_path("main", &project_path, &file_path, 10000)
            .unwrap();
        assert_eq!(boost, 0, "Original file should not boost after replacement");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_hashing_functions() {
        let project_path = PathBuf::from("/test/project");

        // Test project key hashing
        let key1 = QueryTracker::create_project_key(&project_path).unwrap();
        let key2 = QueryTracker::create_project_key(&project_path).unwrap();
        assert_eq!(key1, key2, "Same project should hash to same key");

        // Test query key hashing
        let query_key1 = QueryTracker::create_query_key(&project_path, "test").unwrap();
        let query_key2 = QueryTracker::create_query_key(&project_path, "test").unwrap();
        assert_eq!(
            query_key1, query_key2,
            "Same project+query should hash to same key"
        );

        // Different queries should hash differently
        let query_key3 = QueryTracker::create_query_key(&project_path, "different").unwrap();
        assert_ne!(
            query_key1, query_key3,
            "Different queries should hash to different keys"
        );

        // Different projects should hash differently
        let other_project = PathBuf::from("/other/project");
        let query_key4 = QueryTracker::create_query_key(&other_project, "test").unwrap();
        assert_ne!(
            query_key1, query_key4,
            "Different projects should hash to different keys"
        );
    }
}
