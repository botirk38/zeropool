use std::cell::{Cell, RefCell};

use crate::pool::BufferEntry;

/// Thread-local cache structure combining buffers and config
pub(crate) struct TlsCache {
    pub buffers: Vec<BufferEntry>,
}

impl TlsCache {
    pub const fn new() -> Self {
        Self { buffers: Vec::new() }
    }
}

thread_local! {
    /// Thread-local cache for lock-free fast path
    /// Using RefCell for safe, runtime-checked borrowing
    pub(crate) static TLS_CACHE: RefCell<TlsCache> = const { RefCell::new(TlsCache::new()) };

    /// Thread-local limit for cache size
    /// Using Cell for zero-cost access to avoid repeated initialization checks
    pub(crate) static TLS_LIMIT: Cell<usize> = const { Cell::new(0) };

    /// Thread-local preferred shard index for cache affinity
    /// Each thread consistently uses the same shard for better cache locality
    /// Initialized lazily using thread ID hash
    pub(crate) static SHARD_AFFINITY: Cell<Option<usize>> = const { Cell::new(None) };
}
