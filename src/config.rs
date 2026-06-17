//! Pool configuration defaults and system-aware tuning.

use std::thread;

/// Default minimum buffer size (4KB) — smallest poolable size class.
pub(crate) const DEFAULT_MIN_BUFFER_SIZE: usize = 4 * 1024;

/// Calculate optimal TLS cache size per size class based on CPU count.
pub(crate) const fn default_tls_cache_size(num_cpus: usize) -> usize {
    match num_cpus {
        0..=2 => 2,
        3..=4 => 4,
        5..=8 => 6,
        _ => 8,
    }
}

/// Calculate max buffers per size class based on system parallelism.
pub(crate) const fn default_max_buffers_per_class(num_cpus: usize) -> usize {
    const BASE: usize = 32;
    let scaling = match num_cpus {
        0..16 => 1,
        16..32 => 2,
        32..64 => 3,
        _ => 4,
    };
    BASE * scaling
}

/// Calculate default batch transfer size based on TLS cache size.
pub(crate) const fn default_batch_size(tls_cache_size: usize) -> usize {
    let half = tls_cache_size / 2;
    if half < 2 { 2 } else { half }
}

/// Detect system CPU count, falling back to 4.
pub(crate) fn cpu_count() -> usize {
    thread::available_parallelism().map_or(4, std::num::NonZero::get)
}
