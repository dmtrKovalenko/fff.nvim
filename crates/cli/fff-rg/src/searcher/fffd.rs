//! Client-side daemon searcher. Connects to the `fff-daemon` Unix socket,
//! sends a search request with stdout as the output fd, and reads back a
//! status byte. Spawns the daemon on first use if it isn't already running.

use std::io::{Read, Write};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::Duration;

use std::num::{NonZeroU32, NonZeroU64};

use fff_ipc_domain::{
    GrepQuery, GrepSearch, OutputFlags, RequestHeader, SearchKind, SearchRequest, SearchStatus,
    daemon_socket_path,
};
use sendfd::SendWithFd;

use crate::app_ctx::AppCtx;
use crate::searcher::Search;
use crate::types::cli::ColorMode;

/// Max time to wait for the daemon to write search results.
const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// Max time to wait for the request to be sent to the daemon.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
/// Max time to wait for a freshly-spawned daemon to accept connections.
const DAEMON_STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
/// Poll interval when waiting for daemon socket to become connectable.
const DAEMON_POLL_INTERVAL: Duration = Duration::from_millis(1);

/// Single-use connection to the daemon. Consumed by `query`, which drops the
/// stream on completion — making the one-request-per-connection protocol
/// constraint explicit in the type system.
struct DaemonConnection(UnixStream);

impl DaemonConnection {
    /// Connects to a running daemon, or spawns one and waits for it to be ready.
    #[tracing::instrument(level = "trace", skip_all)]
    fn open(daemon_bin: &std::path::Path) -> Result<Self, Box<dyn std::error::Error>> {
        let socket_path = daemon_socket_path();

        if let Ok(stream) = UnixStream::connect(&socket_path) {
            return Self::configure(stream);
        }

        Self::spawn_daemon(&socket_path, daemon_bin)?;
        Self::configure(UnixStream::connect(&socket_path)?)
    }

    /// Sets read/write timeouts on the stream.
    fn configure(stream: UnixStream) -> Result<Self, Box<dyn std::error::Error>> {
        stream.set_read_timeout(Some(READ_TIMEOUT))?;
        stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
        Ok(Self(stream))
    }

    /// Sends the request + stdout fd, waits for results, and returns match status.
    /// Consumes self — one request per connection.
    #[tracing::instrument(level = "trace", skip_all)]
    fn query(self, req: &SearchRequest) -> Result<bool, Box<dyn std::error::Error>> {
        let req_bytes =
            rkyv::to_bytes::<rkyv::rancor::Error>(req).map_err(|e| format!("serialize: {e}"))?;
        let header = RequestHeader::encode(req_bytes.len());

        let stdout_fd = std::io::stdout().as_raw_fd();
        self.0.send_with_fd(&header, &[stdout_fd])?;

        (&self.0).write_all(&req_bytes)?;

        let mut status = [0u8; 1];
        (&self.0).read_exact(&mut status)?;

        match SearchStatus::try_from(status[0]) {
            Ok(SearchStatus::Match) => Ok(true),
            Ok(SearchStatus::NoMatch) => Ok(false),
            Ok(SearchStatus::Failed) => Err("daemon reported search failure".into()),
            Err(c) => Err(format!("daemon returned unknown status {c}").into()),
        }
    }

    /// Spawns `fff-daemon` and polls until the socket is connectable (up to 5s).
    #[tracing::instrument(level = "trace", skip_all)]
    fn spawn_daemon(
        socket_path: &std::path::Path,
        bin: &std::path::Path,
    ) -> Result<(), Box<dyn std::error::Error>> {
        tracing::debug!("spawning fff-daemon");

        Command::new(bin)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .map_err(|e| format!("failed to spawn {}: {e}", bin.display()))?;

        // Instant::now() is vDSO/commpage — no syscall; connect() dominates.
        let deadline = std::time::Instant::now() + DAEMON_STARTUP_TIMEOUT;
        while std::time::Instant::now() < deadline {
            if UnixStream::connect(socket_path).is_ok() {
                return Ok(());
            }
            std::thread::sleep(DAEMON_POLL_INTERVAL);
        }

        Err(format!(
            "fff-daemon ({}) did not start within {}s. \
             Try running the daemon manually to see errors: FFF_LOG=debug {}",
            bin.display(),
            DAEMON_STARTUP_TIMEOUT.as_secs(),
            bin.display()
        )
        .into())
    }
}

/// [`Search`] backend that delegates to the daemon over IPC.
pub struct DaemonSearcher<'a> {
    ctx: AppCtx<'a>,
}

impl<'a> DaemonSearcher<'a> {
    pub fn new(ctx: AppCtx<'a>) -> Self {
        Self { ctx }
    }

    /// Converts the owned context into a daemon search request.
    fn build_request(&self) -> SearchRequest {
        let args = self.ctx.args;
        let directory = self.ctx.git_root.as_deref().unwrap_or(&self.ctx.dir);
        let pattern = args.pattern.clone().unwrap_or_default();

        let search = if args.files {
            SearchKind::Files { query: pattern }
        } else {
            let context = args.context.unwrap_or(0);
            SearchKind::Grep(GrepSearch {
                query: if args.fixed_strings {
                    GrepQuery::Literal(pattern)
                } else {
                    GrepQuery::Regex(pattern)
                },
                case_mode: args.case.resolve(),
                max_count: args.max_count.and_then(NonZeroU32::new),
                max_filesize: args.max_filesize.and_then(|fs| NonZeroU64::new(fs.as_u64())),
                before_context: args.before_context.unwrap_or(context),
                after_context: args.after_context.unwrap_or(context),
                trim: args.trim,
            })
        };

        SearchRequest {
            directory: directory.to_string(),
            search,
            output: Self::resolve_output_flags(args, self.ctx.is_tty),
        }
    }

    /// Maps CLI output flags to the IPC output bitmask.
    fn resolve_output_flags(args: &crate::types::cli::Args, is_tty: bool) -> OutputFlags {
        let pretty = args.pretty;
        let color = match args.color {
            ColorMode::Always | ColorMode::Ansi => true,
            ColorMode::Never => false,
            ColorMode::Auto => is_tty,
        };

        let mut f = OutputFlags::empty();
        if color || pretty { f |= OutputFlags::COLOR; }
        if !args.no_line_number && (args.line_number || pretty || is_tty) { f |= OutputFlags::LINE_NUMBER; }
        if args.column || args.vimgrep { f |= OutputFlags::COLUMN; }
        if !args.no_heading && (args.heading || pretty || is_tty) { f |= OutputFlags::HEADING; }
        if !args.no_filename { f |= OutputFlags::WITH_FILENAME; }
        if args.count { f |= OutputFlags::COUNT_ONLY; }
        if args.files_with_matches { f |= OutputFlags::FILES_ONLY; }
        if args.quiet { f |= OutputFlags::QUIET; }
        if args.vimgrep { f |= OutputFlags::VIMGREP; }
        f
    }
}

impl Search for DaemonSearcher<'_> {
    fn grep(&self) -> Result<bool, Box<dyn std::error::Error>> {
        let req = self.build_request();
        DaemonConnection::open(&self.ctx.daemon_bin)?.query(&req)
    }

    fn files(&self) -> Result<bool, Box<dyn std::error::Error>> {
        let req = self.build_request();
        DaemonConnection::open(&self.ctx.daemon_bin)?.query(&req)
    }
}
