use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Once;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
static DAEMON: Once = Once::new();

/// Max time to wait for the daemon socket to become connectable.
const DAEMON_STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
/// How often to poll the socket during startup.
const DAEMON_POLL_INTERVAL: Duration = Duration::from_millis(100);

pub fn find_binary(name: &str) -> PathBuf {
    let mut bin = std::env::current_exe().unwrap();
    bin.pop();
    bin.pop();
    bin.push(name);
    bin
}

fn ensure_daemon() {
    DAEMON.call_once(|| {
        use std::os::unix::net::UnixStream;

        let socket = fff_ipc_domain::daemon_socket_path();
        if UnixStream::connect(&socket).is_ok() {
            return;
        }

        let bin = find_binary("fff-daemon");
        assert!(
            bin.exists(),
            "fff-daemon not found at {}. Run: cargo build -p fff-daemon",
            bin.display()
        );

        Command::new(&bin)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap_or_else(|e| panic!("failed to spawn fff-daemon: {e}"));

        let deadline = std::time::Instant::now() + DAEMON_STARTUP_TIMEOUT;
        while std::time::Instant::now() < deadline {
            std::thread::sleep(DAEMON_POLL_INTERVAL);
            if UnixStream::connect(&socket).is_ok() {
                return;
            }
        }
        panic!("fff-daemon did not start within {DAEMON_STARTUP_TIMEOUT:?}");
    });
}

pub struct Dir {
    pub(crate) dir: PathBuf,
}

impl Dir {
    pub fn new(name: &str) -> Self {
        ensure_daemon();

        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let dir = std::env::temp_dir().join("fff-rg-tests").join(format!("{name}-{pid}-{id}"));

        if dir.exists() {
            let _ = fs::remove_dir_all(&dir);
        }
        fs::create_dir_all(&dir).unwrap();

        Command::new("git")
            .args(["init", "-q"])
            .current_dir(&dir)
            .output()
            .expect("git init failed");

        Self { dir }
    }

    pub fn create(&self, name: &str, contents: &str) {
        let path = self.dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    pub fn command(&self) -> TestCommand {
        let bin = find_binary("fff-rg");
        let mut cmd = Command::new(&bin);
        cmd.current_dir(&self.dir);
        TestCommand { cmd, dir: self.dir.clone() }
    }

    pub fn rg(&self) -> TestCommand {
        let mut cmd = Command::new("rg");
        cmd.current_dir(&self.dir);
        TestCommand { cmd, dir: self.dir.clone() }
    }

    pub fn with_project(&self, hay: &crate::hay::Hay) -> &Self {
        self.create("src/main.rs", hay.rust_main);
        self.create("src/lib.rs", hay.rust_lib);
        self.create("tests/config_test.rs", hay.rust_test);
        self.create("config.json", hay.json_config);
        self.create("README.md", hay.unicode_readme);
        self.create("src/indented.rs", hay.indented);
        self.create("data/repeated.txt", hay.repeated);
        self.create("data/no_newline.txt", hay.no_newline);
        self.create("empty.txt", "");
        self
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

pub struct TestCommand {
    cmd: Command,
    dir: PathBuf,
}

impl TestCommand {
    pub fn arg(&mut self, arg: &str) -> &mut Self {
        self.cmd.arg(arg);
        self
    }

    pub fn args(&mut self, args: &[&str]) -> &mut Self {
        self.cmd.args(args);
        self
    }

    pub fn stdout(&mut self) -> String {
        let output = self
            .cmd
            .output()
            .unwrap_or_else(|e| panic!("failed to run fff-rg: {e}\ndir: {}", self.dir.display()));
        String::from_utf8(output.stdout).unwrap()
    }

    pub fn exit_code(&mut self) -> i32 {
        let output = self.cmd.output().unwrap();
        output.status.code().unwrap_or(-1)
    }

    pub fn full_output(&mut self) -> Output {
        let o = self
            .cmd
            .output()
            .unwrap_or_else(|e| panic!("failed to run command: {e}\ndir: {}", self.dir.display()));
        Output {
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
            code: o.status.code().unwrap_or(-1),
        }
    }
}

pub struct Output {
    pub stdout: String,
    #[allow(dead_code)]
    pub stderr: String,
    pub code: i32,
}

pub fn normalize_inline(raw: &str) -> String {
    let trailing = raw.ends_with('\n');
    let mut lines: Vec<&str> = raw.lines().collect();
    lines.sort();
    let mut out = lines.join("\n");
    if trailing {
        out.push('\n');
    }
    out
}

pub fn normalize_heading(raw: &str) -> String {
    let trailing = raw.ends_with('\n');
    // Collapse runs of 2+ newlines to exactly \n\n (rg vs fff-rg differ in blank line counts)
    let mut collapsed = String::with_capacity(raw.len());
    let mut newline_run = 0usize;
    for ch in raw.chars() {
        if ch == '\n' {
            newline_run += 1;
        } else {
            if newline_run >= 2 {
                collapsed.push_str("\n\n");
            } else {
                for _ in 0..newline_run {
                    collapsed.push('\n');
                }
            }
            newline_run = 0;
            collapsed.push(ch);
        }
    }
    if newline_run >= 2 {
        collapsed.push_str("\n\n");
    } else {
        for _ in 0..newline_run {
            collapsed.push('\n');
        }
    }

    let mut blocks: Vec<&str> = collapsed
        .split("\n\n")
        .map(|b| b.trim_matches('\n'))
        .filter(|b| !b.is_empty())
        .collect();
    blocks.sort();
    let mut out = blocks.join("\n\n");
    if trailing && !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

pub fn assert_rg_match(dir: &Dir, args: &[&str], heading: bool) {
    let fff_out = dir.command().args(args).full_output();
    let rg_out = dir.rg().args(args).full_output();

    assert_eq!(
        fff_out.code, rg_out.code,
        "exit code mismatch for args {args:?}\nfff-rg stdout:\n{}\nrg stdout:\n{}",
        fff_out.stdout, rg_out.stdout,
    );

    let normalize: fn(&str) -> String = if heading { normalize_heading } else { normalize_inline };

    let fff_normalized = normalize(&fff_out.stdout);
    let rg_normalized = normalize(&rg_out.stdout);

    assert_eq!(
        fff_normalized, rg_normalized,
        "stdout mismatch for args {args:?}\nfff-rg raw:\n{}\nrg raw:\n{}",
        fff_out.stdout, rg_out.stdout,
    );
}
