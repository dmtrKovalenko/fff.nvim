mod background_watcher;
pub use background_watcher::*;

mod watch;
pub use watch::*;

// The harness reads rescan counters, which release builds compile out.
#[cfg(all(test, rescan_stats))]
mod rescan_tests;
