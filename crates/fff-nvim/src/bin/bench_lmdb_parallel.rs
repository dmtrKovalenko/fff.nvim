/// Parallel LMDB contention bench.
///
/// Spawns N child processes that all open the same frecency + query-tracker
/// dbs and hammer them. Used to measure the cost (or absence of cost) of
/// dropping the `MDB_NOLOCK | NO_SYNC | NO_META_SYNC` env flags.
///
/// Usage:
///   cargo build --release --bin bench_lmdb_parallel
///   ./target/release/bench_lmdb_parallel --procs 4 --iters 5000
///
/// Env var FFF_BENCH_ROLE=worker turns the binary into a worker that talks
/// to a db path passed via FFF_BENCH_DB.
use fff::frecency::FrecencyTracker;
use fff::query_tracker::QueryTracker;
use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

fn parse_args() -> (usize, usize, Option<String>) {
    let mut procs = 4usize;
    let mut iters = 2000usize;
    let mut db_path: Option<String> = None;
    let mut args = env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--procs" => procs = args.next().and_then(|s| s.parse().ok()).unwrap_or(procs),
            "--iters" => iters = args.next().and_then(|s| s.parse().ok()).unwrap_or(iters),
            "--db" => db_path = args.next(),
            _ => {}
        }
    }
    (procs, iters, db_path)
}

fn worker_main(db: &Path, iters: usize, worker_id: u32) {
    let frecency_path = db.join("frecency");
    let history_path = db.join("history");

    let frecency = FrecencyTracker::open(&frecency_path).expect("frecency open");
    let mut query_tracker = QueryTracker::open(&history_path).expect("query tracker open");

    let project = PathBuf::from("/bench/project");
    let started = Instant::now();

    for i in 0..iters {
        let file = PathBuf::from(format!(
            "/bench/project/src/file_{}_{}.rs",
            worker_id,
            i % 256
        ));
        // Frecency write path — same call as track_access on BufEnter.
        frecency.track_access(&file).expect("frecency track_access");

        // Query tracker write path — same as track_query_completion on file open.
        let query = format!("q{}", i % 32);
        query_tracker
            .track_query_completion(&query, &project, &file)
            .expect("query tracker track_query_completion");

        // A read every few iterations.
        if i % 8 == 0 {
            let _ = frecency.seconds_since_last_access(&file).ok();
            let _ = query_tracker
                .get_historical_query(&project, 0)
                .ok()
                .flatten();
        }
    }

    let elapsed = started.elapsed();
    eprintln!(
        "worker {} done: {} iters in {:?} ({:.0} ops/s)",
        worker_id,
        iters,
        elapsed,
        iters as f64 * 3.0 / elapsed.as_secs_f64()
    );
}

fn driver_main(procs: usize, iters: usize, db_override: Option<String>) {
    let db = db_override.map(PathBuf::from).unwrap_or_else(|| {
        std::env::temp_dir().join(format!("fff_bench_lmdb_{}", std::process::id()))
    });
    let _ = std::fs::remove_dir_all(&db);
    std::fs::create_dir_all(&db).expect("create db dir");

    let exe = env::current_exe().expect("current_exe");

    eprintln!(
        "driver: launching {} workers, {} iters each, db={}",
        procs,
        iters,
        db.display()
    );

    let started = Instant::now();
    let mut children = Vec::with_capacity(procs);
    for worker_id in 0..procs {
        let child = Command::new(&exe)
            .env("FFF_BENCH_ROLE", "worker")
            .env("FFF_BENCH_DB", &db)
            .env("FFF_BENCH_ITERS", iters.to_string())
            .env("FFF_BENCH_WORKER_ID", worker_id.to_string())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn worker");
        children.push(child);
    }

    let mut failures = 0u32;
    for mut child in children {
        let status = child.wait().expect("wait");
        if !status.success() {
            failures += 1;
        }
    }

    let elapsed = started.elapsed();
    let total_ops = procs as f64 * iters as f64 * 3.0;
    eprintln!(
        "driver: all workers done in {:?}, ~{:.0} ops/s aggregate, failures={}",
        elapsed,
        total_ops / elapsed.as_secs_f64(),
        failures
    );

    let _ = std::fs::remove_dir_all(&db);
}

fn main() {
    // Worker branch: spawned by driver to contend on the same db.
    if env::var("FFF_BENCH_ROLE").as_deref() == Ok("worker") {
        let db = env::var("FFF_BENCH_DB").expect("FFF_BENCH_DB");
        let iters: usize = env::var("FFF_BENCH_ITERS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1000);
        let worker_id: u32 = env::var("FFF_BENCH_WORKER_ID")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        worker_main(Path::new(&db), iters, worker_id);
        return;
    }

    let (procs, iters, db) = parse_args();
    driver_main(procs, iters, db);
}
