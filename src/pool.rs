use parking_lot::Mutex;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use crate::EvictionPolicy;
use crate::config::PoolConfig;
use crate::tls::{SHARD_AFFINITY, TLS_CACHE, TLS_LIMIT};
use crate::utils::pin_buffer;

/// Metadata for buffer eviction policy
///
/// Uses a saturating access counter to track buffer "hotness"
/// without the overhead of timestamps or LRU lists.
#[derive(Debug)]
pub(crate) struct BufferEntry {
    pub(crate) buffer: Vec<u8>,
    /// Saturating counter (0-255) tracking recent accesses
    /// Higher values indicate "hotter" buffers that should be retained
    pub(crate) access_count: u8,
}

impl BufferEntry {
    #[inline]
    fn new(buffer: Vec<u8>) -> Self {
        Self {
            buffer,
            access_count: 1, // Start with 1 to indicate recent allocation
        }
    }

    /// Mark buffer as accessed (increment counter, saturating at 255)
    #[inline]
    fn mark_accessed(&mut self) {
        self.access_count = self.access_count.saturating_add(1);
    }

    /// Decay access count (used during eviction)
    #[inline]
    fn decay(&mut self) {
        self.access_count = self.access_count.saturating_sub(1);
    }

    #[inline]
    fn capacity(&self) -> usize {
        self.buffer.capacity()
    }
}

/// A single shard containing buffers and a local counter
///
/// Each shard has its own atomic counter to reduce contention
/// compared to a single shared counter across all shards.
#[derive(Debug)]
pub(crate) struct Shard {
    pub(crate) buffers: Mutex<Vec<BufferEntry>>,
    pub(crate) count: AtomicUsize,
}

impl Shard {
    fn new() -> Self {
        Self {
            buffers: Mutex::new(Vec::new()),
            count: AtomicUsize::new(0),
        }
    }
}

/// A high-performance, thread-safe buffer pool optimized for I/O workloads
///
/// # Performance Characteristics
///
/// - **O(1)** buffer allocation from pool (LIFO + first-fit fallback)
/// - **Lock-free** for single-threaded access via thread-local storage
/// - **Sharded** global pool to reduce multi-threaded contention
/// - **System-aware** defaults based on CPU count
/// - **Zero-copy** buffer reuse (no unnecessary allocations)
///
/// # Thread Safety
///
/// `BufferPool` is `Clone` and can be shared across threads. Each thread maintains
/// its own thread-local cache for maximum performance, with a sharded global pool
/// as fallback to minimize lock contention.
///
/// # Architecture
///
/// ```text
/// Thread 1                Thread 2                Thread N
/// ┌──────────┐           ┌──────────┐           ┌──────────┐
/// │TLS Cache │           │TLS Cache │           │TLS Cache │
/// │(2-8 bufs)│           │TLS Cache │           │TLS Cache │
/// └────┬─────┘           └────┬─────┘           └────┬─────┘
///      │(affinity)            │(affinity)            │(affinity)
///      │                      │                      │
///      └──────────┬───────────┴──────────────────────┘
///                 │
///         ┌───────▼────────┐
///         │  Shared Pool   │
///         │  (N shards)    │  Thread-local affinity:
///         │                │  Each thread uses same
///         │ [Shard 0] ...  │  shard for cache locality
///         │ [Shard 1]      │
///         │ [Shard 2]      │  N = next_pow2(num_cpus/2)
///         │    ...         │  clamped to [4, 128]
///         └────────────────┘
/// ```
#[derive(Clone, Debug)]
pub struct BufferPool {
    pub(crate) shards: Arc<Vec<Shard>>, // Changed to use Shard struct
    pub(crate) config: PoolConfig,
    /// Cached mask for fast shard selection (num_shards - 1)
    shard_mask: usize,
    // Removed: total_buffers - replaced with per-shard counters
}

impl BufferPool {
    /// Create a new buffer pool with system-aware defaults
    ///
    /// The pool automatically configures itself based on CPU count:
    /// - Number of shards (4-128, reduces lock contention)
    /// - Thread-local cache size (2-8 buffers per thread)
    /// - Max buffers per shard (16-64)
    /// - 1MB minimum buffer size
    ///
    /// # Example
    /// ```
    /// use zeropool::BufferPool;
    ///
    /// let pool = BufferPool::new();
    /// let buffer = pool.get(1024 * 1024);
    /// // ... use buffer ...
    /// // Buffer automatically returned when it goes out of scope
    /// ```
    #[inline]
    pub fn new() -> Self {
        crate::Builder::default().build()
    }

    /// Create a builder for custom configuration
    ///
    /// Use this when you need to customize the pool behavior.
    ///
    /// # Example
    /// ```
    /// use zeropool::BufferPool;
    ///
    /// let pool = BufferPool::builder()
    ///     .min_buffer_size(512 * 1024)  // 512KB minimum
    ///     .num_shards(16)
    ///     .build();
    /// ```
    #[inline]
    pub fn builder() -> crate::Builder {
        crate::Builder::default()
    }

    /// Create a pool with explicit configuration (internal use)
    pub(crate) fn with_config(config: PoolConfig) -> Self {
        let shards: Vec<Shard> = (0..config.num_shards).map(|_| Shard::new()).collect();

        // Initialize TLS limit for this thread
        TLS_LIMIT.with(|limit| {
            limit.set(config.tls_cache_size);
        });

        Self {
            shards: Arc::new(shards),
            shard_mask: config.num_shards - 1,
            config,
        }
    }

    /// Get the shard index for the current thread using thread-local affinity
    ///
    /// Each thread gets a consistent shard assignment based on its thread ID hash.
    /// This improves cache locality by having each thread reuse the same shard's
    /// cached data structures.
    #[inline(always)]
    fn get_shard_index(&self) -> usize {
        SHARD_AFFINITY.with(|affinity| {
            if let Some(idx) = affinity.get() {
                // Fast path: affinity already computed
                // Validate that cached affinity is valid for this pool's shard count
                if idx < self.shards.len() {
                    return idx;
                }
                // Cached affinity is stale (from a pool with more shards), recompute
            }
            // Cold path: compute affinity once per thread
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            thread::current().id().hash(&mut hasher);
            let idx = (hasher.finish() as usize) & self.shard_mask;
            affinity.set(Some(idx));
            idx
        })
    }

    /// Get a buffer of at least the specified size from the pool
    ///
    /// Returns a `PooledBuffer` that automatically returns to the pool when dropped.
    /// The buffer may have larger capacity than requested for reuse efficiency.
    ///
    /// # Performance
    ///
    /// 1. **Fastest**: Thread-local cache LIFO (lock-free, cache-hot)
    /// 2. **Fast**: Shared pool shard LIFO + first-fit fallback
    /// 3. **Fallback**: New allocation
    ///
    /// # Examples
    ///
    /// ```
    /// use zeropool::BufferPool;
    ///
    /// let pool = BufferPool::new();
    /// {
    ///     let mut buffer = pool.get(1024);
    ///     buffer[0] = 42;
    ///     // Buffer automatically returned here
    /// }
    /// ```
    ///
    /// # Panics
    ///
    /// This method may panic if internal invariants are violated (buffer found but cannot be removed).
    /// In practice, this should never occur under normal usage.
    #[inline]
    #[must_use]
    pub fn get(&self, size: usize) -> crate::buffer::PooledBuffer {
        // Fastest path: thread-local cache (lock-free)
        let tls_hit = TLS_CACHE.with(|tls| {
            let mut cache = tls.borrow_mut();

            // LIFO: Check most recently used buffer first (better cache locality)
            if let Some(entry) = cache.buffers.last()
                && entry.capacity() >= size
            {
                let mut entry = cache.buffers.pop().unwrap();
                entry.mark_accessed(); // NEW: Track access
                let mut buf = entry.buffer;
                // Resize buffer to requested size, zeroing memory in the process
                buf.resize(size, 0);
                return Some(buf);
            }

            // Fallback: First-fit search for compatible buffer
            if let Some(idx) = cache.buffers.iter().position(|e| e.capacity() >= size) {
                let mut entry = cache.buffers.swap_remove(idx);
                entry.mark_accessed(); // NEW: Track access
                let mut buf = entry.buffer;
                // Resize buffer to requested size, zeroing memory in the process
                buf.resize(size, 0);
                return Some(buf);
            }
            None
        });

        if let Some(buf) = tls_hit {
            return crate::buffer::PooledBuffer::new(buf, self.clone());
        }

        // Fast path: try shared pool (sharded to reduce contention)
        let shard_idx = self.get_shard_index();
        // SAFETY: get_shard_index() uses bitmask (shard_mask = num_shards - 1) where
        // num_shards is power of 2, guaranteeing shard_idx < shards.len()
        assert!(shard_idx < self.shards.len(), "Shard index out of bounds");
        let shard = &self.shards[shard_idx];
        let mut buffers = shard.buffers.lock();

        // LIFO: Try most recently returned buffer first (cache-hot)
        if let Some(entry) = buffers.last()
            && entry.capacity() >= size
        {
            let mut entry = buffers.pop().unwrap();
            shard.count.fetch_sub(1, Ordering::Relaxed);
            entry.mark_accessed(); // NEW: Track access
            let mut buffer = entry.buffer;
            // Resize buffer to requested size, zeroing memory in the process
            buffer.resize(size, 0);
            return crate::buffer::PooledBuffer::new(buffer, self.clone());
        }

        // First-fit fallback: scan for compatible buffer
        let vec = if let Some(idx) = buffers.iter().position(|e| e.capacity() >= size) {
            let mut entry = buffers.swap_remove(idx);
            shard.count.fetch_sub(1, Ordering::Relaxed);
            entry.mark_accessed(); // NEW: Track access
            let mut buffer = entry.buffer;
            // Resize buffer to requested size, zeroing memory in the process
            buffer.resize(size, 0);
            buffer
        } else {
            drop(buffers);
            vec![0u8; size]
        };

        crate::buffer::PooledBuffer::new(vec, self.clone())
    }

    /// Return a buffer to the pool for reuse (internal use only)
    ///
    /// This method is called automatically by `PooledBuffer::drop()`.
    /// Users should not call this directly.
    ///
    /// The buffer is zeroed and cleared to prevent information leakage.
    /// Capacity is preserved. Small buffers (below `min_size`)
    /// are automatically discarded.
    #[inline]
    pub(crate) fn put(&self, mut buffer: Vec<u8>) {
        // Zero the buffer memory to prevent information leakage
        buffer.fill(0);
        // Clear length but keep capacity
        buffer.clear();

        // Initialize TLS limit on first use (cold path, happens once per thread)
        let limit = TLS_LIMIT.with(|limit| {
            let current = limit.get();
            if current == 0 {
                limit.set(self.config.tls_cache_size);
                self.config.tls_cache_size
            } else {
                current
            }
        });

        // Fastest path: return to thread-local cache (lock-free)
        // Returns the buffer if not stored
        let buffer = TLS_CACHE.with(|tls| {
            let mut cache = tls.borrow_mut();

            if cache.buffers.len() < limit {
                cache.buffers.push(BufferEntry::new(buffer)); // Wrap in entry
                None // Successfully stored in TLS
            } else {
                Some(buffer) // TLS full, return buffer
            }
        });

        // Fast path succeeded
        let Some(buffer) = buffer else {
            return;
        };

        // Fall back to shared pool (sharded to reduce contention)
        if buffer.capacity() < self.config.min_buffer_size {
            return; // Too small, discard
        }

        // Pin buffer memory if enabled
        if self.config.pinned_memory {
            pin_buffer(&buffer);
        }

        let shard_idx = self.get_shard_index();
        // SAFETY: shard_idx guaranteed in bounds by bitmask operation in get_shard_index()
        assert!(shard_idx < self.shards.len(), "Shard index out of bounds in put");
        let shard = &self.shards[shard_idx];
        let mut buffers = shard.buffers.lock();

        // Limit pool size per shard to prevent unbounded growth
        if buffers.len() < self.config.max_buffers_per_shard {
            buffers.push(BufferEntry::new(buffer)); // Wrap in entry
            shard.count.fetch_add(1, Ordering::Relaxed);
        } else {
            // NEW: Eviction when shard full
            if self.config.eviction_policy == EvictionPolicy::ClockPro {
                Self::evict_and_insert(&mut buffers, buffer);
            }
            // Otherwise drop buffer (current behavior)
        }
        // Lock released automatically
    }

    /// Evict cold buffer and insert new one using CLOCK-Pro algorithm
    fn evict_and_insert(shard: &mut [BufferEntry], new_buffer: Vec<u8>) {
        // Safety check: shard must not be empty
        if shard.is_empty() {
            return;
        }

        // Find buffer with lowest access count
        let mut min_idx = 0;
        let mut min_count = u8::MAX;

        for (idx, entry) in shard.iter_mut().enumerate() {
            entry.decay(); // Decay all counters
            if entry.access_count < min_count {
                min_count = entry.access_count;
                min_idx = idx;
            }
        }

        // Replace coldest buffer with new one (always evict something)
        shard[min_idx] = BufferEntry::new(new_buffer);
        // Note: We don't increment total_buffers since we're replacing, not adding
    }

    /// Pre-allocate buffers in the pool
    ///
    /// Useful for warming up the pool before high-throughput operations.
    /// Distributes buffers evenly across all shards.
    /// Optimized to allocate all buffers first, then distribute with minimal locking.
    pub fn preallocate(&self, count: usize, size: usize) {
        let per_shard = count.div_ceil(self.config.num_shards);
        let total_to_allocate = per_shard * self.config.num_shards;

        // Allocate all buffers at once
        let mut all_buffers: Vec<BufferEntry> = Vec::with_capacity(total_to_allocate);
        for _ in 0..total_to_allocate {
            let mut buf = Vec::with_capacity(size.max(self.config.min_buffer_size));

            // Pin buffer memory if enabled
            if self.config.pinned_memory {
                // Need to set length for pinning, using safe resize
                buf.resize(buf.capacity(), 0);
                pin_buffer(&buf);
                buf.clear();
            }

            all_buffers.push(BufferEntry::new(buf)); // Wrap in entry
        }

        // Distribute buffers across shards with single lock per shard
        // Process shards in reverse order to avoid index shifting during drain
        for shard_idx in (0..self.config.num_shards).rev() {
            // SAFETY: shard_idx < num_shards by loop bounds
            assert!(shard_idx < self.shards.len(), "Shard index out of bounds in preallocate");
            let shard = &self.shards[shard_idx];
            let mut buffers = shard.buffers.lock();
            buffers.reserve(per_shard);

            let start = shard_idx * per_shard;
            let end = start + per_shard;

            // Move buffers directly without cloning
            for entry in all_buffers.drain(start..end) {
                buffers.push(entry);
            }

            shard.count.fetch_add(per_shard, Ordering::Relaxed);
        }
    }

    /// Get the total number of buffers currently in all shards
    ///
    /// Note: Does not include thread-local cached buffers.
    /// This is now O(N) where N = num_shards, using per-shard counters.
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.shards.iter().map(|s| s.count.load(Ordering::Relaxed)).sum()
    }

    /// Check if all shards are empty
    ///
    /// Note: Does not check thread-local caches.
    /// This is now O(N) where N = num_shards, using per-shard counters.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.shards.iter().all(|s| s.count.load(Ordering::Relaxed) == 0)
    }

    /// Clears all buffers from the shared pool
    ///
    /// Removes all buffers from all shards, releasing memory back to the OS.
    /// Thread-local caches are NOT cleared (they're thread-private).
    ///
    /// # Examples
    /// ```
    /// use zeropool::BufferPool;
    ///
    /// let pool = BufferPool::builder().min_buffer_size(0).build();
    ///
    /// // Get and return some buffers to the pool
    /// {
    ///     let _buf1 = pool.get(1024);
    ///     let _buf2 = pool.get(2048);
    ///     // Buffers automatically returned when dropped
    /// }
    ///
    /// // Clear all buffers from the shared pool
    /// pool.clear();
    ///
    /// // Pool is now empty (thread-local caches are not affected)
    /// ```
    ///
    /// # Thread Safety
    /// This method is safe to call concurrently. Each shard is locked
    /// individually, so other threads can continue using the pool.
    ///
    /// # Performance
    /// O(num_shards) with brief lock contention per shard
    pub fn clear(&self) {
        for shard in self.shards.iter() {
            let mut buffers = shard.buffers.lock();
            let count = buffers.len();
            buffers.clear();
            shard.count.fetch_sub(count, Ordering::Relaxed);
        }
    }

    /// Shrinks the capacity of all shards to fit their current contents
    ///
    /// This can reduce memory usage if the pool previously held many buffers
    /// but now holds few. Similar to `Vec::shrink_to_fit()`.
    ///
    /// # Performance
    /// O(num_shards) with brief lock contention per shard
    pub fn shrink_to_fit(&self) {
        for shard in self.shards.iter() {
            shard.buffers.lock().shrink_to_fit();
        }
    }
}

impl Default for BufferPool {
    fn default() -> Self {
        Self::new()
    }
}
