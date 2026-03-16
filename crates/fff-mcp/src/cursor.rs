//! Cursor store for grep pagination.
//!
//! Maintains an in-memory map of opaque cursor IDs to file offsets.
//! Cursors are evicted LRU-style when the store exceeds capacity.

use std::collections::{HashMap, VecDeque};

const MAX_CURSORS: usize = 20;

/// Stores cursor state for paginated grep results.
pub struct CursorStore {
    counter: u64,
    /// Map from cursor ID string → file offset for next page.
    cursors: HashMap<String, usize>,
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
        self.counter = self.counter.wrapping_add(1);
        let id = self.counter.to_string();

        self.cursors.insert(id.clone(), file_offset);
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
        self.cursors.get(id).copied()
    }
}
