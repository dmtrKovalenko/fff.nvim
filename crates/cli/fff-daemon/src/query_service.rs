//! Listens on a per-user Unix domain socket for search requests. Each client
//! connects, sends a [`RequestHeader`] + rkyv body alongside an output fd
//! (via SCM_RIGHTS), and receives a single [`SearchStatus`] byte back.

use std::io::{Read, Write};
use std::num::NonZeroU64;
use std::os::fd::OwnedFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::FromRawFd;
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Max time to wait for the client to send the request body.
const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// Max time to wait for the file index to finish its initial scan.
const SCAN_TIMEOUT: Duration = Duration::from_secs(30);
/// Accept loop poll interval (non-blocking listener).
const ACCEPT_POLL: Duration = Duration::from_millis(1);
/// Owner-only permissions for the daemon socket.
const SOCKET_MODE: u32 = 0o600;
/// Inline buffer for small rkyv bodies — avoids heap allocation for typical requests.
const IPC_BODY_INLINE: usize = 512;
/// Default max file size for grep when the client doesn't specify one (4 MiB).
const DEFAULT_MAX_FILE_SIZE: u64 = 4 * 1024 * 1024;

use fff::{
    FilePicker, FuzzySearchOptions, GrepMode, GrepSearchOptions, QueryParser, parse_grep_query,
};
use fff_ipc_domain::{
    GrepQuery, GrepSearch, OutputFlags, RequestHeader, SearchKind, SearchRequest, SearchStatus,
    daemon_socket_path,
};
use sendfd::RecvWithFd;

use crate::convert::IntoCoreExt;

use crate::session_pool::{MAX_SESSIONS, SessionPool};

/// Parsed client request with the fd to write results into.
struct IncomingQuery {
    request: SearchRequest,
    output: std::fs::File,
}

impl IncomingQuery {
    /// Reads the 4-byte length header, receives the output fd, and deserializes the rkyv body.
    fn recv(
        stream: &mut std::os::unix::net::UnixStream,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut raw_header = [0u8; RequestHeader::SIZE];
        let mut fds = [0; 1];
        let (n, fd_count) = stream.recv_with_fd(&mut raw_header, &mut fds)?;

        if fd_count == 0 {
            return Err("no fd in ancillary data".into());
        }

        // SAFETY: fd received via SCM_RIGHTS — this process owns it exclusively.
        let owned_fd = unsafe { OwnedFd::from_raw_fd(fds[0]) };

        if n < RequestHeader::SIZE {
            return Err("incomplete header".into());
        }

        let header = RequestHeader::decode(raw_header);
        let body_len = header.body_len as usize;

        stream.set_read_timeout(Some(READ_TIMEOUT))?;
        let mut body = smallvec::SmallVec::<[u8; IPC_BODY_INLINE]>::from_elem(0, body_len);
        stream.read_exact(&mut body)?;

        let archived = rkyv::access::<rkyv::Archived<SearchRequest>, rkyv::rancor::Error>(&body)
            .map_err(|e| format!("rkyv access: {e}"))?;
        let request = rkyv::deserialize::<SearchRequest, rkyv::rancor::Error>(archived)
            .map_err(|e| format!("rkyv deserialize: {e}"))?;

        Ok(Self { request, output: std::fs::File::from(owned_fd) })
    }
}

/// RAII guard for the daemon's Unix listener socket. Cleans up the socket file on drop.
struct ActiveDaemonSocket {
    path: std::path::PathBuf,
    listener: UnixListener,
}

impl ActiveDaemonSocket {
    /// Binds a non-blocking Unix listener at the daemon socket path with 0600 permissions.
    fn bind() -> Result<Self, Box<dyn std::error::Error>> {
        let path = daemon_socket_path();
        let _ = std::fs::remove_file(&path);

        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let listener = UnixListener::bind(&path)?;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(SOCKET_MODE));
        listener.set_nonblocking(true).expect("failed to set listener non-blocking");

        Ok(Self { path, listener })
    }
}

impl Drop for ActiveDaemonSocket {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Accepts connections on the daemon socket and dispatches searches to worker threads.
pub struct QueryService {
    pool: Arc<SessionPool>,
    shutdown: Arc<AtomicBool>,
    workers: rayon::ThreadPool,
}

impl QueryService {
    /// Creates the session pool and a rayon thread pool sized to `MAX_SESSIONS`.
    pub fn new(shutdown: Arc<AtomicBool>) -> Self {
        let pool = SessionPool::new();
        let workers = rayon::ThreadPoolBuilder::new()
            .num_threads(MAX_SESSIONS)
            .thread_name(|i| format!("conn-{i}"))
            .build()
            .expect("failed to build connection thread pool");
        Self { pool: Arc::new(pool), shutdown, workers }
    }

    /// Blocking accept loop. Polls the non-blocking listener at 1ms intervals until shutdown.
    #[tracing::instrument(level = "trace", skip(self))]
    pub fn run(&self) {
        let socket = match ActiveDaemonSocket::bind() {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(err = %e, "failed to bind unix socket");
                return;
            }
        };

        tracing::info!(path = %socket.path.display(), "query service listening");

        while !self.shutdown.load(Ordering::Relaxed) {
            match socket.listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false).expect("accepted socket has invalid fd");
                    let pool = self.pool.clone();
                    self.workers.spawn(move || {
                        if let Err(e) = Self::handle_connection(&pool, &mut stream) {
                            tracing::warn!(err = %e, "connection handler failed");
                        }
                    });
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(ACCEPT_POLL);
                }
                Err(e) => {
                    tracing::warn!(err = %e, "accept failed");
                }
            }
        }

        self.pool.shutdown();
    }

    /// Parses one request, runs the search, and writes back a status byte.
    #[tracing::instrument(level = "trace", skip_all)]
    fn handle_connection(
        pool: &SessionPool,
        stream: &mut std::os::unix::net::UnixStream,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut query = IncomingQuery::recv(stream)?;

        let status = match Self::run_search(pool, &query.request, &mut query.output) {
            Ok(true) => SearchStatus::Match,
            Ok(false) => SearchStatus::NoMatch,
            Err(e) => {
                tracing::warn!(err = %e, dir = %query.request.directory, "search failed");
                SearchStatus::Failed
            }
        };

        stream.write_all(&[status.into()])?;
        Ok(())
    }

    /// Acquires a `FilePicker` for the directory, waits for indexing, and dispatches by mode.
    #[tracing::instrument(level = "trace", skip(pool, out), fields(dir = %req.directory))]
    fn run_search(
        pool: &SessionPool,
        req: &SearchRequest,
        out: &mut std::fs::File,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        let picker_handle = pool.get_or_create(Path::new(&req.directory))?;
        if !picker_handle.wait_for_scan(SCAN_TIMEOUT) {
            return Err(format!(
                "indexing {} timed out after 30s; the directory may still be scanning — retry shortly",
                req.directory
            )
            .into());
        }

        let guard = picker_handle.read()?;
        let picker = guard.as_ref().ok_or("picker not ready after scan")?;

        match &req.search {
            SearchKind::Files { query } => {
                Self::write_file_results(picker, query, req.output, out)
            }
            SearchKind::Grep(grep) => Self::write_grep_results(picker, grep, req.output, out),
        }
    }

    /// Runs a grep search and writes formatted matches to the output fd.
    fn write_grep_results(
        picker: &FilePicker,
        grep: &GrepSearch,
        output: OutputFlags,
        out: &mut std::fs::File,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        let (query_str, mode) = match &grep.query {
            GrepQuery::Regex(q) => (q.as_str(), GrepMode::Regex),
            GrepQuery::Literal(q) => (q.as_str(), GrepMode::PlainText),
        };
        let parsed = parse_grep_query(query_str);

        let options = GrepSearchOptions {
            max_file_size: grep.max_filesize.map_or(DEFAULT_MAX_FILE_SIZE, NonZeroU64::get),
            max_matches_per_file: grep.max_count.map_or(0, |n| n.get() as usize),
            case_mode: grep.case_mode.into_core(),
            file_offset: 0,
            page_limit: usize::MAX,
            mode,
            time_budget_ms: 0,
            before_context: grep.before_context as usize,
            after_context: grep.after_context as usize,
            classify_definitions: false,
            trim_whitespace: grep.trim,
            abort_signal: None,
        };

        let result = picker.grep(&parsed, &options);
        let mut writer = crate::output::ResultWriter::new(out, output);
        writer.write_grep(picker, &result)
    }

    /// Runs a fuzzy file search and writes formatted results to the output fd.
    fn write_file_results(
        picker: &FilePicker,
        query: &str,
        output: OutputFlags,
        out: &mut std::fs::File,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        let parsed = QueryParser::default().parse(query);
        let result = picker.fuzzy_search(&parsed, None, FuzzySearchOptions::default());
        let mut writer = crate::output::ResultWriter::new(out, output);
        writer.write_files(picker, &result)
    }
}

