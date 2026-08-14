//! Regression test for #664.
//!
//! Default `heed::EnvOpenOptions` opens the env in `WithTls` mode, where LMDB
//! ties reader locktable slots to OS threads instead of `MDB_txn` objects. Every
//! thread that ever calls `read_txn()` occupies a reader slot for the lifetime
//! of the process, even after the txn commits. fff hits this with rayon workers,
//! the background watcher, the LMDB GC thread and the neovim main thread — a
//! handful of long-running nvim sessions exhaust the default 126-slot table and
//! new nvim processes crash with `MDB_READERS_FULL`.
//!
//! The fix opens the env with `read_txn_without_tls()`. This test spawns many
//! more short-lived threads than `maxreaders` and confirms none of them fail;
//! without the fix it fails around thread 127.

use fff_search::frecency::FrecencyTracker;
use std::sync::Arc;
use std::thread;

#[test]
fn read_txns_from_many_threads_do_not_exhaust_readers() {
    let tmp = tempfile::TempDir::new().unwrap();
    let tracker = Arc::new(FrecencyTracker::open(tmp.path()).unwrap());

    // Well above LMDB's default maxreaders (126). With WithTls each of these
    // would permanently pin a slot; ~127th thread would return MDB_READERS_FULL.
    const N: usize = 400;
    let handles: Vec<_> = (0..N)
        .map(|i| {
            let t = Arc::clone(&tracker);
            thread::Builder::new()
                .name(format!("reader-{i}"))
                .spawn(move || {
                    let path = std::path::PathBuf::from(format!("/tmp/frecency-slot-leak/{i}"));
                    t.access_count(&path)
                        .expect("read txn should not fail with MDB_READERS_FULL")
                })
                .unwrap()
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }
}
