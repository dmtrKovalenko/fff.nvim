use serde::{Deserialize, Serialize};

// ── Master protocol ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MasterRequest {
    /// Connect a new fff-mcp client; returns the worker socket to use.
    Handshake { base_path: String },
    /// List all active workers and their loaded roots.
    ListWorkers,
    /// Query status of a specific worker by index.
    WorkerStatus { index: u32 },
    /// Gracefully stop a worker by index.
    StopWorker { index: u32 },
    /// Fire-and-forget: worker notifies master that a root was LRU-evicted.
    /// Master removes the routing table entry. No response is sent.
    EvictedRoot { slug: String },
    /// Read-only route query for fffctl — does not mutate state or trigger scale-out.
    RouteInfo { base_path: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MasterResponse {
    /// Returned for Handshake — direct the client to this worker socket.
    WorkerSocket { path: String, worker_index: u32 },
    /// Returned for ListWorkers.
    WorkerList { workers: Vec<WorkerInfo> },
    /// Returned for WorkerStatus / RouteInfo.
    WorkerInfo(WorkerInfo),
    Ack,
    Error(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerInfo {
    pub index: u32,
    pub socket_path: String,
    pub root_slugs: Vec<String>,
    pub root_count: usize,
    pub pid: u32,
}

// ── Request ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SearchRequest {
    /// First message sent on a worker socket connection.
    /// Worker loads state for this root on demand and responds with Ack.
    Connect { base_path: String },
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
    /// Return the top-N files by frecency score.
    ListRecentFiles {
        limit: usize,
        /// When true, only include files with a non-clean git status.
        dirty_only: bool,
    },
    /// Return all files with a notable git status.
    GetGitStatus {
        /// When true, include clean files too.
        include_clean: bool,
    },
    /// Return directories ranked by the peak frecency of their child files.
    ListDirectories {
        limit: usize,
    },
}

// ── Response ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SearchResponse {
    SearchResults(Vec<WireSearchResult>),
    GrepResults(WireGrepResponse),
    Error(String),
    Ack,
    RecentFiles(Vec<WireSearchResult>),
    GitStatus(Vec<WireGitFile>),
    Directories(Vec<WireDirEntry>),
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

/// One file from `GetGitStatus`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireGitFile {
    pub path: String,
    /// Human-readable status label ("modified", "untracked", etc.)
    pub status: String,
    pub frecency_score: i32,
}

/// One directory from `ListDirectories`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireDirEntry {
    pub path: String,
    pub max_frecency: i32,
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
    fn recent_files_response_round_trips() {
        let resp = SearchResponse::RecentFiles(vec![WireSearchResult {
            path: "src/hot.rs".into(),
            score: 200,
            git_status: Some(0),
            frecency_score: 200,
        }]);
        let rt = round_trip(&resp);
        match rt {
            SearchResponse::RecentFiles(v) => assert_eq!(v[0].path, "src/hot.rs"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn git_status_response_round_trips() {
        let resp = SearchResponse::GitStatus(vec![WireGitFile {
            path: "src/changed.rs".into(),
            status: "modified".into(),
            frecency_score: 10,
        }]);
        let rt = round_trip(&resp);
        match rt {
            SearchResponse::GitStatus(v) => {
                assert_eq!(v[0].path, "src/changed.rs");
                assert_eq!(v[0].status, "modified");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn directories_response_round_trips() {
        let resp = SearchResponse::Directories(vec![WireDirEntry {
            path: "src/".into(),
            max_frecency: 50,
        }]);
        let rt = round_trip(&resp);
        match rt {
            SearchResponse::Directories(v) => {
                assert_eq!(v[0].path, "src/");
                assert_eq!(v[0].max_frecency, 50);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn list_recent_files_request_round_trips() {
        let req = SearchRequest::ListRecentFiles { limit: 10, dirty_only: true };
        let rt = round_trip(&req);
        match rt {
            SearchRequest::ListRecentFiles { limit, dirty_only } => {
                assert_eq!(limit, 10);
                assert!(dirty_only);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn get_git_status_request_round_trips() {
        let req = SearchRequest::GetGitStatus { include_clean: false };
        let rt = round_trip(&req);
        match rt {
            SearchRequest::GetGitStatus { include_clean } => assert!(!include_clean),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn list_directories_request_round_trips() {
        let req = SearchRequest::ListDirectories { limit: 30 };
        let rt = round_trip(&req);
        match rt {
            SearchRequest::ListDirectories { limit } => assert_eq!(limit, 30),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn master_handshake_round_trips() {
        let req = MasterRequest::Handshake { base_path: "/home/user/project".into() };
        let rt = round_trip(&req);
        match rt {
            MasterRequest::Handshake { base_path } => assert_eq!(base_path, "/home/user/project"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn master_response_worker_socket_round_trips() {
        let resp = MasterResponse::WorkerSocket { path: "/tmp/fff/workers/worker-0.sock".into(), worker_index: 0 };
        let rt = round_trip(&resp);
        match rt {
            MasterResponse::WorkerSocket { path, worker_index } => {
                assert_eq!(path, "/tmp/fff/workers/worker-0.sock");
                assert_eq!(worker_index, 0);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn master_response_worker_list_round_trips() {
        let resp = MasterResponse::WorkerList {
            workers: vec![
                WorkerInfo { index: 0, socket_path: "worker-0.sock".into(), root_slugs: vec!["abc".into()], root_count: 1, pid: 1234 },
                WorkerInfo { index: 1, socket_path: "worker-1.sock".into(), root_slugs: vec![], root_count: 0, pid: 5678 },
            ],
        };
        let rt = round_trip(&resp);
        match rt {
            MasterResponse::WorkerList { workers } => {
                assert_eq!(workers.len(), 2);
                assert_eq!(workers[0].pid, 1234);
                assert_eq!(workers[1].root_count, 0);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn search_request_connect_round_trips() {
        let req = SearchRequest::Connect { base_path: "/home/user/repo".into() };
        let rt = round_trip(&req);
        match rt {
            SearchRequest::Connect { base_path } => assert_eq!(base_path, "/home/user/repo"),
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
