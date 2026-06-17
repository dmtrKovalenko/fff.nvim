//! Fallback search backend that shells out to `rg`. Used when no git
//! worktree is detected (and thus no daemon index is available).

use std::process::Command;

use crate::app_ctx::AppCtx;
use crate::searcher::Search;
use crate::types::cli::Args;

/// [`Search`] backend that spawns `rg` as a subprocess.
pub struct RgSearcher<'a> {
    ctx: AppCtx<'a>,
}

impl<'a> RgSearcher<'a> {
    pub fn new(ctx: AppCtx<'a>) -> Self {
        Self { ctx }
    }

    /// Returns a fresh `rg` command, or errors if `rg` wasn't found at startup.
    fn rg(&self) -> Result<Command, Box<dyn std::error::Error>> {
        self.ctx.rg_command().ok_or_else(|| {
            "rg (ripgrep) not found — install from https://github.com/BurntSushi/ripgrep".into()
        })
    }

    /// Runs an `rg` command and maps its exit code to a match result.
    fn run(mut cmd: Command) -> Result<bool, Box<dyn std::error::Error>> {
        let status = cmd.status().map_err(|e| format!("failed to run rg: {e}"))?;
        match status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            Some(c) => Err(format!("rg exited with code {c}").into()),
            None => Err("rg terminated by signal".into()),
        }
    }

    /// Translates CLI flags into the corresponding `rg` arguments.
    fn apply_args(cmd: &mut Command, args: &Args) {
        args.case.apply_to_rg(cmd);

        if args.fixed_strings {
            cmd.arg("-F");
        }
        if let Some(n) = args.before_context {
            cmd.arg("-B").arg(n.to_string());
        }
        if let Some(n) = args.after_context {
            cmd.arg("-A").arg(n.to_string());
        }
        if let Some(n) = args.context {
            cmd.arg("-C").arg(n.to_string());
        }
        if let Some(n) = args.max_count {
            cmd.arg("-m").arg(n.to_string());
        }
        if let Some(fs) = args.max_filesize {
            cmd.arg("--max-filesize").arg(fs.to_string());
        }
        if args.trim {
            cmd.arg("--trim");
        }
        if args.line_number {
            cmd.arg("-n");
        }
        if args.no_line_number {
            cmd.arg("-N");
        }
        if args.column {
            cmd.arg("--column");
        }
        cmd.arg(format!("--color={}", args.color));
        if args.no_filename {
            cmd.arg("-I");
        }
        if args.heading {
            cmd.arg("--heading");
        }
        if args.no_heading {
            cmd.arg("--no-heading");
        }
        if args.count {
            cmd.arg("-c");
        }
        if args.files_with_matches {
            cmd.arg("-l");
        }
        if args.quiet {
            cmd.arg("-q");
        }
        if args.vimgrep {
            cmd.arg("--vimgrep");
        }
        if args.pretty {
            cmd.arg("-p");
        }
    }
}

impl Search for RgSearcher<'_> {
    #[tracing::instrument(level = "trace", skip(self))]
    fn grep(&self) -> Result<bool, Box<dyn std::error::Error>> {
        let pattern = self.ctx.args.pattern.as_deref().unwrap_or("");
        let mut cmd = self.rg()?;
        cmd.arg(pattern).arg(self.ctx.dir.as_ref());
        Self::apply_args(&mut cmd, self.ctx.args);
        Self::run(cmd)
    }

    #[tracing::instrument(level = "trace", skip(self))]
    fn files(&self) -> Result<bool, Box<dyn std::error::Error>> {
        let mut cmd = self.rg()?;
        cmd.arg("--files").arg(self.ctx.dir.as_ref());
        cmd.arg(format!("--color={}", self.ctx.args.color));
        Self::run(cmd)
    }
}
