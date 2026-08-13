use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_fff-mcp");

#[test]
fn stays_alive_while_parent_alive_despite_idle_timeout() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "hello").unwrap();

    let mut child = Command::new(BIN)
        .arg(dir.path())
        .args([
            "--no-update-check",
            "--no-warmup",
            "--no-watch",
            "--idle-timeout-secs",
            "1",
        ])
        .arg("--log-file")
        .arg(dir.path().join("test.log"))
        .env("FFF_MCP_TEST_WATCHDOG_INTERVAL_MS", "100")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let mut stdin = child.stdin.take().unwrap();
    let stdout_lines = spawn_line_reader(child.stdout.take().unwrap());
    do_handshake(&mut stdin, &stdout_lines);

    // Wait past the idle timeout and several watchdog ticks.
    std::thread::sleep(Duration::from_secs(2));
    assert!(
        child.try_wait().unwrap().is_none(),
        "fff-mcp exited on idle timeout even though its parent is alive"
    );

    // Closing stdin ends the transport; the server must still shut down cleanly.
    drop(stdin);
    wait_for_exit(&mut child, Duration::from_secs(15));
}

#[cfg(unix)]
#[test]
fn exits_when_parent_dies_even_without_idle_timeout() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "hello").unwrap();
    let log_path = dir.path().join("test.log");
    let exit_signal = dir.path().join("exit-parent");

    // Intermediary parent: sh backgrounds fff-mcp and waits until the handshake
    // completes before dying and orphaning it.
    let mut sh = Command::new("sh")
        .arg("-c")
        .arg(
            // Preserve stdin before POSIX shells assign /dev/null to background jobs.
            r#"exec 3<&0
                "$1" "$2" --no-update-check --no-warmup --no-watch \
                --idle-timeout-secs 0 --log-file "$3" <&3 &
                while [ ! -e "$4" ]; do sleep 0.1; done"#,
        )
        .arg("sh")
        .arg(BIN)
        .arg(dir.path())
        .arg(&log_path)
        .arg(&exit_signal)
        .env("FFF_MCP_TEST_WATCHDOG_INTERVAL_MS", "100")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let mut stdin = sh.stdin.take().unwrap();
    let stdout_lines = spawn_line_reader(sh.stdout.take().unwrap());
    do_handshake(&mut stdin, &stdout_lines);

    std::fs::write(exit_signal, "").unwrap();
    sh.wait().unwrap();

    // We still hold the stdin write end, so the only exit path is the parent
    // liveness check. EOF on stdout means fff-mcp closed it by exiting.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match stdout_lines.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(_) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                panic!("fff-mcp did not exit within 5s of its parent dying")
            }
        }
    }
    drop(stdin);

    let logs = read_session_logs(dir.path());
    assert!(
        logs.contains("Parent process") && logs.contains("exited, shutting down"),
        "expected parent-death exit reason in logs, got:\n{}",
        logs
    );
}

fn do_handshake(stdin: &mut ChildStdin, stdout_lines: &mpsc::Receiver<String>) {
    let initialize = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "parent-liveness-test", "version": "0.0.0" }
        }
    });
    writeln!(stdin, "{}", initialize).unwrap();
    stdin.flush().unwrap();

    let response = stdout_lines
        .recv_timeout(Duration::from_secs(30))
        .expect("no initialize response within 30s");
    assert!(
        response.contains("\"serverInfo\""),
        "unexpected initialize response: {}",
        response
    );

    writeln!(
        stdin,
        "{}",
        serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })
    )
    .unwrap();
    stdin.flush().unwrap();
}

fn spawn_line_reader(stdout: std::process::ChildStdout) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(line) => {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    rx
}

fn wait_for_exit(child: &mut Child, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if child.try_wait().unwrap().is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    child.kill().ok();
    panic!(
        "fff-mcp did not exit within {:?} after stdin closed",
        timeout
    );
}

#[cfg(unix)]
fn read_session_logs(dir: &std::path::Path) -> String {
    let mut combined = String::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with("test") && name.ends_with(".log") {
            combined.push_str(&std::fs::read_to_string(entry.path()).unwrap_or_default());
        }
    }
    combined
}
