use std::{
    collections::HashMap,
    fs,
    io,
    path::Path,
};

use serde::{Deserialize, Serialize};

/// Persistent routing state: ring snapshot + per-worker loaded roots.
/// Written atomically to routing.json; loaded by master on startup to
/// reconnect surviving workers after a crash.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct RoutingTable {
    /// Serializable snapshot of the consistent hash ring (virtual nodes).
    pub ring_state: SerializableRing,
    /// worker_index → WorkerEntry for all currently registered workers.
    pub workers: HashMap<u32, WorkerEntry>,
}

/// Per-worker persistent state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerEntry {
    pub index: u32,
    pub socket_path: String,
    pub pid: u32,
    /// Slugs of roots currently loaded in this worker's in-memory registry.
    pub root_slugs: Vec<String>,
}

/// Serializable form of the hash ring's virtual-node list.
/// The actual `HashRing` type lives in `fff-engine` and imports this as its
/// persistence representation so that `fff-ipc` stays dependency-free from
/// engine internals.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct SerializableRing {
    /// Sorted list of `(ring_point, worker_index)` virtual nodes.
    pub nodes: Vec<(u64, u32)>,
}

impl RoutingTable {
    /// Number of roots currently assigned to `worker_index`.
    pub fn entries_for_worker(&self, worker_index: u32) -> usize {
        self.workers
            .get(&worker_index)
            .map(|e| e.root_slugs.len())
            .unwrap_or(0)
    }

    /// Total number of registered workers.
    pub fn worker_count(&self) -> usize {
        self.workers.len()
    }

    /// Load from a JSON file. Returns `Ok(Default)` when the file is absent.
    pub fn load(path: &Path) -> io::Result<Self> {
        match fs::read_to_string(path) {
            Ok(s) => serde_json::from_str(&s)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }

    /// Atomically persist to a JSON file (write-to-tmp then rename).
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        fs::write(&tmp, json)?;
        fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_entry(index: u32, slugs: &[&str]) -> WorkerEntry {
        WorkerEntry {
            index,
            socket_path: format!("worker-{index}.sock"),
            pid: 1000 + index,
            root_slugs: slugs.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn routing_table_round_trips_json() {
        let mut rt = RoutingTable::default();
        rt.workers.insert(0, make_entry(0, &["abc", "def"]));
        rt.workers.insert(1, make_entry(1, &[]));
        rt.ring_state = SerializableRing { nodes: vec![(100, 0), (500, 1)] };

        let json = serde_json::to_string(&rt).unwrap();
        let rt2: RoutingTable = serde_json::from_str(&json).unwrap();

        assert_eq!(rt2.workers.len(), 2);
        assert_eq!(rt2.workers[&0].root_slugs, vec!["abc", "def"]);
        assert_eq!(rt2.ring_state.nodes, vec![(100, 0), (500, 1)]);
    }

    #[test]
    fn entries_for_worker_counts_slugs() {
        let mut rt = RoutingTable::default();
        rt.workers.insert(0, make_entry(0, &["a", "b", "c"]));
        rt.workers.insert(1, make_entry(1, &[]));

        assert_eq!(rt.entries_for_worker(0), 3);
        assert_eq!(rt.entries_for_worker(1), 0);
        assert_eq!(rt.entries_for_worker(99), 0);
    }

    #[test]
    fn load_returns_default_when_absent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("routing.json");
        let rt = RoutingTable::load(&path).unwrap();
        assert!(rt.workers.is_empty());
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("routing.json");

        let mut rt = RoutingTable::default();
        rt.workers.insert(0, make_entry(0, &["slug1"]));
        rt.save(&path).unwrap();

        let rt2 = RoutingTable::load(&path).unwrap();
        assert_eq!(rt2.workers[&0].root_slugs, vec!["slug1"]);
    }

    #[test]
    fn save_is_atomic_no_partial_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("subdir").join("routing.json");
        let mut rt = RoutingTable::default();
        rt.workers.insert(0, make_entry(0, &["x"]));
        // Parent dir doesn't exist yet — save should create it.
        rt.save(&path).unwrap();
        assert!(path.exists());
        // No .tmp residue.
        assert!(!path.with_extension("json.tmp").exists());
    }
}
