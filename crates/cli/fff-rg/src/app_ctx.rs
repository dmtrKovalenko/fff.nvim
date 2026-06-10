//! Startup context for `fff-rg`. Captures CLI args, terminal state, working
//! directory, and git root once — then threads through the searcher pipeline
//! as `&AppCtx` so nothing is recomputed or cloned per query.

use std::borrow::Cow;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::Command;

use crate::types::cli::Args;

/// Runtime context computed once at startup.
pub struct AppCtx<'a> {
    /// Parsed CLI arguments.
    pub args: &'a Args,
    /// Whether stdout is connected to a terminal (controls color/heading defaults).
    pub is_tty: bool,
    /// Root search directory — borrowed from `args.paths[0]` or owned from `cwd`.
    pub dir: Cow<'a, str>,
    /// Git worktree root as a string, if `dir` is inside a repository.
    pub git_root: Option<String>,
    /// Path to the `fff-daemon` binary — `$FFF_DAEMON` if set, else `fff-daemon` from `$PATH`.
    pub daemon_bin: PathBuf,
    /// Resolved absolute path to `rg`, if found on `$PATH` at startup.
    pub rg_bin: Option<PathBuf>,
}

impl<'a> AppCtx<'a> {
    /// Probes the environment once: resolves the search directory, discovers
    /// the git root, and snapshots the terminal state.
    #[tracing::instrument(level = "trace", skip_all)]
    pub fn new(args: &'a Args) -> Self {
        let is_tty = std::io::stdout().is_terminal();
        let dir: Cow<'a, str> = match args.paths.first() {
            Some(p) => Cow::Borrowed(p.as_str()),
            None => Cow::Owned(
                std::env::current_dir()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
            ),
        };
        // workdir() borrows from Repository — must convert before drop.
        let git_root = git2::Repository::discover(dir.as_ref())
            .ok()
            .and_then(|repo| repo.workdir().map(|p| p.to_string_lossy().into_owned()));
        let daemon_bin = std::env::var_os("FFF_DAEMON")
            .map_or_else(|| PathBuf::from("fff-daemon"), PathBuf::from);
        let rg_bin = which::which("rg").ok();
        Self { args, is_tty, dir, git_root, daemon_bin, rg_bin }
    }

    /// Returns a `Command` for `rg`, or `None` if it wasn't found at startup.
    pub fn rg_command(&self) -> Option<Command> {
        self.rg_bin.as_ref().map(Command::new)
    }
}
