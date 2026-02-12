use std::path::{Path, PathBuf};

use crate::constraints::Constrainable;
use crate::query_tracker::QueryMatchEntry;
use fff_query_parser::{FFFQuery, FuzzyQuery, Location};

#[derive(Debug, Clone)]
pub struct FileItem {
    pub path: PathBuf,
    pub relative_path: String,
    pub relative_path_lower: String,
    pub file_name: String,
    pub file_name_lower: String,
    pub file_name_start_index: u16,
    pub size: u64,
    pub modified: u64,
    pub access_frecency_score: i64,
    pub modification_frecency_score: i64,
    pub total_frecency_score: i64,
    pub git_status: Option<git2::Status>,
}

impl Constrainable for FileItem {
    #[inline]
    fn relative_path(&self) -> &str {
        &self.relative_path
    }

    #[inline]
    fn relative_path_lower(&self) -> &str {
        &self.relative_path_lower
    }

    #[inline]
    fn file_name(&self) -> &str {
        &self.file_name
    }

    #[inline]
    fn git_status(&self) -> Option<git2::Status> {
        self.git_status
    }
}

#[derive(Debug, Clone)]
pub struct Score {
    pub total: i32,
    pub base_score: i32,
    pub filename_bonus: i32,
    pub special_filename_bonus: i32,
    pub frecency_boost: i32,
    pub distance_penalty: i32,
    pub current_file_penalty: i32,
    pub combo_match_boost: i32,
    pub exact_match: bool,
    pub match_type: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct PaginationArgs {
    pub offset: usize,
    pub limit: usize,
}

/// Context for scoring files during search.
///
/// The `parsed_query` field contains the pre-parsed query with constraints,
/// fuzzy parts, and location information. Parsing is done once at the API
/// boundary and passed through.
#[derive(Debug, Clone)]
pub struct ScoringContext<'a> {
    /// The original raw query string (for compatibility and debugging)
    pub raw_query: &'a str,
    /// Pre-parsed query containing constraints, fuzzy parts, and location
    pub parsed_query: Option<FFFQuery<'a>>,
    pub project_path: Option<&'a Path>,
    pub current_file: Option<&'a str>,
    pub max_typos: u16,
    pub max_threads: usize,
    pub last_same_query_match: Option<&'a QueryMatchEntry>,
    pub combo_boost_score_multiplier: i32,
    pub min_combo_count: u32,
    pub pagination: PaginationArgs,
}

impl<'a> ScoringContext<'a> {
    /// Get the effective fuzzy query string for matching.
    /// Returns the first fuzzy part, or the raw query if no parsing was done.
    pub fn effective_query(&self) -> &'a str {
        match &self.parsed_query {
            Some(p) => match &p.fuzzy_query {
                FuzzyQuery::Text(t) => t,
                FuzzyQuery::Parts(parts) if !parts.is_empty() => parts[0],
                _ => self.raw_query.trim(),
            },
            None => self.raw_query.trim(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SearchResult<'a> {
    pub items: Vec<&'a FileItem>,
    pub scores: Vec<Score>,
    pub total_matched: usize,
    pub total_files: usize,
    pub location: Option<Location>,
}
