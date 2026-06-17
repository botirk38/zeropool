use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::metrics::{Counters, PoolStats, snapshot};
use crate::size_class::{ClassTable, SizeClass};
use crate::tls::TlsState;

/// Global counter for unique pool instance IDs.
static NEXT_POOL_ID: AtomicU64 = AtomicU64::new(1);

/// Core state backing all [`BufferPool`] handles.
///
/// Holds identity, class routing table, and runtime configuration.
/// No methods — all behavior lives on [`BufferPool`].
#[derive(Debug)]
pub(crate) struct PoolState {
    /// Unique pool ID — prevents TLS cache cross-contamination.
    pub id: u64,
    /// Size-class routing table and lock-free queues.
    pub table: ClassTable,
    /// Max buffers per class in each thread-local cache.
    pub tls_cache_size: usize,
    /// Minimum buffer capacity to keep in the pool.
    pub min_buffer_size: usize,
    /// Whether to mlock buffer memory.
    pub pinned_memory: bool,
    /// Batch size for TLS ↔ shared pool magazine transfers.
    pub batch_size: usize,
    /// Performance counters.
    pub counters: Counters,
}

/// A high-performance, thread-safe buffer pool with size-class bucketing.
///
/// # Architecture
///
/// ```text
/// Thread 1            Thread 2            Thread N
/// ┌────────────┐     ┌────────────┐     ┌────────────┐
/// │ TLS Cache  │     │ TLS Cache  │     │ TLS Cache  │  ← Lock-free
/// │ [class 0]  │     │ [class 0]  │     │ [class 0]  │    per-class
/// │ [class 1]  │     │ [class 1]  │     │ [class 1]  │    LIFO caches
/// │   ...      │     │   ...      │     │   ...      │
/// └─────┬──────┘     └─────┬──────┘     └─────┬──────┘
///       │ batch             │ batch             │ batch
///       └──────────┬───────┴───────────────────┘
///                  │
///          ┌───────▼────────┐
///          │  Shared Pool   │
///          │ (lock-free)    │
///          │                │
///          │ [4KB  queue]   │  ArrayQueue per class
///          │ [16KB queue]   │  CAS-based push/pop
///          │ [64KB queue]   │  No mutex needed
///          │ [256KB queue]  │
///          │ [1MB  queue]   │
///          │ [4MB  queue]   │
///          │ [16MB queue]   │
///          │ [64MB queue]   │
///          └────────────────┘
/// ```
#[derive(Clone, Debug)]
pub struct BufferPool {
    pub(crate) state: Arc<PoolState>,
}

impl BufferPool {
    /// Create a new buffer pool with system-aware defaults.
    ///
    /// Chain configuration methods to customize before use:
    ///
    /// ```
    /// use zeropool::BufferPool;
    ///
    /// // Defaults
    /// let pool = BufferPool::new();
    ///
    /// // Custom
    /// let pool = BufferPool::new()
    ///     .min_buffer_size(4096)
    ///     .tls_cache_size(8)
    ///     .max_buffers_per_class(64)
    ///     .batch_size(4);
    /// ```
    pub fn new() -> Self {
        use crate::config::{
            DEFAULT_MIN_BUFFER_SIZE, cpu_count, default_batch_size, default_max_buffers_per_class,
            default_tls_cache_size,
        };
        let cpus = cpu_count();
        let tls = default_tls_cache_size(cpus);
        Self {
            state: Arc::new(PoolState {
                id: NEXT_POOL_ID.fetch_add(1, Ordering::Relaxed),
                table: ClassTable::new(default_max_buffers_per_class(cpus)),
                tls_cache_size: tls,
                min_buffer_size: DEFAULT_MIN_BUFFER_SIZE,
                pinned_memory: false,
                batch_size: default_batch_size(tls),
                counters: Counters::new(),
            }),
        }
    }

    /// Set the minimum buffer size to keep in the pool.
    ///
    /// Buffers smaller than this are discarded when returned.
    /// Default: 4KB
    pub fn min_buffer_size(self, size: usize) -> Self {
        self.rebuild(|s| s.min_buffer_size = size)
    }

    /// Set the number of buffers kept in thread-local cache per size class.
    ///
    /// Higher values reduce shared pool access but increase per-thread memory.
    /// Default: 2–8 based on CPU count
    pub fn tls_cache_size(self, size: usize) -> Self {
        assert!(size > 0, "tls_cache_size must be > 0");
        self.rebuild(|s| s.tls_cache_size = size)
    }

    /// Set the maximum number of buffers per size class in the shared pool.
    ///
    /// Default: 32–128 based on CPU count
    pub fn max_buffers_per_class(self, count: usize) -> Self {
        assert!(count > 0, "max_buffers_per_class must be > 0");
        self.rebuild(|s| s.table = ClassTable::new(count))
    }

    /// Enable pinned memory (mlock) for pooled buffers.
    ///
    /// Locks buffers in RAM to prevent swapping.
    /// Default: false
    pub fn pinned_memory(self, enabled: bool) -> Self {
        self.rebuild(|s| s.pinned_memory = enabled)
    }

    /// Set the batch size for TLS ↔ shared pool transfers.
    ///
    /// When a thread-local cache misses, this many buffers are moved at once
    /// from the shared pool (magazine-style).
    /// Default: half of TLS cache size (min 2)
    pub fn batch_size(self, size: usize) -> Self {
        self.rebuild(|s| s.batch_size = size)
    }

    /// Apply a mutation to the inner state, reconstructing the Arc.
    ///
    /// Only valid at construction time (single owner).
    fn rebuild(mut self, f: impl FnOnce(&mut PoolState)) -> Self {
        let state = Arc::get_mut(&mut self.state)
            .expect("cannot reconfigure a shared pool — call config methods before cloning");
        f(state);
        self
    }

    /// Get a buffer of at least the specified size from the pool.
    ///
    /// Returns a [`PooledBuffer`](crate::PooledBuffer) that automatically
    /// returns to the pool on drop.
    ///
    /// # Performance
    ///
    /// 1. **Fastest**: TLS cache pop (lock-free, ~8–60ns)
    /// 2. **Fast**: Batch refill from shared pool (lock-free CAS)
    /// 3. **Fallback**: Fresh allocation via `SizeClass::allocate`
    ///
    /// # Example
    /// ```
    /// use zeropool::BufferPool;
    ///
    /// let pool = BufferPool::new();
    /// let mut buffer = pool.get(1024);
    /// buffer[0] = 42;
    /// ```
    #[inline]
    #[must_use]
    pub fn get(&self, size: usize) -> crate::PooledBuffer {
        self.state.counters.gets.fetch_add(1, Ordering::Relaxed);

        let Some((class_idx, class)) = self.state.table.route(size) else {
            self.state.counters.oversize.fetch_add(1, Ordering::Relaxed);
            self.state.counters.allocations.fetch_add(1, Ordering::Relaxed);
            return crate::PooledBuffer::new(ClassTable::oversize(size), self.clone(), u8::MAX);
        };

        // ── TLS fast path (lock-free) ──────────────────────────────
        let tls_result = TlsState::with(|state| {
            if !state.owns(self.state.id) {
                state.bind(self.state.id, self.state.tls_cache_size);
            }

            if let Some(buf) = state.caches[class_idx].pop() {
                return Some((buf, true)); // true = TLS hit
            }

            state.refill(class_idx, class, self.state.batch_size).map(|buf| (buf, false)) // false = shared pool hit
        });

        let ci = class_idx as u8;

        if let Some((mut buf, from_tls)) = tls_result {
            if from_tls {
                self.state.counters.tls_hits.fetch_add(1, Ordering::Relaxed);
            } else {
                self.state.counters.shared_hits.fetch_add(1, Ordering::Relaxed);
            }
            SizeClass::resize(&mut buf, size);
            return crate::PooledBuffer::new(buf, self.clone(), ci);
        }

        // ── Cold path: fresh allocation ────────────────────────────
        self.state.counters.allocations.fetch_add(1, Ordering::Relaxed);
        crate::PooledBuffer::new(class.allocate(size), self.clone(), ci)
    }

    /// Return a buffer to the pool for reuse.
    ///
    /// `class_hint` is the class index stored in [`PooledBuffer`](crate::PooledBuffer)
    /// at allocation time (`u8::MAX` for oversize buffers that bypass pooling).
    #[inline(always)]
    pub(crate) fn put(&self, mut buffer: Vec<u8>, class_hint: u8) {
        self.state.counters.puts.fetch_add(1, Ordering::Relaxed);
        buffer.clear();

        if class_hint == u8::MAX {
            self.state.counters.discards.fetch_add(1, Ordering::Relaxed);
            return;
        }

        let cap = buffer.capacity();

        if cap < self.state.min_buffer_size {
            self.state.counters.discards.fetch_add(1, Ordering::Relaxed);
            return;
        }

        // Use the stored class index. If the buffer was grown via
        // io::Write, reclassify so it lands in the right queue.
        let class_idx = if cap >= ClassTable::boundary(class_hint as usize) {
            class_hint as usize
        } else {
            let Some((idx, _)) = self.state.table.route_capacity(cap) else {
                return;
            };
            idx
        };

        self.pin(&mut buffer);

        // ── TLS fast path ──────────────────────────────────────────
        let overflow = TlsState::with(|state| {
            if !state.owns(self.state.id) {
                state.bind(self.state.id, self.state.tls_cache_size);
            }

            let class = &self.state.table[class_idx];

            if state.caches[class_idx].len() >= state.limit {
                state.spill(class_idx, class, self.state.batch_size);
            }

            if state.caches[class_idx].len() < state.limit {
                state.caches[class_idx].push(buffer);
                return None;
            }

            Some(buffer)
        });

        if let Some(buf) = overflow {
            let _ = self.state.table[class_idx].push(buf);
        }
    }

    /// Pre-allocate buffers in the pool for the given size class.
    ///
    /// Useful for warming up the pool before high-throughput operations.
    ///
    /// # Example
    /// ```
    /// use zeropool::BufferPool;
    ///
    /// let pool = BufferPool::new().min_buffer_size(0);
    /// pool.preallocate(16, 64 * 1024); // 16 × 64KB buffers
    /// ```
    pub fn preallocate(&self, count: usize, size: usize) {
        let Some((_, class)) = self.state.table.route(size) else {
            return;
        };

        for _ in 0..count {
            let mut buf = Vec::with_capacity(class.class_size);
            self.pin(&mut buf);
            if class.push(buf).is_err() {
                break;
            }
        }
    }

    /// Total number of buffers across all shared size classes.
    ///
    /// Does not include thread-local cached buffers.
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.state.table.total_buffered()
    }

    /// Whether all shared size classes are empty.
    ///
    /// Does not check thread-local caches.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.state.table.all_empty()
    }

    /// Drain all buffers from all shared size classes.
    ///
    /// Thread-local caches are NOT cleared.
    pub fn clear(&self) {
        self.state.table.clear_all();
    }

    /// Point-in-time snapshot of pool statistics.
    ///
    /// Includes aggregate counters (gets, hits, allocations, discards),
    /// derived rates, and per-class buffer counts.
    ///
    /// # Example
    /// ```
    /// use zeropool::BufferPool;
    ///
    /// let pool = BufferPool::new().min_buffer_size(0);
    /// let buf = pool.get(4096);
    /// drop(buf);
    ///
    /// let s = pool.stats();
    /// assert_eq!(s.gets, 1);
    /// assert_eq!(s.puts, 1);
    /// println!("{s}"); // human-readable summary
    /// ```
    #[inline]
    pub fn stats(&self) -> PoolStats {
        snapshot(&self.state.counters, self.state.table.classes())
    }

    /// Reset all performance counters to zero.
    pub fn reset_stats(&self) {
        self.state.counters.reset();
    }

    /// Pin buffer memory to RAM if configured.
    ///
    /// Encapsulates the full pin workflow: set len = capacity, mlock, clear.
    /// No-op when `pinned_memory` is false.
    #[inline(always)]
    fn pin(&self, buffer: &mut Vec<u8>) {
        if !self.state.pinned_memory {
            return;
        }
        if buffer.capacity() == 0 {
            return;
        }
        // SAFETY: capacity was allocated; all u8 patterns valid.
        // Temporarily set len = capacity so mlock sees the full allocation.
        unsafe { buffer.set_len(buffer.capacity()) };
        let _ = region::lock(buffer.as_ptr(), buffer.len());
        buffer.clear();
    }
}

impl Default for BufferPool {
    fn default() -> Self {
        Self::new()
    }
}
