use crate::constraints::Constraint;

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

/// Configuration for full-text search (grep) - limited constraints
#[derive(Debug, Clone, Copy, Default)]
pub struct GrepConfig;

impl ParserConfig for GrepConfig {
    fn enable_extension(&self) -> bool {
        false
    }

    fn enable_glob(&self) -> bool {
        false
    }

    fn enable_path_segments(&self) -> bool {
        true
    }

    fn enable_git_status(&self) -> bool {
        false
    }
}
