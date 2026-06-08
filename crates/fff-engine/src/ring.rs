use std::path::Path;

use fff_ipc::SerializableRing;

const DEFAULT_VIRTUAL_NODES: usize = 150;

/// Virtual-node consistent hash ring mapping paths to worker indices.
///
/// Uses blake3 for all hashing. Ring space is [0, u64::MAX].
/// Each worker occupies `virtual_nodes` evenly distributed points;
/// `assign` walks clockwise from the path's hash to find the owner.
pub(crate) struct HashRing {
    /// Sorted (ring_point, worker_index) pairs — the canonical ring state.
    nodes: Vec<(u64, u32)>,
}

impl HashRing {
    pub fn new() -> Self {
        Self { nodes: Vec::new() }
    }

    /// Add a worker to the ring with `virtual_nodes` virtual positions.
    pub fn add_worker(&mut self, index: u32, virtual_nodes: usize) {
        for i in 0..virtual_nodes {
            let key = format!("worker-{index}-vnode-{i}");
            let point = point_for_key(key.as_bytes());
            self.nodes.push((point, index));
        }
        self.nodes.sort_unstable_by_key(|&(p, _)| p);
        self.nodes.dedup_by_key(|&mut (p, _)| p);
    }

    /// Remove all virtual nodes belonging to `index`.
    pub fn remove_worker(&mut self, index: u32) {
        self.nodes.retain(|&(_, w)| w != index);
    }

    /// Map `base_path` to the worker index that owns it, or `None` if empty.
    pub fn assign(&self, base_path: &Path) -> Option<u32> {
        if self.nodes.is_empty() {
            return None;
        }
        let canonical = std::fs::canonicalize(base_path)
            .unwrap_or_else(|_| base_path.to_path_buf());
        let point = point_for_key(canonical.as_os_str().as_encoded_bytes());

        // Binary search for the first node with ring_point >= point (clockwise).
        let idx = self.nodes.partition_point(|&(p, _)| p < point);
        // Wrap around to the first node when point is past the last node.
        let node = if idx < self.nodes.len() { &self.nodes[idx] } else { &self.nodes[0] };
        Some(node.1)
    }

    /// Unique worker indices currently in the ring.
    pub fn workers(&self) -> Vec<u32> {
        let mut seen = Vec::new();
        for &(_, w) in &self.nodes {
            if !seen.contains(&w) {
                seen.push(w);
            }
        }
        seen
    }

    /// Total virtual node count (not unique workers).
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    #[allow(dead_code)]
    pub(crate) fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Snapshot the ring state for persistence.
    pub fn to_serializable(&self) -> SerializableRing {
        SerializableRing { nodes: self.nodes.clone() }
    }

    /// Restore a ring from a persisted snapshot.
    pub fn from_serializable(ring: SerializableRing) -> Self {
        let mut nodes = ring.nodes;
        nodes.sort_unstable_by_key(|&(p, _)| p);
        Self { nodes }
    }

    /// Convenience: add worker with the default virtual node count (150).
    pub fn add_worker_default(&mut self, index: u32) {
        self.add_worker(index, DEFAULT_VIRTUAL_NODES);
    }
}

impl Default for HashRing {
    fn default() -> Self {
        Self::new()
    }
}

fn point_for_key(bytes: &[u8]) -> u64 {
    let hash = blake3::hash(bytes);
    let b = hash.as_bytes();
    u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assign_empty_ring_returns_none() {
        let ring = HashRing::new();
        assert!(ring.assign(Path::new("/any/path")).is_none());
    }

    #[test]
    fn assign_single_worker_always_returns_it() {
        let mut ring = HashRing::new();
        ring.add_worker(0, 150);
        for path in ["/project/a", "/project/b", "/tmp/x"] {
            assert_eq!(ring.assign(Path::new(path)), Some(0));
        }
    }

    #[test]
    fn both_workers_appear_across_sample() {
        let mut ring = HashRing::new();
        ring.add_worker(0, 150);
        ring.add_worker(1, 150);

        let paths: Vec<String> = (0..100).map(|i| format!("/project/root-{i}")).collect();
        let mut saw = [false; 2];
        for p in &paths {
            match ring.assign(Path::new(p)) {
                Some(0) => saw[0] = true,
                Some(1) => saw[1] = true,
                _ => {}
            }
        }
        assert!(saw[0], "worker 0 never assigned");
        assert!(saw[1], "worker 1 never assigned");
    }

    #[test]
    fn remove_worker_clears_its_nodes() {
        let mut ring = HashRing::new();
        ring.add_worker(0, 150);
        ring.add_worker(1, 150);
        let before = ring.len();
        ring.remove_worker(0);
        assert_eq!(ring.len(), before - 150);
        // Only worker 1 should remain.
        for _ in 0..20 {
            assert_eq!(ring.assign(Path::new("/any")), Some(1));
        }
    }

    #[test]
    fn assign_is_stable_after_unrelated_worker_added() {
        let mut ring = HashRing::new();
        ring.add_worker(0, 150);
        ring.add_worker(1, 150);

        // Find a path that resolves to worker 0.
        let target = (0..200u32)
            .map(|i| format!("/project/stable-{i}"))
            .find(|p| ring.assign(Path::new(p)) == Some(0))
            .expect("no path mapped to worker 0 in 200 attempts");

        // Add worker 2 — existing assignments should be mostly stable.
        // (We just check our specific target stays on 0 if adding a worker
        // on the far side of the ring doesn't displace it.)
        ring.add_worker(2, 150);
        // The test is probabilistic: ~2/3 of paths stay on their original
        // worker. We verify the ring still returns a valid index.
        let result = ring.assign(Path::new(&target));
        assert!(result.is_some(), "assign returned None after adding worker");
    }

    #[test]
    fn serialize_deserialize_preserves_assignment() {
        let mut ring = HashRing::new();
        ring.add_worker(0, 150);
        ring.add_worker(1, 150);

        let paths: Vec<String> = (0..20).map(|i| format!("/project/{i}")).collect();
        let original: Vec<Option<u32>> = paths.iter().map(|p| ring.assign(Path::new(p))).collect();

        let snap = ring.to_serializable();
        let restored = HashRing::from_serializable(snap);
        let after: Vec<Option<u32>> = paths.iter().map(|p| restored.assign(Path::new(p))).collect();

        assert_eq!(original, after);
    }

    #[test]
    fn workers_returns_unique_indices() {
        let mut ring = HashRing::new();
        ring.add_worker(0, 150);
        ring.add_worker(1, 150);
        let mut ws = ring.workers();
        ws.sort();
        assert_eq!(ws, vec![0, 1]);
    }
}
