//! Dedicated rayon pools. The global pool spans every logical core, which
//! oversubscribes asymmetric chips (Apple P+E): E-cores are ~2× slower and
//! `open()` contends on a per-VFS lock past P-core count, so a larger pool is
//! slower on file-heavy work.

use std::sync::LazyLock;

/// Dedicated thread pool for background work (scan, warmup, bigram build).
pub static BACKGROUND_THREAD_POOL: LazyLock<rayon::ThreadPool> = LazyLock::new(|| {
    let total = std::thread::available_parallelism()
        .map(|p| p.get())
        .unwrap_or(4);

    // Background work is mostly syscall-bound; halving parallelism leaves
    // cores for search/UI at negligible throughput cost.
    let bg_threads = (total / 2).max(2);
    rayon::ThreadPoolBuilder::new()
        .num_threads(bg_threads)
        .thread_name(|i| format!("fff-bg-{i}"))
        .start_handler(|_| {
            // QoS pin keeps workers on P-cores; the kernel otherwise drifts
            // them to ~2× slower E-cores.
            #[cfg(target_os = "macos")]
            unsafe {
                let _ = libc::pthread_set_qos_class_self_np(
                    libc::qos_class_t::QOS_CLASS_USER_INITIATED,
                    0,
                );
            }
        })
        .build()
        .expect("failed to create background rayon pool")
});

/// Physical performance-core count via sysctl, falling back to logical cores.
/// On a 12P+4E M4 Max, grep runs 16t=6.2s vs 13t=4.9s — fewer threads win.
#[cfg(target_os = "macos")]
fn performance_core_count() -> usize {
    let mut count: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    let name = c"hw.perflevel0.physicalcpu";
    let ok = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            &mut count as *mut _ as *mut libc::c_void,
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if ok == 0 && count > 0 {
        count as usize
    } else {
        std::thread::available_parallelism()
            .map(|p| p.get())
            .unwrap_or(4)
    }
}

/// Pool for grep content search: P-core sized and QoS-pinned on macOS, full
/// parallelism elsewhere. Avoids E-core drag and VFS-lock contention.
pub static SEARCH_THREAD_POOL: LazyLock<rayon::ThreadPool> = LazyLock::new(|| {
    #[cfg(target_os = "macos")]
    let threads = performance_core_count();
    #[cfg(not(target_os = "macos"))]
    let threads = std::thread::available_parallelism()
        .map(|p| p.get())
        .unwrap_or(4);

    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(|i| format!("fff-search-{i}"))
        .start_handler(|_| {
            #[cfg(target_os = "macos")]
            unsafe {
                let _ = libc::pthread_set_qos_class_self_np(
                    libc::qos_class_t::QOS_CLASS_USER_INITIATED,
                    0,
                );
            }
        })
        .build()
        .expect("failed to create search rayon pool")
});
