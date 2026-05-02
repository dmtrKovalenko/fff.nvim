//! Regression test: stopping the background watcher while the caller
//! holds the [`SharedFilePicker`] write lock must NOT deadlock.
//!
//! There are two lock-ordering hazards the watcher has to handle:
//!
//!   1. The debouncer's event thread calls our handler, which wants
//!      `shared_picker.write()` to apply events. `stop()` used to
//!      `join()` that thread under the caller's write guard.
//!
//!   2. The owner thread registers new-directory watches and injects
//!      their existing files. Previously it held the debouncer mutex
//!      across `shared_picker.write()`, while `stop()` takes the
//!      debouncer mutex under the caller's write guard — inverse
//!      lock orders, classic deadlock.
//!
//! macOS FSEvents is the reliable reproducer for (1) because fresh
//! `fs::write()` calls inside a just-watched temp dir queue events
//! faster than the debounce tick can drain them. Creating new
//! subdirectories exercises (2) via the owner thread's `watch_tx`.

use std::fs;
use std::sync::mpsc;
use std::time::Duration;
use tempfile::TempDir;

use fff_search::file_picker::{FFFMode, FilePicker};
use fff_search::{FilePickerOptions, SharedFilePicker, SharedFrecency};

/// Run `f` on a worker thread, require it to finish within `timeout`,
/// panic with `msg` otherwise. The caller gets to describe what the
/// worker is doing so a hung test produces an actionable message.
fn run_with_deadlock_guard(
    msg: &'static str,
    timeout: Duration,
    f: impl FnOnce() + Send + 'static,
) {
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let worker = std::thread::Builder::new()
        .name("deadlock-guard-worker".into())
        .spawn(move || {
            f();
            let _ = done_tx.send(());
        })
        .expect("spawn worker");

    match done_rx.recv_timeout(timeout) {
        Ok(()) => {}
        Err(_) => panic!("{msg}"),
    }
    worker.join().expect("worker panicked");
}

fn make_watched_picker(base: &std::path::Path) -> (SharedFilePicker, SharedFrecency) {
    let shared_picker = SharedFilePicker::default();
    let shared_frecency = SharedFrecency::default();

    FilePicker::new_with_shared_state(
        shared_picker.clone(),
        shared_frecency.clone(),
        FilePickerOptions {
            base_path: base.to_string_lossy().to_string(),
            enable_mmap_cache: false,
            enable_content_indexing: false,
            mode: FFFMode::Neovim,
            watch: true,
            ..Default::default()
        },
    )
    .expect("Failed to create FilePicker");

    assert!(
        shared_picker.wait_for_scan(Duration::from_secs(10)),
        "initial scan never completed"
    );
    assert!(
        shared_picker.wait_for_watcher(Duration::from_secs(10)),
        "watcher never installed"
    );

    (shared_picker, shared_frecency)
}

/// Hazard (1): debouncer event handler is waiting on `shared_picker.write()`
/// while the caller joins it from under the same guard.
#[test]
fn stop_background_monitor_under_write_lock_does_not_deadlock_file_events() {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path().to_path_buf();

    for i in 0..4 {
        fs::write(base.join(format!("file_{i}.txt")), format!("seed {i}\n")).unwrap();
    }

    let (shared_picker, _shared_frecency) = make_watched_picker(&base);

    // Produce enough filesystem churn that the debouncer has events
    // queued and is likely mid-handler by the time we call stop.
    for round in 0..8 {
        for i in 0..4 {
            let path = base.join(format!("file_{i}.txt"));
            fs::write(&path, format!("edit {round}-{i}\n")).unwrap();
        }
    }

    // Give the kernel time to deliver events into the debouncer queue
    // (50 ms = default debouncer tick).
    std::thread::sleep(Duration::from_millis(60));

    let sp = shared_picker.clone();
    run_with_deadlock_guard(
        "stop_background_monitor() deadlocked under shared_picker.write() — \
         the debouncer thread is likely waiting on the same write lock \
         while we join it",
        Duration::from_secs(5),
        move || {
            let mut guard = sp.write().expect("write lock");
            if let Some(ref mut picker) = *guard {
                picker.stop_background_monitor();
            }
        },
    );
}

/// Hazard (2): owner thread holds the debouncer mutex while waiting
/// on `shared_picker.write()`, and `stop()` takes the debouncer mutex
/// under the caller's write guard.
#[test]
fn stop_background_monitor_under_write_lock_does_not_deadlock_new_dirs() {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path().to_path_buf();

    fs::write(base.join("seed.txt"), "seed\n").unwrap();

    let (shared_picker, _shared_frecency) = make_watched_picker(&base);

    // Create a burst of new subdirectories with files inside. On Linux
    // the watcher event thread sends each new dir to `watch_tx`, and
    // the owner thread processes them (taking the debouncer mutex +
    // `shared_picker.write()`). On macOS the owner thread still runs
    // `track_files_from_new_directories`, which takes the write lock.
    for d in 0..8 {
        let sub = base.join(format!("sub_{d}"));
        fs::create_dir(&sub).unwrap();
        for f in 0..4 {
            fs::write(sub.join(format!("f_{f}.txt")), format!("{d}-{f}\n")).unwrap();
        }
    }

    std::thread::sleep(Duration::from_millis(120));

    let sp = shared_picker.clone();
    run_with_deadlock_guard(
        "stop_background_monitor() deadlocked under shared_picker.write() — \
         the watcher owner thread is likely holding the debouncer mutex and \
         waiting on the same write lock while we try to take the debouncer \
         mutex to tear it down",
        Duration::from_secs(5),
        move || {
            let mut guard = sp.write().expect("write lock");
            if let Some(ref mut picker) = *guard {
                picker.stop_background_monitor();
            }
        },
    );
}
