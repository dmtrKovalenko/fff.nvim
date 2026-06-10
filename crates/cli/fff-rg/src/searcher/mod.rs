//! Search backend selection. Inside a git worktree the daemon is used
//! (indexed, persistent); outside it we fall back to a plain `rg` subprocess.

mod fffd;
mod rg;
mod search;

pub use search::Search;

use crate::app_ctx::AppCtx;

/// Dispatches searches to either the daemon or a direct `rg` invocation.
pub enum Searcher<'a> {
    /// Direct `rg` subprocess — used outside git worktrees.
    Rg(rg::RgSearcher<'a>),
    /// IPC to the `fff-daemon` — used inside git worktrees.
    Daemon(fffd::DaemonSearcher<'a>),
}

impl<'a> Searcher<'a> {
    /// Picks the right backend based on whether a git root was discovered.
    pub fn new(ctx: AppCtx<'a>) -> Self {
        if ctx.git_root.is_some() {
            Self::Daemon(fffd::DaemonSearcher::new(ctx))
        } else {
            Self::Rg(rg::RgSearcher::new(ctx))
        }
    }
}

impl Search for Searcher<'_> {
    fn grep(&self) -> Result<bool, Box<dyn std::error::Error>> {
        match self {
            Self::Rg(s) => s.grep(),
            Self::Daemon(s) => s.grep(),
        }
    }

    fn files(&self) -> Result<bool, Box<dyn std::error::Error>> {
        match self {
            Self::Rg(s) => s.files(),
            Self::Daemon(s) => s.files(),
        }
    }
}

