//! Regression test: indexing a sparse-checked-out git repository must not
//! crash and must only surface paths that are actually materialized on
//! disk. libgit2 happily reports statuses for paths that were excluded by
//! `core.sparseCheckout`; `update_git_statuses` already silently skips
//! those (see `file_picker.rs::1346`), this test pins that contract.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fff_search::file_picker::FilePicker;
use fff_search::{
    FilePickerOptions, FuzzySearchOptions, PaginationArgs, QueryParser, SharedFilePicker,
    SharedFrecency,
};
use git2::Repository;
use tempfile::TempDir;
use tracing_subscriber::fmt::MakeWriter;

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("git binary must be installed to run this test");
    assert!(
        out.status.success(),
        "git {args:?} failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

#[derive(Clone, Default)]
struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

impl CapturedLogs {
    fn lines(&self) -> String {
        let buf = self.0.lock().unwrap();
        String::from_utf8_lossy(&buf).to_string()
    }
}

impl<'a> MakeWriter<'a> for CapturedLogs {
    type Writer = CapturedLogsWriter;
    fn make_writer(&'a self) -> Self::Writer {
        CapturedLogsWriter(Arc::clone(&self.0))
    }
}

struct CapturedLogsWriter(Arc<Mutex<Vec<u8>>>);

impl Write for CapturedLogsWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn fuzzy_search_paths(picker: &FilePicker, query: &str) -> Vec<String> {
    let parser = QueryParser::default();
    let parsed = parser.parse(query);
    let result = picker.fuzzy_search(
        &parsed,
        None,
        FuzzySearchOptions {
            max_threads: 1,
            pagination: PaginationArgs {
                offset: 0,
                limit: 200,
            },
            ..Default::default()
        },
    );
    result
        .items
        .iter()
        .map(|f| f.relative_path(picker))
        .collect()
}

/// End-to-end: build a "remote" repo with files in two top-level dirs,
/// clone it with a cone-mode sparse-checkout that materializes only one
/// dir, then index the worktree both via the synchronous `collect_files`
/// path and the async `new_with_shared_state` path. The picker must:
///   * see only the materialized files,
///   * NOT panic when libgit2 hands it statuses for non-materialized paths,
///   * surface no `ERROR`/`WARN` log lines from fff core,
///   * yield correct fuzzy-search results,
///   * skip non-materialized paths via the documented debug message in
///     `update_git_statuses`.
#[test]
fn sparse_checkout_indexes_only_materialized_files() {
    let logs = CapturedLogs::default();
    let _guard = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_ansi(false)
            .with_writer(logs.clone())
            .finish(),
    );

    let upstream_tmp = TempDir::new().unwrap();
    let upstream = upstream_tmp.path();

    git(upstream, &["init", "-b", "main"]);
    fs::create_dir_all(upstream.join("included/sub")).unwrap();
    fs::create_dir_all(upstream.join("excluded/deep")).unwrap();
    fs::write(upstream.join("README.md"), "root readme\n").unwrap();
    fs::write(upstream.join("included/in_a.rs"), "// a\n").unwrap();
    fs::write(upstream.join("included/sub/in_b.rs"), "// b\n").unwrap();
    fs::write(upstream.join("excluded/ex_a.rs"), "// ea\n").unwrap();
    fs::write(upstream.join("excluded/deep/ex_b.rs"), "// eb\n").unwrap();
    git(upstream, &["add", "-A"]);
    git(upstream, &["commit", "-m", "seed", "--no-gpg-sign"]);

    let work_tmp = TempDir::new().unwrap();
    let work = work_tmp.path();
    git(
        work,
        &[
            "clone",
            "--no-local",
            "--no-checkout",
            upstream.to_str().unwrap(),
            ".",
        ],
    );
    git(work, &["sparse-checkout", "init", "--cone"]);
    git(work, &["sparse-checkout", "set", "included"]);
    git(work, &["checkout", "main"]);

    // Sanity: only the included dir + top-level files materialized.
    assert!(work.join("README.md").is_file());
    assert!(work.join("included/in_a.rs").is_file());
    assert!(work.join("included/sub/in_b.rs").is_file());
    assert!(!work.join("excluded").exists());

    // Sanity: libgit2's status output DOES contain the sparse-excluded
    // paths in the index — proving the "path not in index" branch in
    // `update_git_statuses` actually has work to do.
    {
        let repo = Repository::open(work).unwrap();
        let mut opts = git2::StatusOptions::new();
        opts.include_untracked(true)
            .include_unmodified(true)
            .recurse_untracked_dirs(true)
            .exclude_submodules(false);
        let statuses = repo.statuses(Some(&mut opts)).unwrap();
        let entries: Vec<String> = statuses
            .iter()
            .filter_map(|e| e.path().map(|p| p.to_owned()))
            .collect();
        assert!(
            entries.iter().any(|p| p == "excluded/ex_a.rs"),
            "libgit2 must surface sparse-excluded paths so we can prove the \
             picker handles them; got {:?}",
            entries
        );
    }

    // ---- Path 1: synchronous `collect_files`. -----------------------
    let mut picker = FilePicker::new(FilePickerOptions {
        base_path: work.to_string_lossy().to_string(),
        enable_mmap_cache: false,
        enable_content_indexing: false,
        watch: false,
        ..Default::default()
    })
    .expect("failed to create FilePicker");
    picker
        .collect_files()
        .expect("collect_files must not panic on a sparse-checked repo");

    let indexed: Vec<String> = picker
        .get_files()
        .iter()
        .map(|f| f.relative_path(&picker))
        .collect();

    assert!(
        indexed.iter().any(|p| p.ends_with("in_a.rs")),
        "expected to find included/in_a.rs in index, got {:?}",
        indexed
    );
    assert!(
        indexed.iter().any(|p| p.ends_with("in_b.rs")),
        "expected to find included/sub/in_b.rs in index, got {:?}",
        indexed
    );
    assert!(
        indexed
            .iter()
            .all(|p| !p.contains("ex_a.rs") && !p.contains("ex_b.rs")),
        "sparse-excluded files leaked into the index: {:?}",
        indexed
    );

    let hits = fuzzy_search_paths(&picker, "in_b");
    assert!(
        hits.iter().any(|p| p.ends_with("in_b.rs")),
        "fuzzy search must surface materialized files, got {:?}",
        hits
    );

    drop(picker);

    // ---- Path 2: async `new_with_shared_state` + refresh_git_status. ---
    // This goes through the orchestrated scan + git-apply pipeline
    // (`scan.rs::apply_git_status_and_frecency` followed by an explicit
    // `refresh_git_status` that hits the `update_git_statuses` branch
    // which is the one that has to silently drop sparse-excluded paths).
    let shared_picker = SharedFilePicker::default();
    let shared_frecency = SharedFrecency::default();
    FilePicker::new_with_shared_state(
        shared_picker.clone(),
        shared_frecency.clone(),
        FilePickerOptions {
            base_path: work.to_string_lossy().to_string(),
            enable_mmap_cache: false,
            enable_content_indexing: false,
            watch: false,
            ..Default::default()
        },
    )
    .expect("new_with_shared_state must succeed on a sparse-checked repo");

    assert!(
        shared_picker.wait_for_scan(Duration::from_secs(15)),
        "initial scan timed out"
    );

    let applied = shared_picker
        .refresh_git_status(&shared_frecency)
        .expect("refresh_git_status must not error on a sparse-checked repo");
    assert!(
        applied >= 3,
        "expected statuses for materialized files, got {applied}"
    );

    {
        let guard = shared_picker.read().unwrap();
        let picker = guard.as_ref().unwrap();
        for f in picker.get_files() {
            let rel = f.relative_path(picker);
            assert!(
                !rel.contains("ex_a.rs") && !rel.contains("ex_b.rs"),
                "sparse-excluded file leaked through async path: {rel}"
            );
        }
    }

    // ---- Log assertions. --------------------------------------------
    let captured = logs.lines();

    assert!(
        captured.contains("Git status for path not in index, skipping"),
        "expected the documented debug skip-message for sparse-excluded \
         paths to be emitted at least once, full log was:\n{captured}"
    );

    let unexpected: Vec<&str> = captured
        .lines()
        .filter(|l| l.contains(" ERROR ") || l.contains(" WARN "))
        // The walker emits a warning when given a non-git base path; we
        // are *inside* a git repo so it should not trip, but keep the
        // filter narrow regardless.
        .filter(|l| !l.contains("No git repository found"))
        .collect();
    assert!(
        unexpected.is_empty(),
        "no ERROR/WARN lines expected on a sparse-checked repo, got:\n{}",
        unexpected.join("\n")
    );
}
