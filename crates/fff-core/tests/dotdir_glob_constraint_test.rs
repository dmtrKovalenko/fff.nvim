//! Regression test for https://github.com/dmtrKovalenko/fff/issues/828
//!
//! Glob path constraints must reach indexed files under dot-directories.
//! Both glob backends have to agree: zlob needs `ZLOB_PERIOD`, globset has no
//! leading-dot rule at all.

use std::fs;
use std::path::Path;
use tempfile::TempDir;

use fff_search::file_picker::FilePicker;
use fff_search::{FilePickerOptions, FuzzySearchOptions, PaginationArgs, QueryParser};

fn create_picker(base: &Path, specs: &[&str]) -> FilePicker {
    for rel in specs {
        let full_path = base.join(rel);
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&full_path, "{}\n").unwrap();
    }
    // hidden entries are only walked inside a git repo (`walk::hidden(!is_git_repo)`)
    std::process::Command::new("git")
        .arg("init")
        .current_dir(base)
        .output()
        .expect("git init failed");

    let mut picker = FilePicker::new(FilePickerOptions {
        base_path: base.to_string_lossy().to_string(),
        enable_mmap_cache: false,
        watch: false,
        ..Default::default()
    })
    .expect("failed to create FilePicker");
    picker.collect_files().expect("failed to collect files");
    picker
}

fn search(picker: &FilePicker, query: &str) -> Vec<String> {
    let parser = QueryParser::default();
    let parsed = parser.parse(query);
    picker
        .fuzzy_search(
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
        )
        .items
        .iter()
        .map(|f| f.relative_path(picker))
        .collect()
}

#[test]
fn glob_constraints_match_under_dot_directories() {
    let tmp = TempDir::new().unwrap();
    let picker = create_picker(
        tmp.path(),
        &[
            "home/.pi/agent/settings.json",
            "home/pi/agent/settings.json",
            "home/nested/herdr-plugin.toml",
        ],
    );

    let dot = "home/.pi/agent/settings.json";
    let indexed = search(&picker, "");
    assert!(
        indexed.iter().any(|p| p == dot),
        "precondition: {dot} must be indexed, got {indexed:?}"
    );

    for pattern in [
        "**/settings.json",
        "home/.pi/**/settings.json",
        "home/.pi/**",
        "**/.pi/**",
        "*/.pi/**/*.json",
    ] {
        let results = search(&picker, pattern);
        assert!(
            results.iter().any(|p| p == dot),
            "glob {pattern:?} must match {dot}, got {results:?}"
        );
    }

    // control: a wildcard must still not leak past its scope
    let scoped = search(&picker, "home/pi/**");
    assert_eq!(scoped, vec!["home/pi/agent/settings.json".to_string()]);
}
