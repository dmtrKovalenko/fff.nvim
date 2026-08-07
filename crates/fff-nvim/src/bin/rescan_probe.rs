use fff::file_picker::FilePicker;
use fff::{
    FFFMode, FilePickerOptions, RESCAN_STATS_ENABLED, RescanReason, RescanStats, SharedFilePicker,
    SharedFrecency,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(250);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (base_path, run_for) = parse_args()?;

    if !RESCAN_STATS_ENABLED {
        return Err(
            "this build has rescan accounting compiled out; rebuild with \
                    `--features rescan-stats` (or drop `--release`)"
                .into(),
        );
    }

    let picker = SharedFilePicker::default();
    let frecency = SharedFrecency::noop();

    println!("indexing {base_path} ...");
    let started = Instant::now();
    FilePicker::new_with_shared_state(
        picker.clone(),
        frecency.clone(),
        FilePickerOptions {
            base_path: base_path.clone(),
            enable_mmap_cache: false,
            mode: FFFMode::default(),
            watch: true,
            ..Default::default()
        },
    )?;

    if !picker.wait_for_scan(Duration::from_secs(600)) {
        return Err("timed out waiting for the initial scan".into());
    }
    if !picker.wait_for_watcher(Duration::from_secs(600)) {
        return Err("timed out waiting for the watcher".into());
    }

    println!(
        "indexed {} files in {:.2}s; watching for rescan requests.\n",
        live_files(&picker),
        started.elapsed().as_secs_f64()
    );
    picker.reset_rescan_stats();

    let running = Arc::new(AtomicBool::new(true));
    let stop = Arc::clone(&running);
    ctrlc::set_handler(move || stop.store(false, Ordering::SeqCst))?;

    let watching_since = Instant::now();
    let mut last = RescanStats::default();

    while running.load(Ordering::SeqCst) {
        std::thread::sleep(POLL);

        let stats = picker.rescan_stats();
        let delta = stats.since(&last);
        if delta.total > 0 || delta.throttled > 0 {
            let now = watching_since.elapsed().as_secs_f64();
            let files = live_files(&picker);
            let overflow = overflow_files(&picker);

            for reason in RescanReason::ALL {
                for _ in 0..delta.count(reason) {
                    println!(
                        "[{now:>8.2}s] request   {reason:<21} files={files} overflow={overflow}"
                    );
                }
                let suppressed = delta.count_throttled(reason);
                if suppressed > 0 {
                    println!("[{now:>8.2}s] throttled {reason:<21} x{suppressed}");
                }
            }
            last = stats;
        }

        if run_for.is_some_and(|limit| watching_since.elapsed() >= limit) {
            break;
        }
    }

    let elapsed = watching_since.elapsed();
    let stats = picker.rescan_stats();
    println!("\n{:.1}s watched", elapsed.as_secs_f64());
    println!("{stats}");
    if stats.watcher_triggered() > 0 {
        println!(
            "{:.1} watcher rescan requests/minute",
            stats.watcher_triggered() as f64 / elapsed.as_secs_f64().max(1.0) * 60.0
        );
    } else {
        println!("no full rescans: every change was applied incrementally");
    }
    if stats.throttled > 0 {
        println!(
            "{} additional request(s) were throttled; {} total requests observed",
            stats.throttled,
            stats.total + stats.throttled
        );
    }

    if let Ok(mut guard) = picker.write() {
        guard.take();
    }

    Ok(())
}

fn parse_args() -> Result<(String, Option<Duration>), Box<dyn std::error::Error>> {
    let mut base_path = None;
    let mut run_for = None;
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--seconds" | "-s" => {
                let value = args.next().ok_or("--seconds needs a value")?;
                run_for = Some(Duration::from_secs(value.parse()?));
            }
            "--help" | "-h" => {
                println!("usage: rescan_probe [path] [--seconds N]");
                std::process::exit(0);
            }
            other => base_path = Some(other.to_string()),
        }
    }

    let base_path = match base_path {
        Some(path) => path,
        None => std::env::current_dir()?.to_string_lossy().into_owned(),
    };

    Ok((base_path, run_for))
}

fn live_files(picker: &SharedFilePicker) -> usize {
    picker
        .read()
        .ok()
        .and_then(|g| g.as_ref().map(|p| p.live_file_count()))
        .unwrap_or(0)
}

fn overflow_files(picker: &SharedFilePicker) -> usize {
    picker
        .read()
        .ok()
        .and_then(|g| g.as_ref().map(|p| p.get_overflow_files().len()))
        .unwrap_or(0)
}
