use std::thread;

use crate::pool::{BufferPool, Shared};
use crate::size_class::ClassTable;

/// Default minimum buffer size (4KB) — smallest poolable size class.
const DEFAULT_MIN_BUFFER_SIZE: usize = 4 * 1024;

/// Calculate optimal TLS cache size per size class based on CPU count.
const fn default_tls_cache_size(num_cpus: usize) -> usize {
    match num_cpus {
        0..=2 => 2,
        3..=4 => 4,
        5..=8 => 6,
        _ => 8,
    }
}

/// Calculate max buffers per size class based on system parallelism.
const fn default_max_buffers_per_class(num_cpus: usize) -> usize {
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
const fn default_batch_size(tls_cache_size: usize) -> usize {
    let half = tls_cache_size / 2;
    if half < 2 { 2 } else { half }
}

/// Builder for configuring a [`BufferPool`].
///
/// # Example
/// ```
/// use zeropool::BufferPool;
///
/// let pool = BufferPool::builder()
///     .min_buffer_size(4096)
///     .tls_cache_size(4)
///     .build();
/// ```
#[derive(Debug, Clone, Default)]
pub struct Builder {
    tls_cache_size: Option<usize>,
    max_buffers_per_class: Option<usize>,
    min_buffer_size: Option<usize>,
    pinned_memory: Option<bool>,
    batch_size: Option<usize>,
}

impl Builder {
    /// Set the minimum buffer size to keep in the pool.
    ///
    /// Buffers smaller than this are discarded when returned.
    /// Default: 4KB
    pub fn min_buffer_size(mut self, size: usize) -> Self {
        self.min_buffer_size = Some(size);
        self
    }

    /// Set the number of buffers kept in thread-local cache per size class.
    ///
    /// Higher values reduce shared pool access but increase per-thread memory.
    /// Default: 2–8 based on CPU count
    pub fn tls_cache_size(mut self, size: usize) -> Self {
        self.tls_cache_size = Some(size);
        self
    }

    /// Set the maximum number of buffers per size class in the shared pool.
    ///
    /// Controls how many buffers of each size the shared pool retains.
    /// Default: 32–128 based on CPU count
    pub fn max_buffers_per_class(mut self, count: usize) -> Self {
        self.max_buffers_per_class = Some(count);
        self
    }

    /// Enable pinned memory (mlock) for pooled buffers.
    ///
    /// Locks buffers in RAM to prevent swapping. Useful for latency-critical
    /// or security-sensitive applications. Falls back gracefully if pinning fails.
    /// Default: false
    pub fn pinned_memory(mut self, enabled: bool) -> Self {
        self.pinned_memory = Some(enabled);
        self
    }

    /// Set the batch size for TLS ↔ shared pool transfers.
    ///
    /// When a thread-local cache misses, this many buffers are moved at once
    /// from the shared pool (magazine-style). Larger values amortize access
    /// cost but increase per-transfer latency.
    /// Default: half of TLS cache size (min 2)
    pub fn batch_size(mut self, size: usize) -> Self {
        self.batch_size = Some(size);
        self
    }

    /// Build the [`BufferPool`] with the configured settings.
    ///
    /// Any settings not explicitly set use system-aware defaults.
    ///
    /// # Panics
    ///
    /// Panics if `tls_cache_size` is 0 or `max_buffers_per_class` is 0.
    pub fn build(self) -> BufferPool {
        let num_cpus = thread::available_parallelism().map_or(4, std::num::NonZero::get);

        let tls_cache_size =
            self.tls_cache_size.unwrap_or_else(|| default_tls_cache_size(num_cpus));

        let max_buffers_per_class = self
            .max_buffers_per_class
            .unwrap_or_else(|| default_max_buffers_per_class(num_cpus));

        assert!(tls_cache_size > 0, "tls_cache_size must be > 0");
        assert!(max_buffers_per_class > 0, "max_buffers_per_class must be > 0");

        let batch_size = self.batch_size.unwrap_or_else(|| default_batch_size(tls_cache_size));

        BufferPool::from_shared(Shared {
            id: BufferPool::next_id(),
            table: ClassTable::new(max_buffers_per_class),
            tls_cache_size,
            min_buffer_size: self.min_buffer_size.unwrap_or(DEFAULT_MIN_BUFFER_SIZE),
            pinned_memory: self.pinned_memory.unwrap_or(false),
            batch_size,
        })
    }
}
