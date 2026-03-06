//! Cursor store for grep pagination.
//!
//! Maintains an in-memory map of opaque cursor IDs to file offsets.
//! Cursors are evicted LRU-style when the store exceeds capacity.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

const MAX_CURSORS: usize = 20;

/// Stores cursor state for paginated grep results.
pub struct CursorStore {
    counter: AtomicU64,
    /// Map from cursor ID string → file offset for next page.
    cursors: HashMap<String, usize>,
    /// Insertion order for LRU eviction.
    insertion_order: Vec<String>,
}

impl CursorStore {
    pub fn new() -> Self {
        Self {
            counter: AtomicU64::new(0),
            cursors: HashMap::new(),
            insertion_order: Vec::new(),
        }
    }

    /// Store a cursor and return its opaque ID string.
    pub fn store(&mut self, file_offset: usize) -> String {
        let id = self
            .counter
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1)
            .to_string();

        self.cursors.insert(id.clone(), file_offset);
        self.insertion_order.push(id.clone());

        // Evict oldest cursors
        while self.cursors.len() > MAX_CURSORS {
            if let Some(oldest) = self.insertion_order.first().cloned() {
                self.cursors.remove(&oldest);
                self.insertion_order.remove(0);
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
