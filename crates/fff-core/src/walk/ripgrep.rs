use crate::ignore::non_git_repo_overrides;
use crate::types::FileItem;
use crate::walk::WalkOutput;
use crate::watch::is_git_file;
use ignore::WalkBuilder;
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[tracing::instrument(skip_all, name = "ripgrep walker", level = "info")]
pub(crate) fn walk_collect_files(
    base_path: &Path,
    is_git_repo: bool,
    follow_symlinks: bool,
    threads: usize,
    synced_files_count: &Arc<AtomicUsize>,
) -> crate::Result<WalkOutput> {
    let policy = crate::ignore::GitIgnorePolicy::discover(base_path);
    let mut walk_builder = WalkBuilder::new(base_path);
    walk_builder
        // this is a very important guard for the user opening ~/ or other root non-git dir
        .hidden(!is_git_repo)
        .git_ignore(true)
        // User and repository-wide excludes enter through GitIgnorePolicy so
        // scan and watcher rebuild from one precedence-ordered source list.
        .git_exclude(false)
        .git_global(false)
        .ignore(true)
        .follow_links(follow_symlinks)
        .threads(threads);

    for path in &policy.ignore_files {
        if let Some(error) = walk_builder.add_ignore(path) {
            tracing::warn!(?error, path = %path.display(), "Failed to load Git ignore policy source");
        }
    }

    if !is_git_repo && let Some(overrides) = non_git_repo_overrides(base_path) {
        walk_builder.overrides(overrides);
    }

    let walker = walk_builder.build_parallel();

    let pairs = parking_lot::Mutex::new(Vec::<(FileItem, String)>::new());
    walker.run(|| {
        let pairs = &pairs;
        let counter = Arc::clone(synced_files_count);
        let base_path = base_path.to_path_buf();

        Box::new(move |result| {
            let Ok(entry) = result else {
                return ignore::WalkState::Continue;
            };

            if entry.file_type().is_some_and(|ft| ft.is_file()) {
                let path = entry.path();

                // Ignore walkers sometimes surface files inside `.git/`
                // when the base is itself a git repo — skip them.
                if is_git_file(path) {
                    return ignore::WalkState::Continue;
                }

                let metadata = entry.metadata().ok();
                let (file_item, rel_path) =
                    FileItem::new_from_walk(path, &base_path, None, metadata.as_ref());

                pairs.lock().push((file_item, rel_path));
                counter.fetch_add(1, Ordering::Relaxed);
            }
            ignore::WalkState::Continue
        })
    });

    Ok(WalkOutput {
        pairs: pairs.into_inner(),
        ignore_rules: None,
        policy_sources: policy.sources,
    })
}
