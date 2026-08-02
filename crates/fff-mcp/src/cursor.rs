//! Cursor store for grep pagination.
//!
//! Maintains an in-memory map of opaque cursor IDs to file offsets.
//! Cursors are evicted LRU-style when the store exceeds capacity.

use fff::grep::GrepMatch;
use std::collections::{HashMap, HashSet, VecDeque};

const MAX_CURSORS: usize = 20;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct MatchKey {
    pub(crate) file_path: String,
    pub(crate) line_number: u64,
    pub(crate) byte_offset: u64,
}

impl MatchKey {
    pub(crate) fn new(file_path: String, line_number: u64, byte_offset: u64) -> Self {
        Self {
            file_path,
            line_number,
            byte_offset,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PendingMatch {
    pub(crate) file_path: String,
    pub(crate) match_data: GrepMatch,
}

#[derive(Clone, Debug)]
pub(crate) struct MultiGrepCursor {
    pub(crate) patterns: Vec<String>,
    pub(crate) constraints: String,
    pub(crate) next_offsets: Vec<Option<usize>>,
    pub(crate) pending: Vec<PendingMatch>,
    pub(crate) seen: HashSet<MatchKey>,
    pub(crate) seen_files: HashSet<String>,
}

impl MultiGrepCursor {
    pub(crate) fn new(patterns: Vec<String>, constraints: String) -> Self {
        Self {
            next_offsets: vec![Some(0); patterns.len()],
            patterns,
            constraints,
            pending: Vec::new(),
            seen: HashSet::new(),
            seen_files: HashSet::new(),
        }
    }

    pub(crate) fn has_more(&self) -> bool {
        !self.pending.is_empty() || self.next_offsets.iter().any(Option::is_some)
    }

    pub(crate) fn remember_match(&mut self, key: MatchKey) -> bool {
        self.seen.insert(key)
    }

    pub(crate) fn remember_file(&mut self, path: String) -> bool {
        self.seen_files.insert(path)
    }
}

enum CursorState {
    FileOffset(usize),
    MultiGrep(MultiGrepCursor),
}

/// Stores cursor state for paginated grep results.
pub struct CursorStore {
    counter: u64,
    /// Map from cursor ID string to the state required for the next page.
    cursors: HashMap<String, CursorState>,
    /// Insertion order for LRU eviction.
    insertion_order: VecDeque<String>,
}

impl CursorStore {
    pub fn new() -> Self {
        Self {
            counter: 0,
            cursors: HashMap::new(),
            insertion_order: VecDeque::new(),
        }
    }

    /// Store a cursor and return its opaque ID string.
    pub fn store(&mut self, file_offset: usize) -> String {
        self.store_state(CursorState::FileOffset(file_offset))
    }

    pub(crate) fn store_multi_grep(&mut self, cursor: MultiGrepCursor) -> String {
        self.store_state(CursorState::MultiGrep(cursor))
    }

    pub(crate) fn get_multi_grep(&self, id: &str) -> Option<MultiGrepCursor> {
        match self.cursors.get(id) {
            Some(CursorState::MultiGrep(cursor)) => Some(cursor.clone()),
            Some(CursorState::FileOffset(_)) | None => None,
        }
    }

    fn store_state(&mut self, state: CursorState) -> String {
        self.counter = self.counter.wrapping_add(1);
        let id = self.counter.to_string();

        self.cursors.insert(id.clone(), state);
        self.insertion_order.push_back(id.clone());

        // Evict oldest cursors
        while self.cursors.len() > MAX_CURSORS {
            if let Some(oldest) = self.insertion_order.pop_front() {
                self.cursors.remove(&oldest);
            } else {
                break;
            }
        }

        id
    }

    /// Retrieve the file offset for a cursor ID.
    pub fn get(&self, id: &str) -> Option<usize> {
        match self.cursors.get(id) {
            Some(CursorState::FileOffset(offset)) => Some(*offset),
            Some(CursorState::MultiGrep(_)) | None => None,
        }
    }
}
