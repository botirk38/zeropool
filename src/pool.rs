use std::sync::atomic::{AtomicU64, Ordering};

use crate::allocator::{Allocator, HeapAllocator};
use crate::size_class::{ClassTable, SizeClass};
use crate::stats::{Counters, Stats, snapshot};
use crate::tls::TlsState;

/// Global counter for unique pool instance IDs.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Shared state backing all [`ZeroPool`] handles.
///
/// Holds identity, class routing table, and runtime configuration.
/// All behavior lives on [`ZeroPool`].
#[derive(Debug)]
pub(crate) struct State {
    pub id: u64,
    pub table: ClassTable,
    pub tls_cache_size: usize,
    pub min_buffer_size: usize,
    pub pinned_memory: bool,
    pub batch_size: usize,
    pub track_stats: bool,
    pub counters: Counters,
    pub allocator: Box<dyn Allocator>,
}

/// A user-space byte allocator with size-class bucketing and thread-local caching.
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
#[derive(Debug)]
pub struct ZeroPool {
    pub(crate) state: State,
}

impl ZeroPool {
    /// Create a new allocator with system-aware defaults.
    ///
    /// Chain configuration methods to customize before use:
    ///
    /// ```
    /// use zeropool::ZeroPool;
    ///
    /// // Defaults
    /// let pool = ZeroPool::new();
    ///
    /// // Custom
    /// let pool = ZeroPool::new()
    ///     .min_buffer_size(4096)
    ///     .tls_cache_size(8)
    ///     .max_buffers_per_class(64)
    ///     .batch_size(4)
    ///     .track_stats(true);
    /// ```
    pub fn new() -> Self {
        use crate::config::{
            DEFAULT_MIN_BUFFER_SIZE, cpu_count, default_batch_size, default_max_buffers_per_class,
            default_tls_cache_size,
        };
        let cpus = cpu_count();
        let tls = default_tls_cache_size(cpus);
        Self {
            state: State {
                id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
                table: ClassTable::new(default_max_buffers_per_class(cpus)),
                tls_cache_size: tls,
                min_buffer_size: DEFAULT_MIN_BUFFER_SIZE,
                pinned_memory: false,
                batch_size: default_batch_size(tls),
                track_stats: false,
                counters: Counters::new(),
                allocator: Box::new(HeapAllocator),
            },
        }
    }

    /// Set a custom allocator for buffer creation.
    ///
    /// Default: [`HeapAllocator`] (standard `Vec::with_capacity`).
    ///
    /// ```
    /// use zeropool::{Allocator, ZeroPool};
    ///
    /// struct MyAllocator;
    /// impl Allocator for MyAllocator {
    ///     fn allocate(&self, capacity: usize) -> Vec<u8> {
    ///         Vec::with_capacity(capacity)
    ///     }
    /// }
    ///
    /// let pool = ZeroPool::new().allocator(MyAllocator);
    /// ```
    pub fn allocator(self, alloc: impl Allocator) -> Self {
        self.rebuild(|s| s.allocator = Box::new(alloc))
    }

    /// Set the minimum buffer size to keep in the pool.
    ///
    /// Buffers smaller than this are discarded on dealloc.
    /// Default: 4KB
    pub fn min_buffer_size(self, size: usize) -> Self {
        self.rebuild(|s| s.min_buffer_size = size)
    }

    /// Set the number of buffers kept in thread-local cache per size class.
    ///
    /// Higher values reduce shared pool access but increase per-thread memory.
    /// Also recomputes batch size (half of TLS cache, min 2) unless
    /// `.batch_size()` is called afterwards to override.
    /// Default: 2–8 based on CPU count
    pub fn tls_cache_size(self, size: usize) -> Self {
        assert!(size > 0, "tls_cache_size must be > 0");
        self.rebuild(|s| {
            s.tls_cache_size = size;
            s.batch_size = crate::config::default_batch_size(size);
        })
    }

    /// Set the maximum number of buffers per size class in the shared pool.
    ///
    /// Default: 32–128 based on CPU count
    pub fn max_buffers_per_class(self, count: usize) -> Self {
        assert!(count > 0, "max_buffers_per_class must be > 0");
        self.rebuild(|s| s.table = ClassTable::new(count))
    }

    /// Enable pinned memory (mlock) for allocated buffers.
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

    /// Enable or disable runtime statistics tracking.
    ///
    /// Disabled by default because hot-path atomic counters are measurable
    /// overhead in tight allocation loops. Enable this when you need
    /// [`stats()`](Self::stats) to report allocation counters.
    pub fn track_stats(self, enabled: bool) -> Self {
        self.rebuild(|s| s.track_stats = enabled)
    }

    fn rebuild(mut self, f: impl FnOnce(&mut State)) -> Self {
        f(&mut self.state);
        self
    }

    /// Allocate a buffer of at least `size` bytes.
    ///
    /// Returns a [`Buf`](crate::Buf) that automatically deallocates back
    /// to the pool on drop.
    ///
    /// # Performance
    ///
    /// 1. **Fastest**: TLS cache pop (lock-free, ~24ns)
    /// 2. **Fast**: Batch refill from shared pool (lock-free CAS)
    /// 3. **Cold**: Fresh allocation via the configured [`Allocator`]
    ///
    /// # Example
    /// ```
    /// use zeropool::ZeroPool;
    ///
    /// let pool = ZeroPool::new();
    /// let mut buf = pool.alloc(1024);
    /// buf[0] = 42;
    /// ```
    #[inline]
    #[must_use]
    pub fn alloc(&self, size: usize) -> crate::Buf<'_> {
        if self.state.track_stats {
            self.state.counters.gets.fetch_add(1, Ordering::Relaxed);
        }

        let Some((class_idx, class)) = self.state.table.route(size) else {
            if self.state.track_stats {
                self.state.counters.oversize.fetch_add(1, Ordering::Relaxed);
                self.state.counters.allocations.fetch_add(1, Ordering::Relaxed);
            }
            let mut buf = self.allocate_raw(size, size);
            self.pin(&mut buf);
            return crate::Buf::new(buf, self, u8::MAX);
        };

        // ── TLS fast path (lock-free) ──────────────────────────────
        let tls_result = TlsState::with(|tls| {
            if !tls.owns(self.state.id) {
                tls.bind(self.state.id, self.state.tls_cache_size);
            }

            if let Some(buf) = tls.caches[class_idx].pop() {
                return Some((buf, true));
            }

            tls.refill(class_idx, class, self.state.batch_size).map(|buf| (buf, false))
        });

        let ci = class_idx as u8;

        if let Some((mut buf, from_tls)) = tls_result {
            if from_tls {
                if self.state.track_stats {
                    self.state.counters.tls_hits.fetch_add(1, Ordering::Relaxed);
                }
            } else if self.state.track_stats {
                self.state.counters.shared_hits.fetch_add(1, Ordering::Relaxed);
            }
            SizeClass::resize(&mut buf, size);
            return crate::Buf::new(buf, self, ci);
        }

        // ── Cold path: fresh allocation ────────────────────────────
        if self.state.track_stats {
            self.state.counters.allocations.fetch_add(1, Ordering::Relaxed);
        }
        let mut buf = self.allocate_raw(class.class_size, size);
        self.pin(&mut buf);
        crate::Buf::new(buf, self, ci)
    }

    /// Return a buffer to the pool for reuse.
    ///
    /// `class_hint` is the class index stored in [`Buf`](crate::Buf)
    /// at allocation time (`u8::MAX` for oversize buffers that bypass pooling).
    #[inline(always)]
    pub(crate) fn dealloc(&self, mut buffer: Vec<u8>, class_hint: u8) {
        if self.state.track_stats {
            self.state.counters.puts.fetch_add(1, Ordering::Relaxed);
        }
        buffer.clear();

        if class_hint == u8::MAX {
            if self.state.track_stats {
                self.state.counters.discards.fetch_add(1, Ordering::Relaxed);
            }
            return;
        }

        let cap = buffer.capacity();

        if cap < self.state.min_buffer_size {
            if self.state.track_stats {
                self.state.counters.discards.fetch_add(1, Ordering::Relaxed);
            }
            return;
        }

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
        let overflow = TlsState::with(|tls| {
            if !tls.owns(self.state.id) {
                tls.bind(self.state.id, self.state.tls_cache_size);
            }

            let class = &self.state.table[class_idx];

            if tls.caches[class_idx].len() >= tls.limit {
                tls.spill(class_idx, class, self.state.batch_size);
            }

            if tls.caches[class_idx].len() < tls.limit {
                tls.caches[class_idx].push(buffer);
                return None;
            }

            Some(buffer)
        });

        if let Some(buf) = overflow {
            let _ = self.state.table[class_idx].push(buf);
        }
    }

    /// Warm up the pool by pre-allocating buffers for the given size class.
    ///
    /// # Example
    /// ```
    /// use zeropool::ZeroPool;
    ///
    /// let pool = ZeroPool::new().min_buffer_size(0).track_stats(true);
    /// pool.warm(16, 64 * 1024); // 16 × 64KB buffers
    /// ```
    pub fn warm(&self, count: usize, size: usize) {
        let Some((_, class)) = self.state.table.route(size) else {
            return;
        };

        for _ in 0..count {
            let mut buf = self.state.allocator.allocate(class.class_size);
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
    pub fn drain(&self) {
        self.state.table.clear_all();
    }

    /// Point-in-time snapshot of allocator statistics.
    ///
    /// # Example
    /// ```
    /// use zeropool::ZeroPool;
    ///
    /// let pool = ZeroPool::new().min_buffer_size(0).track_stats(true);
    /// let buf = pool.alloc(4096);
    /// drop(buf);
    ///
    /// let s = pool.stats();
    /// assert_eq!(s.gets, 1);
    /// assert_eq!(s.puts, 1);
    /// println!("{s}");
    /// ```
    #[inline]
    pub fn stats(&self) -> Stats {
        snapshot(&self.state.counters, self.state.table.classes())
    }

    /// Reset all performance counters to zero.
    pub fn reset_stats(&self) {
        self.state.counters.reset();
    }

    /// Allocate a raw buffer via the configured allocator and set its length.
    #[cold]
    fn allocate_raw(&self, capacity: usize, len: usize) -> Vec<u8> {
        let mut buf = self.state.allocator.allocate(capacity);
        // SAFETY: allocator guarantees capacity >= `capacity` >= `len`.
        // All u8 bit patterns are valid.
        unsafe {
            buf.set_len(len);
        }
        buf
    }

    /// Pin buffer memory to RAM if configured.
    #[inline(always)]
    fn pin(&self, buffer: &mut Vec<u8>) {
        if !self.state.pinned_memory {
            return;
        }
        if buffer.capacity() == 0 {
            return;
        }
        // SAFETY: capacity was allocated; all u8 patterns valid.
        unsafe { buffer.set_len(buffer.capacity()) };
        let _ = region::lock(buffer.as_ptr(), buffer.len());
        buffer.clear();
    }
}

impl Default for ZeroPool {
    fn default() -> Self {
        Self::new()
    }
}
