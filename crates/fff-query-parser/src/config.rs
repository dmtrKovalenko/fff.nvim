use crate::constraints::Constraint;
use zlob::{ZlobFlags, has_wildcards};

/// Parser configuration trait - allows different picker types to customize parsing
pub trait ParserConfig {
    fn enable_glob(&self) -> bool {
        true
    }

    /// Should parse extension shortcuts (e.g., *.rs)
    fn enable_extension(&self) -> bool {
        true
    }

    /// Should parse exclusion patterns (e.g., !test)
    fn enable_exclude(&self) -> bool {
        true
    }

    /// Should parse path segments (e.g., /src/)
    fn enable_path_segments(&self) -> bool {
        true
    }

    /// Should parse type constraints (e.g., type:rust)
    fn enable_type_filter(&self) -> bool {
        true
    }

    /// Should parse git status (e.g., status:modified)
    fn enable_git_status(&self) -> bool {
        true
    }

    /// Determine whether a token should be treated as a glob constraint.
    ///
    /// The default implementation delegates to `zlob::has_wildcards` with
    /// `RECOMMENDED` flags, which recognises `*`, `?`, `[`, `{…}` etc.
    ///
    /// Override this in configs where some wildcard characters are common
    /// in search text (e.g. grep mode where `?` and `[` appear in code).
    fn is_glob_pattern(&self, token: &str) -> bool {
        has_wildcards(token, ZlobFlags::RECOMMENDED)
    }

    /// Custom constraint parsers for picker-specific needs
    fn parse_custom<'a>(&self, _input: &'a str) -> Option<Constraint<'a>> {
        None
    }
}

/// Default configuration for file picker - all features enabled
#[derive(Debug, Clone, Copy, Default)]
pub struct FilePickerConfig;

impl ParserConfig for FilePickerConfig {
    // All defaults enabled
}

/// Configuration for full-text search (grep) - file constraints enabled for
/// filtering which files to search, git status disabled since it's not useful
/// when searching file contents.
///
/// Glob detection is narrowed: only patterns containing a path separator (`/`)
/// or brace expansion (`{…}`) are treated as globs. Characters like `?` and
/// `[` are extremely common in source code and must remain literal search text.
#[derive(Debug, Clone, Copy, Default)]
pub struct GrepConfig;

impl ParserConfig for GrepConfig {
    fn enable_path_segments(&self) -> bool {
        true
    }

    fn enable_git_status(&self) -> bool {
        false
    }

    /// Only recognise globs that are clearly directory/path oriented.
    ///
    /// Characters like `?`, `[`, and bare `*` (without `/`) are extremely
    /// common in source code (`foo?`, `arr[0]`, `*ptr`) and must NOT be
    /// consumed as glob constraints. We only treat a token as a glob when
    /// it contains path-oriented patterns:
    ///
    /// - Contains `/` → path glob (e.g. `src/**/*.rs`, `*/tests/*`)
    /// - Contains `{…}` → brace expansion (e.g. `{src,lib}`)
    fn is_glob_pattern(&self, token: &str) -> bool {
        // Must contain at least one glob wildcard character
        if !has_wildcards(token, ZlobFlags::RECOMMENDED) {
            return false;
        }

        let bytes = token.as_bytes();

        // Contains path separator → clearly a path glob
        if bytes.contains(&b'/') {
            return true;
        }

        // Brace expansion → useful for directory alternatives
        if bytes.contains(&b'{') && bytes.contains(&b'}') {
            return true;
        }

        // Everything else (?, [, bare * without /) → treat as literal text
        false
    }
}
