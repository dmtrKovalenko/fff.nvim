#![cfg(rescan_stats)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use fff_search::file_picker::{FFFMode, FilePicker};
use fff_search::{FilePickerOptions, RescanStats, SharedFilePicker, SharedFrecency};
use tempfile::TempDir;

const SETTLE: Duration = Duration::from_millis(600);

#[test]
fn saving_source_files_does_not_rescan() {
    let repo = WatchedRepo::new(|base| {
        write(base, ".gitignore", "target/\n");
        for i in 0..20 {
            write(base, &format!("src/mod{i}.rs"), "pub fn f() {}");
        }
    });

    for round in 0..10 {
        for i in 0..20 {
            repo.write(
                &format!("src/mod{i}.rs"),
                &format!("pub fn f() {{ let _ = {round}; }}"),
            );
        }
        repo.settle();
    }

    repo.assert_quiet("200 file saves");
}

#[test]
fn build_output_in_ignored_directories_does_not_rescan() {
    let repo = WatchedRepo::new(|base| {
        write(base, ".gitignore", "target/\nnode_modules/\ndist/\n");
        write(base, "src/main.rs", "fn main() {}");
    });

    for round in 0..4 {
        for i in 0..150 {
            repo.write(&format!("target/debug/deps/unit-{round}-{i}.o"), "binary");
            repo.write(&format!("dist/chunk-{round}-{i}.js"), "bundled");
        }
        repo.settle();
    }

    repo.assert_quiet("1200 build artifacts written into ignored directories");
}

#[test]
fn adding_source_files_and_directories_does_not_rescan() {
    let repo = WatchedRepo::new(|base| {
        write(base, ".gitignore", "target/\n");
        write(base, "src/main.rs", "fn main() {}");
    });

    for i in 0..40 {
        repo.write(&format!("src/feature{i}/mod.rs"), "pub mod inner;");
        repo.write(&format!("src/feature{i}/inner.rs"), "pub fn go() {}");
    }
    repo.settle();

    assert!(
        repo.wait_indexed("src/feature39/inner.rs"),
        "watcher must index files in newly created directories"
    );
    repo.assert_quiet("40 new directories with 80 files");
}

#[test]
fn recreating_generated_files_does_not_rescan() {
    let repo = WatchedRepo::new(|base| {
        write(base, ".gitignore", "target/\n");
        write(base, "src/main.rs", "fn main() {}");
    });

    // Recreated paths must reuse their overflow slots.
    for round in 0..12 {
        for i in 0..40 {
            repo.write(&format!("src/generated/api{i}.rs"), "pub struct A;");
        }
        repo.settle();
        for i in 0..40 {
            repo.remove(&format!("src/generated/api{i}.rs"));
        }
        repo.settle();
        assert!(
            repo.overflow_len() <= 64,
            "round {round}: regenerating the same paths grew the overflow region to {}",
            repo.overflow_len()
        );
    }

    repo.assert_quiet("12 codegen cycles over 40 stable paths");
}

#[test]
fn git_workflow_does_not_rescan() {
    let repo = WatchedRepo::new(|base| {
        write(base, ".gitignore", "target/\n");
        write(base, "src/main.rs", "fn main() {}");
        write(base, "src/lib.rs", "pub mod thing;");
        git(base, &["init", "-b", "main"]);
        git(base, &["add", "-A"]);
        git(base, &["commit", "-m", "initial"]);
    });

    repo.write("src/main.rs", "fn main() { println!(\"hi\"); }");
    repo.settle();
    repo.git(&["add", "-A"]);
    repo.settle();
    repo.git(&["commit", "-m", "second"]);
    repo.settle();
    repo.git(&["checkout", "-b", "feature"]);
    repo.settle();
    repo.write("src/feature.rs", "pub fn feature() {}");
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-m", "feature"]);
    repo.settle();
    repo.git(&["checkout", "main"]);
    repo.settle();
    repo.git(&["merge", "feature"]);
    repo.settle();

    repo.assert_quiet("a commit / branch / merge cycle");
}

#[test]
fn reading_files_does_not_rescan() {
    let repo = WatchedRepo::new(|base| {
        write(base, ".gitignore", "target/\n");
        for i in 0..50 {
            write(base, &format!("src/mod{i}.rs"), "pub fn f() {}");
        }
    });

    // Preview rendering and grep open every file in the result list. Reacting
    // to those reads would make the picker rescan while the user scrolls.
    for _ in 0..5 {
        for i in 0..50 {
            let _ = std::fs::read(repo.path(&format!("src/mod{i}.rs"))).unwrap();
        }
    }
    repo.settle();

    repo.assert_quiet("reading every indexed file");
}

#[test]
fn npm_install_style_churn_does_not_rescan() {
    let repo = WatchedRepo::new(|base| {
        write(base, ".gitignore", "node_modules/\n");
        write(base, "src/index.ts", "export const a = 1;");
    });

    for pkg in 0..100 {
        repo.write(&format!("node_modules/pkg{pkg}/package.json"), "{}");
        repo.write(
            &format!("node_modules/pkg{pkg}/index.js"),
            "module.exports={}",
        );
        repo.write(&format!("node_modules/pkg{pkg}/.gitignore"), "dist\n");
    }
    repo.settle();
    repo.settle();

    repo.assert_quiet("an npm install into an ignored node_modules");
}

#[test]
fn a_churning_root_is_capped_at_one_rescan_per_cooldown() {
    let repo = WatchedRepo::new(|base| {
        write(base, "src/main.rs", "fn main() {}");
    });

    // Root ignore changes force watcher rescan requests.
    for round in 0..25 {
        repo.write(".gitignore", &format!("target/\n# round {round}\n"));
        std::thread::sleep(Duration::from_millis(120));
    }
    repo.settle();

    let stats = repo.rescans();
    assert!(
        stats.total <= 1,
        "a churning root must not exceed one walk per cooldown, got {stats}"
    );
    assert!(
        stats.throttled > 0,
        "the suppressed triggers must be recorded, got {stats}"
    );
}

struct WatchedRepo {
    base: PathBuf,
    picker: SharedFilePicker,
    _frecency: SharedFrecency,
    _tmp: TempDir,
}

impl WatchedRepo {
    fn new(setup: impl FnOnce(&Path)) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = fff_search::path_utils::canonicalize(tmp.path()).unwrap();
        setup(&base);

        let picker = SharedFilePicker::default();
        let frecency = SharedFrecency::noop();
        FilePicker::new_with_shared_state(
            picker.clone(),
            frecency.clone(),
            FilePickerOptions {
                base_path: base.to_string_lossy().into_owned(),
                enable_mmap_cache: false,
                mode: FFFMode::Neovim,
                watch: true,
                ..Default::default()
            },
        )
        .expect("failed to create file picker");

        assert!(
            picker.wait_for_scan(Duration::from_secs(60)),
            "timed out waiting for the initial scan"
        );
        assert!(
            picker.wait_for_watcher(Duration::from_secs(60)),
            "timed out waiting for the watcher"
        );

        let repo = Self {
            base,
            picker,
            _frecency: frecency,
            _tmp: tmp,
        };
        repo.settle();
        repo.picker.reset_rescan_stats();
        repo
    }

    fn settle(&self) {
        std::thread::sleep(SETTLE);
        assert!(
            self.picker
                .wait_for_indexing_complete(Duration::from_secs(60)),
            "timed out waiting for background indexing to finish"
        );
    }

    fn assert_quiet(&self, workload: &str) {
        let stats = self.rescans();
        assert_eq!(
            stats.watcher_triggered(),
            0,
            "{workload} must be absorbed incrementally, but the watcher fell back to {stats}"
        );
    }

    fn rescans(&self) -> RescanStats {
        self.picker.rescan_stats()
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.base.join(rel)
    }

    fn write(&self, rel: &str, contents: &str) {
        write(&self.base, rel, contents);
    }

    fn remove(&self, rel: &str) {
        std::fs::remove_file(self.path(rel)).unwrap();
    }

    fn git(&self, args: &[&str]) {
        git(&self.base, args);
    }

    fn wait_indexed(&self, rel: &str) -> bool {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while std::time::Instant::now() < deadline {
            if self.is_indexed(rel) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    fn is_indexed(&self, rel: &str) -> bool {
        let guard = self.picker.read().unwrap();
        guard
            .as_ref()
            .and_then(|p| p.get_file_by_path(self.path(rel)))
            .is_some_and(|file| !file.is_deleted())
    }

    fn overflow_len(&self) -> usize {
        let guard = self.picker.read().unwrap();
        guard
            .as_ref()
            .map(|p| p.get_overflow_files().len())
            .unwrap_or(0)
    }
}

impl Drop for WatchedRepo {
    fn drop(&mut self) {
        // Stop the watcher before the tree disappears, otherwise a late batch
        // races the tempdir removal.
        if let Ok(mut guard) = self.picker.write() {
            guard.take();
        }
    }
}

fn write(base: &Path, rel: &str, contents: &str) {
    let path = base.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@test.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@test.com")
        .output()
        .unwrap_or_else(|e| panic!("git {args:?} failed to spawn: {e}"));

    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
