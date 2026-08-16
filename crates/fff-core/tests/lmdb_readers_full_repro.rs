// Repro for #783: fff opens LMDB envs with only map_size set, leaving heed's
// default max_readers (126) and default TLS mode. Long-lived threads each pin a
// reader slot for the thread's lifetime, so >126 live reader threads exhaust the
// table with MDB_READERS_FULL.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, mpsc};
use std::time::Duration;

use heed::EnvOpenOptions;

fn temp_env_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("fff-readers-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// Regression for #783: with only map_size set (pre-fix), heed's default 126
// reader slots are exhausted once >126 live threads each hold a read txn. fff now
// raises max_readers, so this many live readers must all get a slot.
const FFF_MAX_READERS: u32 = 1024;

#[test]
fn raised_max_readers_admits_more_than_126_live_readers() {
    let dir = temp_env_dir("raised");
    let env = unsafe {
        EnvOpenOptions::new()
            .map_size(10 * 1024 * 1024)
            .max_readers(FFF_MAX_READERS)
            .open(&dir)
    }
    .unwrap();

    const THREADS: usize = 200;
    let stop = Arc::new(AtomicBool::new(false));
    let ready = Arc::new(Barrier::new(THREADS + 1));
    let readers_full = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<bool>(); // true = read txn acquired

    let mut handles = Vec::new();
    for _ in 0..THREADS {
        let env = env.clone();
        let stop = stop.clone();
        let ready = ready.clone();
        let readers_full = readers_full.clone();
        let tx = tx.clone();
        handles.push(std::thread::spawn(move || {
            match env.read_txn() {
                Ok(txn) => {
                    tx.send(true).ok();
                    ready.wait();
                    while !stop.load(Ordering::Relaxed) {
                        std::thread::park_timeout(Duration::from_millis(5));
                    }
                    drop(txn); // hold the slot for the whole test
                }
                Err(e) => {
                    if e.to_string().contains("MDB_READERS_FULL") {
                        readers_full.store(true, Ordering::Relaxed);
                    }
                    tx.send(false).ok();
                    ready.wait();
                }
            }
        }));
    }
    drop(tx);

    // Collect exactly one result per thread; parked threads keep their tx clone
    // alive, so we must not wait for the channel to close.
    let acquired = AtomicUsize::new(0);
    for _ in 0..THREADS {
        if rx.recv().unwrap() {
            acquired.fetch_add(1, Ordering::Relaxed);
        }
    }
    ready.wait();

    let acquired = acquired.load(Ordering::Relaxed);
    stop.store(true, Ordering::Relaxed);
    for h in handles {
        h.join().unwrap();
    }
    let _ = std::fs::remove_dir_all(&dir);

    // With max_readers raised, all 200 live reader threads must get a slot and
    // none may see MDB_READERS_FULL. On the pre-fix default of 126 this plateaus
    // at 126 and the rest fail.
    assert!(
        !readers_full.load(Ordering::Relaxed),
        "MDB_READERS_FULL hit: only {acquired}/{THREADS} live reader threads got a slot"
    );
    assert_eq!(
        acquired, THREADS,
        "all {THREADS} live reader threads should get a slot; got {acquired}"
    );
}
