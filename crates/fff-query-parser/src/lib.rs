//! Fast query parser for file search
//!
//! This parser takes a search query and extracts structured constraints
//! while preserving text for fuzzy matching. Designed for maximum performance:
//! - Single-pass parsing with minimal branching
//! - Stack-allocated string buffers
//!
//! # Examples
//!
//! ```
//! use fff_query_parser::{QueryParser, Constraint, FuzzyQuery};
//!
//! let parser = QueryParser::default();
//!
//! // Single-token queries return FFFQuery with Text fuzzy query and no constraints
//! let result = parser.parse("hello");
//! assert!(result.constraints.is_empty());
//! assert_eq!(result.fuzzy_query, FuzzyQuery::Text("hello"));
//!
//! // Multi-token queries are parsed
//! let result = parser.parse("name *.rs");
//! match &result.fuzzy_query {
//!     FuzzyQuery::Text(text) => assert_eq!(*text, "name"),
//!     _ => panic!("Expected text"),
//! }
//! assert!(matches!(result.constraints[0], Constraint::Extension("rs")));
//!
//! // Parse glob pattern with text
//! let result = parser.parse("**/*.rs foo");
//! assert!(matches!(result.constraints[0], Constraint::Glob("**/*.rs")));
//!
//! // Parse negation
//! let result = parser.parse("!*.rs foo");
//! match &result.constraints[0] {
//!     Constraint::Not(inner) => {
//!         assert!(matches!(inner.as_ref(), Constraint::Extension("rs")));
//!     }
//!     _ => panic!("Expected Not constraint"),
//! }
//! ```

mod config;
mod constraints;
pub mod glob_detect;
pub mod location;
mod parser;

pub use config::{
    AiGrepConfig, DirSearchConfig, FileSearchConfig, GrepConfig, MixedSearchConfig, ParserConfig,
};
pub use constraints::{Constraint, GitStatusFilter};
pub use location::Location;
pub use parser::{FFFQuery, FuzzyQuery, QueryParser};

pub type ConstraintVec<'a> = Vec<Constraint<'a>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_query() {
        let parser = QueryParser::default();
        let result = parser.parse("");
        assert!(result.constraints.is_empty());
        assert_eq!(result.fuzzy_query, FuzzyQuery::Empty);
    }

    #[test]
    fn test_whitespace_only() {
        let parser = QueryParser::default();
        let result = parser.parse("   ");
        assert!(result.constraints.is_empty());
        assert_eq!(result.fuzzy_query, FuzzyQuery::Empty);
    }

    #[test]
    fn test_single_token() {
        let parser = QueryParser::default();
        let result = parser.parse("hello");
        assert!(result.constraints.is_empty());
        assert_eq!(result.fuzzy_query, FuzzyQuery::Text("hello"));
    }

    #[test]
    fn test_simple_text() {
        let parser = QueryParser::default();
        let result = parser.parse("hello world");

        match &result.fuzzy_query {
            FuzzyQuery::Parts(parts) => {
                assert_eq!(parts.len(), 2);
                assert_eq!(parts[0], "hello");
                assert_eq!(parts[1], "world");
            }
            _ => panic!("Expected Parts fuzzy query"),
        }

        assert_eq!(result.constraints.len(), 0);
    }

    #[test]
    fn test_extension_only() {
        let parser = QueryParser::default();
        // Single constraint token - returns Some so constraint can be applied
        let result = parser.parse("*.rs");
        assert!(matches!(result.fuzzy_query, FuzzyQuery::Empty));
        assert_eq!(result.constraints.len(), 1);
        assert!(matches!(result.constraints[0], Constraint::Extension("rs")));
    }

    #[test]
    fn test_glob_pattern() {
        let parser = QueryParser::default();
        let result = parser.parse("**/*.rs foo");
        assert_eq!(result.constraints.len(), 1);
        // Glob patterns with ** are treated as globs, not extensions
        match &result.constraints[0] {
            Constraint::Glob(pattern) => assert_eq!(*pattern, "**/*.rs"),
            other => panic!("Expected Glob constraint, got {:?}", other),
        }
    }

    #[test]
    fn test_negation_pattern() {
        let parser = QueryParser::default();
        let result = parser.parse("!test foo");
        assert_eq!(result.constraints.len(), 1);
        match &result.constraints[0] {
            Constraint::Not(inner) => {
                assert!(matches!(**inner, Constraint::Text("test")));
            }
            _ => panic!("Expected Not constraint"),
        }
    }

    #[test]
    fn test_path_segment() {
        let parser = QueryParser::default();
        let result = parser.parse("/src/ foo");
        assert_eq!(result.constraints.len(), 1);
        assert!(matches!(
            result.constraints[0],
            Constraint::PathSegment("src")
        ));
    }

    #[test]
    fn test_git_status() {
        let parser = QueryParser::default();
        let result = parser.parse("status:modified foo");
        assert_eq!(result.constraints.len(), 1);
        assert!(matches!(
            result.constraints[0],
            Constraint::GitStatus(GitStatusFilter::Modified)
        ));
    }

    #[test]
    fn test_file_type() {
        let parser = QueryParser::default();
        let result = parser.parse("type:rust foo");
        assert_eq!(result.constraints.len(), 1);
        assert!(matches!(
            result.constraints[0],
            Constraint::FileType("rust")
        ));
    }

    #[test]
    fn test_complex_query() {
        let parser = QueryParser::default();
        let result = parser.parse("src name *.rs !test /lib/ status:modified");

        // Verify we have fuzzy text
        match &result.fuzzy_query {
            FuzzyQuery::Parts(parts) => {
                assert_eq!(parts.len(), 2);
                assert_eq!(parts[0], "src");
                assert_eq!(parts[1], "name");
            }
            _ => panic!("Expected Parts fuzzy query"),
        }

        // Should have multiple constraints
        assert!(result.constraints.len() >= 4);

        // Verify specific constraints exist
        let has_extension = result
            .constraints
            .iter()
            .any(|c| matches!(c, Constraint::Extension("rs")));
        let has_not = result
            .constraints
            .iter()
            .any(|c| matches!(c, Constraint::Not(_)));
        let has_path = result
            .constraints
            .iter()
            .any(|c| matches!(c, Constraint::PathSegment("lib")));
        let has_git_status = result
            .constraints
            .iter()
            .any(|c| matches!(c, Constraint::GitStatus(_)));

        assert!(has_extension, "Should have Extension constraint");
        assert!(has_not, "Should have Not constraint");
        assert!(has_path, "Should have PathSegment constraint");
        assert!(has_git_status, "Should have GitStatus constraint");
    }

    #[test]
    fn test_small_constraint_count() {
        let parser = QueryParser::default();
        let result = parser.parse("*.rs *.toml !test");
        assert_eq!(result.constraints.len(), 3);
    }

    #[test]
    fn test_many_fuzzy_parts() {
        let parser = QueryParser::default();
        let result = parser.parse("one two three four five six");

        match &result.fuzzy_query {
            FuzzyQuery::Parts(parts) => {
                assert_eq!(parts.len(), 6);
                assert_eq!(parts[0], "one");
                assert_eq!(parts[5], "six");
            }
            _ => panic!("Expected Parts fuzzy query"),
        }
    }
}
