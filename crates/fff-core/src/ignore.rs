use std::path::Path;

pub(crate) const NON_GIT_IGNORED_DIRS: &[&str] = &[
    "node_modules",
    "__pycache__",
    "venv",
    ".venv",
    // Rust (these are glob-only patterns for non_git_repo_overrides,
    // is_non_code_directory matches the "target" component separately)
    "target/debug",
    "target/release",
    "target/rust-analyzer",
    "target/criterion",
];

#[cfg(target_os = "macos")]
pub(crate) const PLATFORM_IGNORED_DIRS: &[&str] =
    &["Library/Application Support", "Library/Caches"];

#[cfg(target_os = "windows")]
pub(crate) const PLATFORM_IGNORED_DIRS: &[&str] = &[
    "bin/Debug",
    "bin/Release",
    "Program Files",
    "Program Files (x86)",
    "AppData/Local",
    "AppData/Roaming",
];

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(crate) const PLATFORM_IGNORED_DIRS: &[&str] = &[];

pub(crate) fn non_git_repo_overrides(base_path: &Path) -> Option<ignore::overrides::Override> {
    use ignore::overrides::OverrideBuilder;

    let mut builder = OverrideBuilder::new(base_path);
    for dir in NON_GIT_IGNORED_DIRS.iter().chain(PLATFORM_IGNORED_DIRS) {
        let pattern = format!("!**/{dir}/");
        if let Err(e) = builder.add(&pattern) {
            tracing::warn!("failed to add ignore pattern {pattern}: {e}");
        }
    }

    builder.build().ok()
}

pub(crate) fn is_non_code_directory(path: &Path) -> bool {
    let path_str = path.as_os_str().to_str().unwrap_or("");
    NON_GIT_IGNORED_DIRS
        .iter()
        .chain(PLATFORM_IGNORED_DIRS)
        .any(|&dir| {
            #[cfg(target_os = "windows")]
            let dir = dir.replace('/', std::path::MAIN_SEPARATOR_STR);
            #[cfg(target_os = "windows")]
            return path_str.contains(dir.as_str());

            #[cfg(not(target_os = "windows"))]
            path_str.contains(dir)
        })
}
