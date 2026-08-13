//! One process must be able to hold many trackers over the same LMDB path
//! (issues #700/#760): they share a single pooled env instead of failing
//! with `EnvAlreadyOpened`.

use std::path::{Path, PathBuf};

use fff_search::frecency::FrecencyTracker;
use fff_search::query_tracker::QueryTracker;
use fff_search::shared::SharedFrecency;

fn unique_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("fff-env-pool-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn same_path_trackers_share_one_env() {
    let dir = unique_dir("share");
    let file = Path::new("/virtual/env-pool/shared.rs");

    let a = FrecencyTracker::open(&dir).expect("first open");
    let b = FrecencyTracker::open(&dir).expect("second open in the same process (#700/#760)");

    a.track_access(file).expect("write via a");
    assert_eq!(b.access_count(file).expect("read via b"), 1);

    drop(a);
    b.track_access(file)
        .expect("b must stay usable after a drops");
    assert_eq!(b.access_count(file).unwrap(), 2);
    drop(b);

    let c = FrecencyTracker::open(&dir).expect("reopen after all handles dropped");
    assert_eq!(
        c.access_count(file).unwrap(),
        2,
        "data persisted across reopen"
    );

    drop(c);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn concurrent_open_and_drop_never_collide() {
    let dir = unique_dir("hammer");
    let file = Path::new("/virtual/env-pool/hammer.rs");

    let mut handles = Vec::new();
    for t in 0..8 {
        let dir = dir.clone();
        handles.push(std::thread::spawn(move || {
            for i in 0..100 {
                let tracker = FrecencyTracker::open(&dir)
                    .unwrap_or_else(|e| panic!("thread {t} iteration {i}: {e}"));
                if i % 20 == 0 {
                    tracker.track_access(file).expect("track access");
                }
            }
        }));
    }
    for handle in handles {
        handle.join().expect("no thread may panic");
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn different_store_on_same_path_is_rejected_with_clear_error() {
    let dir = unique_dir("mismatch");

    let _frecency = FrecencyTracker::open(&dir).expect("frecency open");
    let err = QueryTracker::open(&dir).expect_err("env options differ, must be rejected");

    let msg = err.to_string();
    assert!(
        msg.contains("frecency") && msg.contains("query"),
        "error must name both stores so the user can fix their config, got: {msg}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn destroy_refuses_while_shared_then_succeeds_when_sole() {
    let dir = unique_dir("destroy");
    let file = Path::new("/virtual/env-pool/destroy.rs");

    let shared = SharedFrecency::default();
    shared
        .init(FrecencyTracker::open(&dir).expect("init open"))
        .expect("init");
    let other = FrecencyTracker::open(&dir).expect("second handle over the same db");

    shared
        .destroy()
        .expect_err("destroy must refuse while another tracker uses the env");

    // Refusal must keep both the files and the shared handle intact.
    assert!(
        dir.join("data.mdb").exists(),
        "db files survive a refused destroy"
    );
    shared
        .read()
        .expect("read lock")
        .as_ref()
        .expect("tracker restored after refused destroy")
        .track_access(file)
        .expect("shared handle still works");

    drop(other);
    let removed = shared
        .destroy()
        .expect("sole-owner destroy succeeds")
        .expect("a path was removed");
    assert!(
        !removed.exists(),
        "db dir deleted once nobody shares the env"
    );
}
