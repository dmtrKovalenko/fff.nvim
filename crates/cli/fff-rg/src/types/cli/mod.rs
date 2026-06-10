//! CLI argument types for `fff-rg`. Parsed by clap at startup and threaded
//! into [`AppCtx`](crate::app_ctx::AppCtx) for the rest of the process.

mod case_mode;

pub use case_mode::CaseModeArgs;

use bytesize::ByteSize;
use clap::{Parser, ValueEnum};

/// When to emit ANSI color codes in output.
#[derive(Clone, Copy, ValueEnum)]
pub enum ColorMode {
    /// Force color on, using platform-native sequences.
    Always,
    /// Force color on, always using ANSI escape codes.
    Ansi,
    /// Disable color entirely.
    Never,
    /// Color when stdout is a terminal, plain otherwise.
    Auto,
}

impl std::fmt::Display for ColorMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Always => "always",
            Self::Ansi => "ansi",
            Self::Never => "never",
            Self::Auto => "auto",
        })
    }
}

#[derive(Parser)]
#[command(
    name = "fff-rg",
    about = "FFF — daemon-accelerated file finder and grep",
    after_help = "Falls back to rg when searching outside a git repository."
)]
/// Mirrors a subset of `rg` flags so `fff-rg` is a drop-in replacement.
#[allow(clippy::struct_excessive_bools)]
pub struct Args {
    /// Search pattern (regex by default, literal with -F)
    pub pattern: Option<String>,

    /// Paths to search (default: current directory)
    pub paths: Vec<String>,

    /// Case sensitivity flags (`-i`, `-s`, `-S`).
    #[command(flatten)]
    pub case: CaseModeArgs,

    /// Treat pattern as a literal string, not a regex.
    #[arg(short = 'F', long = "fixed-strings")]
    pub fixed_strings: bool,

    /// Lines of context after each match.
    #[arg(short = 'A', long = "after-context", value_name = "NUM")]
    pub(crate) after_context: Option<u32>,

    /// Lines of context before each match.
    #[arg(short = 'B', long = "before-context", value_name = "NUM")]
    pub(crate) before_context: Option<u32>,

    /// Lines of context before and after each match.
    #[arg(short = 'C', long = "context", value_name = "NUM")]
    pub(crate) context: Option<u32>,

    /// Max matches per file.
    #[arg(short = 'm', long = "max-count", value_name = "NUM")]
    pub(crate) max_count: Option<u32>,

    /// Skip files larger than this size.
    #[arg(long = "max-filesize", value_name = "SIZE")]
    pub(crate) max_filesize: Option<ByteSize>,

    /// Strip leading whitespace from matched lines.
    #[arg(long)]
    pub(crate) trim: bool,

    /// Prefix each match with its line number.
    #[arg(short = 'n', long = "line-number")]
    pub(crate) line_number: bool,

    /// Suppress line numbers.
    #[arg(short = 'N', long = "no-line-number")]
    pub(crate) no_line_number: bool,

    /// Show the byte-column offset of each match.
    #[arg(long)]
    pub(crate) column: bool,

    /// When to use color in output.
    #[arg(long, value_enum, value_name = "WHEN", default_value = "auto")]
    pub(crate) color: ColorMode,

    /// Print the filename for each match.
    #[arg(short = 'H', long = "with-filename")]
    pub(crate) with_filename: bool,

    /// Suppress filenames in output.
    #[arg(short = 'I', long = "no-filename")]
    pub(crate) no_filename: bool,

    /// Group matches under filename headers.
    #[arg(long)]
    pub(crate) heading: bool,

    /// Print each match on its own line with the filename prefix.
    #[arg(long = "no-heading")]
    pub(crate) no_heading: bool,

    /// Print only a count of matching lines per file.
    #[arg(short = 'c', long)]
    pub(crate) count: bool,

    /// Print only filenames that contain matches.
    #[arg(short = 'l', long = "files-with-matches")]
    pub(crate) files_with_matches: bool,

    /// Suppress all output; exit status indicates match/no-match.
    #[arg(short = 'q', long)]
    pub(crate) quiet: bool,

    /// Output in `file:line:col:text` format for editor integration.
    #[arg(long)]
    pub(crate) vimgrep: bool,

    /// Shorthand for `--color=always --heading --line-number`.
    #[arg(short = 'p', long)]
    pub(crate) pretty: bool,

    /// List files instead of searching their contents.
    #[arg(long)]
    pub files: bool,

    /// Log level for diagnostics (`FFF_LOG` env var).
    #[arg(long, default_value = "warn", env = "FFF_LOG", global = true)]
    pub log_level: String,
}

