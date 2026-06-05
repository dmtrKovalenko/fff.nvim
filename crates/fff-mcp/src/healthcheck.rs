use crate::Args;
use git2::Repository;

fn check(label: &str, ok: bool, detail: &str) -> bool {
    let marker = if ok { "+" } else { "x" };
    println!("  [{marker}] {label}: {detail}");
    ok
}

fn warn(label: &str, detail: &str) {
    println!("  [!] {label}: {detail}");
}

pub fn run_healthcheck(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    let version = concat!(env!("CARGO_PKG_VERSION"), " (", env!("FFF_GIT_HASH"), ")");
    println!("fff-mcp {version}\n");

    let mut all_ok = true;

    // 1. Base path
    let base_path = args.base_path.clone().unwrap_or_else(|| {
        std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string()
    });

    let path_exists = std::path::Path::new(&base_path).is_dir();
    all_ok &= check(
        "Base path",
        path_exists,
        if path_exists { &base_path } else { "directory does not exist" },
    );

    // 2. Git repository
    match Repository::discover(&base_path) {
        Ok(repo) => {
            if let Some(workdir) = repo.workdir() {
                all_ok &= check("Git repository", true, &format!("{}", workdir.display()));
            } else {
                all_ok &= check("Git repository", true, "bare repository");
            }
        }
        Err(_) => {
            warn(
                "Git repository",
                "not found (fff-mcp will still work, but git-status features are disabled)",
            );
        }
    }

    // 3. Daemon socket connectivity (Unix proxy path)
    #[cfg(unix)]
    {
        use crate::client::{EngineClient, HealthStatus};
        let base = std::path::Path::new(&base_path);
        match EngineClient::check_health(base) {
            HealthStatus::Ok => {
                all_ok &= check("fff-engine daemon", true, "reachable via socket");
            }
            HealthStatus::NotStarted(sock) => {
                // Not an error — daemon starts lazily on first tool call.
                warn(
                    "fff-engine daemon",
                    &format!("not yet started (socket {} absent — will be spawned on first use)", sock.display()),
                );
            }
            HealthStatus::ConnRefused(e) => {
                all_ok &= check(
                    "fff-engine daemon",
                    false,
                    &format!("socket exists but connection refused: {e}"),
                );
            }
        }
    }

    // 4. Log file
    if let Some(ref log_path) = args.log_file {
        let parent_ok = std::path::Path::new(log_path)
            .parent()
            .is_some_and(|p| p.is_dir());
        all_ok &= check(
            "Log file",
            parent_ok,
            if parent_ok { log_path } else { "parent directory does not exist" },
        );
    } else {
        check("Log file", false, "path not resolved");
    }

    if all_ok {
        println!("All checks passed.");
        Ok(())
    } else {
        Err("Some checks failed — review the items marked [x] above.".into())
    }
}
