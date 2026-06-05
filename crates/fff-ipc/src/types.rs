use serde::{Deserialize, Serialize};

// ── Request ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SearchRequest {
    Grep {
        query: String,
        options: GrepOptions,
    },
    FindFiles {
        query: String,
        options: FindOptions,
    },
    MultiGrep {
        patterns: Vec<String>,
        /// Raw constraint query string (e.g. `"*.rs !test/"`). Parsed in
        /// fff-engine using the same AiGrepConfig parser as fff-mcp.
        constraints: Option<String>,
        options: GrepOptions,
    },
    /// Fire-and-forget frecency write. fff-mcp sends and does not await a
    /// response. fff-engine sends no response for this variant.
    RecordAccess {
        path: String,
    },
    /// Hot-reload the daemon's log filter. Accepts any RUST_LOG-style string
    /// (e.g. "debug", "info", "fff_engine=debug,info"). Returns Ack.
    SetLogLevel {
        level: String,
    },
}

// ── Response ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SearchResponse {
    SearchResults(Vec<WireSearchResult>),
    GrepResults(WireGrepResponse),
    Error(String),
    Ack,
}

// ── Grep result types ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireGrepResponse {
    pub matches: Vec<WireGrepFileMatches>,
    pub total_files_searched: usize,
    pub total_files: usize,
    pub files_with_matches: usize,
    pub next_file_offset: usize,
    pub regex_fallback_error: Option<String>,
}

/// All matches for a single file, grouped together to mirror GrepResult's
/// file-indexed layout without carrying raw pointers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireGrepFileMatches {
    pub path: String,
    pub size: u64,
    pub git_status: Option<u32>,
    /// access_frecency_score + modification_frecency_score from FileItem.
    pub frecency_score: i32,
    pub matches: Vec<WireGrepMatch>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireGrepMatch {
    pub line_number: u64,
    pub col: usize,
    pub line_text: String,
    /// Byte offsets `(start, end)` within `line_text` for each match span.
    pub match_byte_offsets: Vec<(u32, u32)>,
    pub is_definition: bool,
    pub context_before: Vec<String>,
    pub context_after: Vec<String>,
}

// ── Find-files result type ────────────────────────────────────────────────────

/// One ranked file from a fuzzy find_files search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireSearchResult {
    pub path: String,
    pub score: i32,
    pub git_status: Option<u32>,
    /// access_frecency_score + modification_frecency_score.
    pub frecency_score: i32,
}

// ── Options ───────────────────────────────────────────────────────────────────

/// Serialisable subset of `GrepSearchOptions`. Fields that cannot cross the
/// wire (e.g. `abort_signal: Arc<AtomicBool>`) are omitted; fff-engine
/// applies sensible defaults for them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrepOptions {
    pub max_file_size: u64,
    pub max_matches_per_file: usize,
    pub smart_case: bool,
    pub file_offset: usize,
    pub page_limit: usize,
    pub mode: WireGrepMode,
    pub time_budget_ms: u64,
    pub before_context: usize,
    pub after_context: usize,
    pub classify_definitions: bool,
    pub trim_whitespace: bool,
}

impl Default for GrepOptions {
    fn default() -> Self {
        Self {
            max_file_size: 10 * 1024 * 1024,
            max_matches_per_file: 10,
            smart_case: true,
            file_offset: 0,
            page_limit: 50,
            mode: WireGrepMode::PlainText,
            time_budget_ms: 0,
            before_context: 0,
            after_context: 0,
            classify_definitions: false,
            trim_whitespace: true,
        }
    }
}

/// Wire-safe mirror of `GrepMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireGrepMode {
    PlainText,
    Regex,
    Fuzzy,
}

/// Serialisable subset of `FuzzySearchOptions`. Lifetime-bound fields
/// (`current_file: Option<&'a str>`, `project_path: Option<&'a Path>`) are
/// converted to owned types.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindOptions {
    pub max_threads: usize,
    pub current_file: Option<String>,
    pub combo_boost_score_multiplier: i32,
    pub min_combo_count: u32,
    pub offset: usize,
    pub limit: usize,
}

impl Default for FindOptions {
    fn default() -> Self {
        Self {
            max_threads: 0,
            current_file: None,
            combo_boost_score_multiplier: 3,
            min_combo_count: 2,
            offset: 0,
            limit: 20,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip<T: serde::Serialize + serde::de::DeserializeOwned>(value: &T) -> T {
        let bytes = bincode::serialize(value).expect("serialize");
        bincode::deserialize(&bytes).expect("deserialize")
    }

    #[test]
    fn grep_request_round_trips() {
        let req = SearchRequest::Grep {
            query: "héllo wörld".into(),
            options: GrepOptions::default(),
        };
        let rt = round_trip(&req);
        match rt {
            SearchRequest::Grep { query, .. } => assert_eq!(query, "héllo wörld"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn empty_search_results_round_trips() {
        let resp = SearchResponse::SearchResults(vec![]);
        let rt = round_trip(&resp);
        match rt {
            SearchResponse::SearchResults(v) => assert!(v.is_empty()),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn error_response_round_trips() {
        let resp = SearchResponse::Error("something went wrong".into());
        let rt = round_trip(&resp);
        match rt {
            SearchResponse::Error(msg) => assert_eq!(msg, "something went wrong"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn grep_results_round_trips() {
        let resp = SearchResponse::GrepResults(WireGrepResponse {
            matches: vec![WireGrepFileMatches {
                path: "src/lib.rs".into(),
                size: 1024,
                git_status: Some(1),
                frecency_score: 42,
                matches: vec![WireGrepMatch {
                    line_number: 42,
                    col: 4,
                    line_text: "fn main() {}".into(),
                    match_byte_offsets: vec![(3, 7)],
                    is_definition: true,
                    context_before: vec![],
                    context_after: vec![],
                }],
            }],
            total_files_searched: 10,
            total_files: 100,
            files_with_matches: 1,
            next_file_offset: 0,
            regex_fallback_error: None,
        });
        let rt = round_trip(&resp);
        match rt {
            SearchResponse::GrepResults(r) => {
                assert_eq!(r.matches[0].path, "src/lib.rs");
                assert_eq!(r.matches[0].matches[0].line_number, 42);
            }
            _ => panic!("wrong variant"),
        }
    }
}
