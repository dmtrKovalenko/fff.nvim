use fff_search::file_picker::{FFFMode, FilePicker};
use fff_search::{
    DirSearchConfig, FilePickerOptions, FuzzySearchOptions, PaginationArgs, QueryParser,
    SharedFilePicker, SharedFrecency,
};
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};
use tempfile::TempDir;

fn make_watched_picker(base: &Path) -> (SharedFilePicker, SharedFrecency) {
    let shared_picker = SharedFilePicker::default();
    let shared_frecency = SharedFrecency::noop();

    FilePicker::new_with_shared_state(
        shared_picker.clone(),
        shared_frecency.clone(),
        FilePickerOptions {
            base_path: base.to_string_lossy().into_owned(),
            enable_mmap_cache: false,
            enable_content_indexing: false,
            mode: FFFMode::Neovim,
            watch: true,
            ..Default::default()
        },
    )
    .expect("FilePicker::new_with_shared_state");

    assert!(
        shared_picker.wait_for_scan(Duration::from_secs(30)),
        "initial scan did not complete"
    );
    assert!(
        shared_picker.wait_for_watcher(Duration::from_secs(30)),
        "watcher did not install"
    );
    // macOS FSEvents streams need a beat before they deliver reliably
    std::thread::sleep(Duration::from_millis(300));

    (shared_picker, shared_frecency)
}

fn search_dirs(picker: &SharedFilePicker, query: &str) -> Vec<String> {
    let guard = picker.read().expect("picker read lock");
    let p = guard.as_ref().expect("picker initialized");
    let parser = QueryParser::new(DirSearchConfig);
    let parsed = parser.parse(query);
    let results = p.fuzzy_search_directories(
        &parsed,
        FuzzySearchOptions {
            pagination: PaginationArgs {
                offset: 0,
                limit: 100,
            },
            ..Default::default()
        },
    );
    results.items.iter().map(|d| d.relative_path(p)).collect()
}

fn wait_until<F: Fn() -> bool>(cond: F, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    cond()
}

#[test]
fn removed_directory_disappears_from_dir_search() {
    let tmp = TempDir::new().unwrap();
    let base = fff_search::path_utils::canonicalize(tmp.path()).unwrap();
    fs::create_dir_all(base.join("doomed/nested")).unwrap();
    fs::write(base.join("doomed/a.rs"), "x").unwrap();
    fs::write(base.join("doomed/nested/b.rs"), "x").unwrap();
    fs::write(base.join("keep.rs"), "x").unwrap();

    let (picker, _frecency) = make_watched_picker(&base);
    assert!(
        search_dirs(&picker, "doomed")
            .iter()
            .any(|d| d.starts_with("doomed")),
        "sanity: dir indexed after scan"
    );

    fs::remove_dir_all(base.join("doomed")).unwrap();

    assert!(
        wait_until(
            || !search_dirs(&picker, "doomed")
                .iter()
                .any(|d| d.starts_with("doomed")),
            Duration::from_secs(10)
        ),
        "removed dir must disappear from dir search, got: {:?}",
        search_dirs(&picker, "doomed")
    );
}

#[test]
fn moved_out_directory_disappears_from_dir_search() {
    let tmp = TempDir::new().unwrap();
    let trash = TempDir::new().unwrap();
    let base = fff_search::path_utils::canonicalize(tmp.path()).unwrap();
    fs::create_dir_all(base.join("doomed/nested")).unwrap();
    fs::write(base.join("doomed/a.rs"), "x").unwrap();
    fs::write(base.join("doomed/nested/b.rs"), "x").unwrap();
    fs::write(base.join("keep.rs"), "x").unwrap();

    let (picker, _frecency) = make_watched_picker(&base);
    assert!(
        search_dirs(&picker, "doomed")
            .iter()
            .any(|d| d.starts_with("doomed")),
        "sanity: dir indexed after scan"
    );

    fs::rename(base.join("doomed"), trash.path().join("doomed")).unwrap();

    assert!(
        wait_until(
            || !search_dirs(&picker, "doomed")
                .iter()
                .any(|d| d.starts_with("doomed")),
            Duration::from_secs(10)
        ),
        "moved-out dir must disappear from dir search, got: {:?}",
        search_dirs(&picker, "doomed")
    );
}

#[test]
fn moved_in_directory_appears_in_dir_search() {
    let tmp = TempDir::new().unwrap();
    let staging = TempDir::new().unwrap();
    let base = fff_search::path_utils::canonicalize(tmp.path()).unwrap();
    fs::write(base.join("keep.rs"), "x").unwrap();

    let incoming = staging.path().join("arrived");
    fs::create_dir_all(incoming.join("nested")).unwrap();
    fs::write(incoming.join("a.rs"), "x").unwrap();
    fs::write(incoming.join("nested/b.rs"), "x").unwrap();

    let (picker, _frecency) = make_watched_picker(&base);
    assert!(search_dirs(&picker, "arrived").is_empty(), "sanity");

    fs::rename(&incoming, base.join("arrived")).unwrap();

    assert!(
        wait_until(
            || {
                let dirs = search_dirs(&picker, "arrived");
                dirs.iter().any(|d| d.starts_with("arrived"))
            },
            Duration::from_secs(10)
        ),
        "moved-in dir must appear in dir search, got: {:?}",
        search_dirs(&picker, "arrived")
    );
}

#[test]
fn new_file_in_new_directory_surfaces_the_dir() {
    let tmp = TempDir::new().unwrap();
    let base = fff_search::path_utils::canonicalize(tmp.path()).unwrap();
    fs::write(base.join("keep.rs"), "x").unwrap();

    let (picker, _frecency) = make_watched_picker(&base);
    assert!(search_dirs(&picker, "brandnew").is_empty(), "sanity");

    fs::create_dir_all(base.join("brandnew")).unwrap();
    fs::write(base.join("brandnew/file.rs"), "x").unwrap();

    assert!(
        wait_until(
            || search_dirs(&picker, "brandnew")
                .iter()
                .any(|d| d.starts_with("brandnew")),
            Duration::from_secs(10)
        ),
        "new dir must appear in dir search, got: {:?}",
        search_dirs(&picker, "brandnew")
    );
}

#[test]
fn deleting_last_file_keeps_directory_visible() {
    let tmp = TempDir::new().unwrap();
    let base = fff_search::path_utils::canonicalize(tmp.path()).unwrap();
    fs::create_dir_all(base.join("lonely")).unwrap();
    fs::write(base.join("lonely/only.rs"), "x").unwrap();
    fs::write(base.join("keep.rs"), "x").unwrap();

    let (picker, _frecency) = make_watched_picker(&base);

    // the file goes away but the directory itself still exists on disk
    fs::remove_file(base.join("lonely/only.rs")).unwrap();

    assert!(
        wait_until(
            || {
                let guard = picker.read().unwrap();
                let p = guard.as_ref().unwrap();
                p.get_file_by_path(base.join("lonely/only.rs"))
                    .is_none_or(|f| f.is_deleted())
            },
            Duration::from_secs(10)
        ),
        "file removal must be applied"
    );
    assert!(
        search_dirs(&picker, "lonely")
            .iter()
            .any(|d| d.starts_with("lonely")),
        "dir still exists on disk and must stay searchable"
    );
}

#[test]
fn recreated_directory_reappears_in_dir_search() {
    let tmp = TempDir::new().unwrap();
    let base = fff_search::path_utils::canonicalize(tmp.path()).unwrap();
    fs::create_dir_all(base.join("phoenix")).unwrap();
    fs::write(base.join("phoenix/a.rs"), "x").unwrap();
    fs::write(base.join("keep.rs"), "x").unwrap();

    let (picker, _frecency) = make_watched_picker(&base);

    fs::remove_dir_all(base.join("phoenix")).unwrap();
    assert!(
        wait_until(
            || !search_dirs(&picker, "phoenix")
                .iter()
                .any(|d| d.starts_with("phoenix")),
            Duration::from_secs(10)
        ),
        "dir must disappear after removal"
    );

    fs::create_dir_all(base.join("phoenix")).unwrap();
    fs::write(base.join("phoenix/a.rs"), "x").unwrap();

    assert!(
        wait_until(
            || search_dirs(&picker, "phoenix")
                .iter()
                .any(|d| d.starts_with("phoenix")),
            Duration::from_secs(10)
        ),
        "recreated dir must reappear in dir search, got: {:?}",
        search_dirs(&picker, "phoenix")
    );
}

/// Regression for #725: a dir that is EMPTY at scan time must be indexed —
/// searchable in dir search and watched so later file creations are seen.
#[test]
fn empty_directory_at_scan_is_searchable_and_watched() {
    let tmp = TempDir::new().unwrap();
    let base = fff_search::path_utils::canonicalize(tmp.path()).unwrap();
    fs::create_dir_all(base.join("commands")).unwrap();
    fs::write(base.join("keep.rs"), "x").unwrap();

    let (picker, _frecency) = make_watched_picker(&base);

    assert!(
        search_dirs(&picker, "commands")
            .iter()
            .any(|d| d.starts_with("commands")),
        "empty dir must be searchable right after the scan, got: {:?}",
        search_dirs(&picker, "commands")
    );

    // The empty dir must reuse its scan-built DirItem when a file lands in it
    // and the watcher must have registered a watch on it (the #725 repro).
    fs::write(base.join("commands/review.md"), "# review").unwrap();
    assert!(
        wait_until(
            || {
                let guard = picker.read().unwrap();
                let p = guard.as_ref().unwrap();
                p.get_file_by_path(base.join("commands/review.md"))
                    .is_some()
            },
            Duration::from_secs(10)
        ),
        "file created in a scan-time-empty dir must be indexed"
    );

    let guard = picker.read().unwrap();
    let p = guard.as_ref().unwrap();
    let commands_dirs = p
        .get_dirs()
        .iter()
        .filter(|d| d.relative_path(p).starts_with("commands"))
        .count();
    assert_eq!(commands_dirs, 1, "no duplicate DirItem for the empty dir");
}
