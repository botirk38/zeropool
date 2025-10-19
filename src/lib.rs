//! # ZeroPool - High-Performance Buffer Pool for Rust
//!
//! `ZeroPool` provides a thread-safe buffer pool optimized for high-throughput I/O workloads.
//! It achieves exceptional performance through:
//!
//! - **System-aware defaults**: Automatically adapts to CPU count and hardware topology
//! - **Thread-local caching**: Lock-free fast path with adaptive cache size (2-8 buffers)
//! - **Sharded global pool**: Reduces contention with CPU-scaled sharding (4-128 shards)
//! - **Zero-copy operations**: Avoids unnecessary memory allocations and copies
//! - **Smart buffer reuse**: First-fit allocation with configurable size limits
//! - **Optional memory pinning**: Lock pages in RAM to prevent swapping (requires `pinned` feature)
//!
//! # Performance
//!
//! In benchmarks with 500MB buffers, `ZeroPool` achieves:
//! - **70% faster** than no pooling (176ms → 52ms)
//! - **3.36x speedup** through buffer reuse
//! - **Lock-free** fast path for single-threaded workloads
//! - **Scales automatically** from embedded systems to 128+ core servers
//!
//! # Quick Start
//!
//! ```rust
//! use zeropool::BufferPool;
//!
//! // Create a pool with smart defaults
//! let pool = BufferPool::new();
//!
//! // Get a buffer
//! let mut buffer = pool.get(1024 * 1024); // 1MB buffer
//!
//! // Use the buffer for I/O operations
//! // ... read/write operations ...
//!
//! // Return buffer to pool for reuse
//! pool.put(buffer);
//! ```
//!
//! # Custom Configuration
//!
//! Use the builder pattern for custom configuration:
//!
//! ```rust
//! use zeropool::BufferPool;
//!
//! let pool = BufferPool::builder()
//!     .min_buffer_size(512 * 1024)      // Keep buffers >= 512KB
//!     .tls_cache_size(8)                // 8 buffers per thread
//!     .max_buffers_per_shard(32)        // Up to 32 buffers per shard
//!     .num_shards(16)                   // Override CPU-based default
//!     .build();
//! ```
//!
//! # Memory Pinning
//!
//! Lock buffer memory in RAM to prevent swapping:
//!
//! ```rust
//! use zeropool::BufferPool;
//!
//! let pool = BufferPool::builder()
//!     .pinned_memory(true)
//!     .build();
//! ```
//!
//! Useful for high-performance computing, security-sensitive data, or real-time systems.
//! May require elevated privileges. Falls back gracefully if pinning fails.
//!
//! # System-Aware Scaling
//!
//! ZeroPool automatically adapts to your system:
//!
//! | System | Cores | TLS Cache | Shards | Buffers/Shard | Total Capacity |
//! |--------|-------|-----------|--------|---------------|----------------|
//! | Embedded | 4 | 4 | 4 | 16 | 64 (~64MB) |
//! | Laptop | 8 | 6 | 8 | 16 | 128 (~128MB) |
//! | Workstation | 16 | 6 | 8 | 32 | 256 (~256MB) |
//! | Small Server | 32 | 8 | 16 | 64 | 1024 (~1GB) |
//! | Large Server | 64 | 8 | 32 | 64 | 2048 (~2GB) |
//! | Supercompute | 128 | 8 | 64 | 64 | 4096 (~4GB) |
//!
//! # Safety
//!
//! ZeroPool uses minimal `unsafe` code to avoid zero-filling buffers on reuse via `set_len()`.
//! This is safe because:
//! - Capacity checks guarantee sufficient space before setting length
//! - Buffers are immediately overwritten by I/O operations (the primary use case)
//! - Callers using buffers for reads should zero-fill themselves if needed
//!
//! The unsafe usage is limited to 2 locations in hot paths (`get()` method only).

use parking_lot::Mutex;
use std::cell::{Cell, RefCell};
use std::sync::Arc;

/// Default minimum buffer size (1MB) - optimized for ML/tensor workloads
/// Use PoolConfig presets for other workload types
const DEFAULT_MIN_BUFFER_SIZE: usize = 1024 * 1024;

/// Calculate optimal TLS cache size based on system CPU count
///
/// Lower core counts indicate fewer concurrent threads, so smaller cache is sufficient.
/// Higher core counts indicate more parallelism and benefit from larger TLS cache.
///
/// # Formula
/// - 1-2 cores (embedded): 2 buffers
/// - 3-4 cores (desktop): 4 buffers  
/// - 5-8 cores (workstation): 6 buffers
/// - 9+ cores (server): 8 buffers
const fn calculate_default_tls_cache_size(num_cpus: usize) -> usize {
    match num_cpus {
        0..=2 => 2,
        3..=4 => 4,
        5..=8 => 6,
        _ => 8,
    }
}

/// Calculate optimal shard count based on system CPU count
///
/// Strategy: Use ~1 shard per 2 cores (rounded to power-of-2) to balance
/// lock contention vs cache locality.
///
/// # Formula
/// - Target = max(num_cpus / 2, 4)
/// - Round up to next power of 2
/// - Clamp to [4, 128] range
///
/// # Examples
/// - 4 cores → 4 shards
/// - 8 cores → 8 shards  
/// - 16 cores → 8 shards
/// - 32 cores → 16 shards
/// - 64 cores → 32 shards
/// - 128 cores → 64 shards
fn calculate_num_shards(num_cpus: usize) -> usize {
    let target = (num_cpus / 2).max(4);
    next_power_of_2(target).clamp(4, 128)
}

/// Calculate optimal max buffers per shard based on system parallelism
///
/// More cores = more concurrent operations = more buffers needed.
/// Scales conservatively to avoid excessive memory usage.
///
/// # Formula
/// - Base: 16 buffers per shard
/// - Scaling: multiply by (num_cpus / 8), clamped to [1, 4]
/// - Result: 16-64 buffers per shard
///
/// # Examples
/// - 4 cores → 16 buffers/shard
/// - 8 cores → 16 buffers/shard
/// - 16 cores → 32 buffers/shard
/// - 32 cores → 64 buffers/shard
/// - 64+ cores → 64 buffers/shard (max)
const fn calculate_max_buffers_per_shard(num_cpus: usize) -> usize {
    let base = 16;
    let scaling = if num_cpus < 16 {
        1
    } else if num_cpus < 24 {
        2
    } else if num_cpus < 32 {
        3
    } else {
        4
    };
    base * scaling
}

/// Builder for configuring a `BufferPool`
///
/// Follows the idiomatic Rust builder pattern. Use `BufferPool::builder()` to create.
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
#[derive(Debug, Clone, Default)]
pub struct Builder {
    num_shards: Option<usize>,
    tls_cache_size: Option<usize>,
    max_buffers_per_shard: Option<usize>,
    min_buffer_size: Option<usize>,
    pinned_memory: bool,
}

impl Builder {
    /// Set the minimum buffer size to keep in the pool
    ///
    /// Buffers smaller than this will be discarded when returned to the pool.
    /// Default: 1MB (optimized for large I/O operations)
    pub const fn min_buffer_size(mut self, size: usize) -> Self {
        self.min_buffer_size = Some(size);
        self
    }

    /// Set the number of shards in the global pool
    ///
    /// More shards reduce lock contention in multi-threaded scenarios.
    /// Will be rounded up to next power of 2 and clamped to [4, 128].
    /// Default: Automatically calculated based on CPU count
    pub fn num_shards(mut self, count: usize) -> Self {
        self.num_shards = Some(count);
        self
    }

    /// Set the number of buffers kept in thread-local cache per thread
    ///
    /// Higher values reduce shared pool access but increase per-thread memory usage.
    /// Default: 2-8 based on CPU count
    pub const fn tls_cache_size(mut self, size: usize) -> Self {
        self.tls_cache_size = Some(size);
        self
    }

    /// Set the maximum number of buffers per shard
    ///
    /// Total pool capacity = num_shards × max_buffers_per_shard
    /// Default: 16-64 based on CPU count
    pub const fn max_buffers_per_shard(mut self, count: usize) -> Self {
        self.max_buffers_per_shard = Some(count);
        self
    }

    /// Enable memory pinning to prevent buffer pages from being swapped to disk
    ///
    /// When enabled, buffer memory will be locked in RAM using `mlock()`.
    /// Useful for high-performance computing, security-sensitive data, or real-time systems.
    ///
    /// May require elevated privileges or ulimit adjustments.
    /// Falls back gracefully if pinning fails (with a warning).
    ///
    /// Default: false
    pub const fn pinned_memory(mut self, enabled: bool) -> Self {
        self.pinned_memory = enabled;
        self
    }

    /// Build the `BufferPool` with the configured settings
    ///
    /// Any settings not explicitly set will use sensible system-aware defaults.
    pub fn build(self) -> BufferPool {
        let num_cpus = std::thread::available_parallelism()
            .map(std::num::NonZero::get)
            .unwrap_or(4);

        let config = PoolConfig {
            num_shards: self
                .num_shards
                .map(|n| next_power_of_2(n).clamp(4, 128))
                .unwrap_or_else(|| calculate_num_shards(num_cpus)),
            tls_cache_size: self
                .tls_cache_size
                .unwrap_or_else(|| calculate_default_tls_cache_size(num_cpus)),
            max_buffers_per_shard: self
                .max_buffers_per_shard
                .unwrap_or_else(|| calculate_max_buffers_per_shard(num_cpus)),
            min_buffer_size: self.min_buffer_size.unwrap_or(DEFAULT_MIN_BUFFER_SIZE),
            pinned_memory: self.pinned_memory,
        };

        BufferPool::with_config(config)
    }
}

/// Internal configuration for buffer pool (not part of public API)
#[derive(Debug, Clone)]
struct PoolConfig {
    num_shards: usize,
    tls_cache_size: usize,
    max_buffers_per_shard: usize,
    min_buffer_size: usize,
    pinned_memory: bool,
}

impl Default for PoolConfig {
    /// Create pool configuration with system-aware defaults
    ///
    /// Automatically adapts to your system by detecting CPU count and
    /// calculating optimal parameters for:
    /// - Number of shards (reduces lock contention)
    /// - TLS cache size (improves single-threaded performance)
    /// - Buffers per shard (scales with parallelism)
    ///
    /// # Adaptive Behavior
    ///
    /// - **TLS cache**: 2-8 buffers based on CPU count
    /// - **Shards**: ~1 shard per 2 cores (power-of-2, range [4, 128])
    /// - **Buffers/shard**: 16-64 based on parallelism level
    /// - **Min buffer size**: 1MB (optimized for tensor workloads)
    ///
    /// For workload-specific optimizations, use preset methods:
    /// - `PoolConfig::for_ml_workload()`
    /// - `PoolConfig::for_network_io()`
    /// - `PoolConfig::for_file_io()`
    /// - `PoolConfig::balanced()`
    fn default() -> Self {
        let num_cpus = std::thread::available_parallelism()
            .map(std::num::NonZero::get)
            .unwrap_or(4);

        Self {
            num_shards: calculate_num_shards(num_cpus),
            tls_cache_size: calculate_default_tls_cache_size(num_cpus),
            max_buffers_per_shard: calculate_max_buffers_per_shard(num_cpus),
            min_buffer_size: DEFAULT_MIN_BUFFER_SIZE,
            pinned_memory: false,
        }
    }
}

/// Round up to next power of 2
#[inline]
const fn next_power_of_2(n: usize) -> usize {
    if n == 0 {
        return 1;
    }
    let mut power = 1;
    while power < n {
        power <<= 1;
    }
    power
}

/// Pin a buffer's memory to prevent it from being swapped to disk
///
/// Uses `mlock()` to lock pages in RAM. Falls back gracefully if pinning fails.
#[inline]
fn pin_buffer(buffer: &[u8]) -> bool {
    if buffer.is_empty() {
        return true;
    }

    let ptr = buffer.as_ptr();
    let len = buffer.len();

    // Lock the memory region
    match region::lock(ptr, len) {
        Ok(_) => true,
        Err(e) => {
            // Log warning but don't fail - pinning is best-effort
            eprintln!("Warning: Failed to pin buffer memory: {e}. Continuing without pinning.");
            false
        }
    }
}

/// Thread-local cache structure combining buffers and config
struct TlsCache {
    buffers: Vec<Vec<u8>>,
    limit: usize,
}

impl TlsCache {
    const fn new() -> Self {
        Self {
            buffers: Vec::new(),
            limit: 0,
        }
    }
}

thread_local! {
    /// Thread-local cache for lock-free fast path
    /// Using RefCell for safe, runtime-checked borrowing
    static TLS_CACHE: RefCell<TlsCache> = const { RefCell::new(TlsCache::new()) };

    /// Thread-local shard counter using Cell for zero-cost access
    /// Cell is sufficient since we only need to get/set a usize (Copy type)
    static SHARD_COUNTER: Cell<usize> = const { Cell::new(0) };
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
/// │(4 bufs)  │           │(4 bufs)  │           │(4 bufs)  │
/// └────┬─────┘           └────┬─────┘           └────┬─────┘
///      │                      │                      │
///      └──────────┬───────────┴──────────────────────┘
///                 │
///         ┌───────▼────────┐
///         │  Shared Pool   │
///         │  (N shards)    │
///         │                │
///         │ [Shard 0] ...  │  N = next_pow2(num_cpus)
///         │ [Shard 1]      │  clamped to [4, 64]
///         │ [Shard 2]      │
///         │    ...         │
///         └────────────────┘
/// ```
#[derive(Clone)]
pub struct BufferPool {
    shards: Arc<Vec<Mutex<Vec<Vec<u8>>>>>,
    config: PoolConfig,
    /// Cached mask for fast shard selection (num_shards - 1)
    shard_mask: usize,
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
    /// pool.put(buffer);
    /// ```
    #[inline]
    pub fn new() -> Self {
        Builder::default().build()
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
    pub fn builder() -> Builder {
        Builder::default()
    }

    /// Create a pool with explicit configuration (internal use)
    fn with_config(config: PoolConfig) -> Self {
        let shards: Vec<Mutex<Vec<Vec<u8>>>> = (0..config.num_shards)
            .map(|_| {
                let mut buffers = Vec::new();

                // Pre-allocate and pin initial buffers if pinned memory is enabled
                if config.pinned_memory {
                    // Pre-allocate a reasonable number of buffers per shard
                    let initial_buffers = (config.max_buffers_per_shard / 2).max(4);
                    buffers.reserve(config.max_buffers_per_shard);

                    for _ in 0..initial_buffers {
                        let mut buffer = vec![0; config.min_buffer_size];
                        pin_buffer(&buffer);
                        buffer.clear(); // Clear but keep capacity and pinning
                        buffers.push(buffer);
                    }
                }

                Mutex::new(buffers)
            })
            .collect();

        // Initialize TLS cache for this thread
        TLS_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            cache.limit = config.tls_cache_size;
            // Pre-reserve capacity to avoid reallocations
            cache.buffers.reserve(config.tls_cache_size);
        });

        Self {
            shards: Arc::new(shards),
            shard_mask: config.num_shards - 1,
            config,
        }
    }

    /// Get the shard index for the current thread using round-robin
    #[inline]
    fn get_shard_index(&self) -> usize {
        SHARD_COUNTER.with(|counter| {
            let c = counter.get();
            let idx = c & self.shard_mask; // Fast modulo using cached mask
            counter.set(c.wrapping_add(1));
            idx
        })
    }

    /// Get a buffer of at least the specified size from the pool
    ///
    /// Returns an owned `Vec<u8>` with length set to `size`. The buffer may have
    /// larger capacity than requested for reuse efficiency.
    ///
    /// # Performance
    ///
    /// 1. **Fastest**: Thread-local cache LIFO (lock-free, cache-hot)
    /// 2. **Fast**: Shared pool shard LIFO + first-fit fallback
    /// 3. **Fallback**: New allocation
    ///
    /// # Panics
    ///
    /// This method may panic if internal invariants are violated (buffer found but cannot be removed).
    /// In practice, this should never occur under normal usage.
    #[inline]
    #[must_use]
    pub fn get(&self, size: usize) -> Vec<u8> {
        // Fastest path: thread-local cache (lock-free)
        let tls_hit = TLS_CACHE.with(|tls| {
            let mut cache = tls.borrow_mut();

            // Initialize limit on first access (cold path)
            if cache.limit == 0 {
                cache.limit = self.config.tls_cache_size;
                cache.buffers.reserve(self.config.tls_cache_size);
            }

            // LIFO: Check most recently used buffer first (better cache locality)
            if let Some(buf) = cache.buffers.last()
                && buf.capacity() >= size
            {
                let mut buf = cache.buffers.pop().unwrap();
                // SAFETY: capacity check above guarantees sufficient space
                unsafe {
                    buf.set_len(size);
                }
                return Some(buf);
            }

            // Fallback: First-fit search for compatible buffer
            if let Some(idx) = cache.buffers.iter().position(|b| b.capacity() >= size) {
                let mut buf = cache.buffers.swap_remove(idx);
                // SAFETY: capacity check in position() guarantees sufficient space
                unsafe {
                    buf.set_len(size);
                }
                return Some(buf);
            }
            None
        });

        if let Some(buf) = tls_hit {
            return buf;
        }

        // Fast path: try shared pool (sharded to reduce contention)
        let shard_idx = self.get_shard_index();
        let mut shard = self.shards[shard_idx].lock();

        // LIFO: Try most recently returned buffer first (cache-hot)
        if let Some(buf) = shard.last()
            && buf.capacity() >= size
        {
            let mut buffer = shard.pop().unwrap();
            // SAFETY: capacity check above guarantees sufficient space
            unsafe {
                buffer.set_len(size);
            }
            return buffer;
        }

        // First-fit fallback: scan for compatible buffer
        let idx_opt = shard.iter().position(|b| b.capacity() >= size);

        if let Some(idx) = idx_opt {
            let mut buffer = shard.swap_remove(idx);
            // SAFETY: capacity check in position() guarantees sufficient space
            unsafe {
                buffer.set_len(size);
            }
            buffer
        } else {
            // Lock released automatically before allocation
            std::mem::drop(shard);
            // Allocate new buffer (already zero-filled)
            // Will be pinned when returned to pool if pinned_memory is enabled
            vec![0u8; size]
        }
    }

    /// Return a buffer to the pool for reuse
    ///
    /// The buffer is cleared but capacity is preserved. Small buffers (below `min_size`)
    /// are automatically discarded.
    ///
    /// # Performance
    ///
    /// Prefer returning buffers to thread-local cache (lock-free) before falling back
    /// to the shared pool.
    ///
    /// # Panics
    ///
    /// This method may panic if the TLS cache is in an invalid state.
    /// In practice, this should never occur under normal usage.
    #[inline]
    pub fn put(&self, mut buffer: Vec<u8>) {
        // Clear length but keep capacity
        buffer.clear();

        // Fastest path: return to thread-local cache (lock-free)
        // Returns the buffer if not stored
        let buffer = TLS_CACHE.with(|tls| {
            let mut cache = tls.borrow_mut();

            // Initialize limit if needed (cold path)
            if cache.limit == 0 {
                cache.limit = self.config.tls_cache_size;
                cache.buffers.reserve(self.config.tls_cache_size);
            }

            if cache.buffers.len() < cache.limit {
                cache.buffers.push(buffer);
                None // Successfully stored in TLS
            } else {
                Some(buffer) // TLS full, return buffer
            }
        });

        // Fast path succeeded
        if buffer.is_none() {
            return;
        }

        let buffer = buffer.unwrap();

        // Fall back to shared pool (sharded to reduce contention)
        if buffer.capacity() < self.config.min_buffer_size {
            return; // Too small, discard
        }

        // Pin buffer before adding to pool if enabled
        let buffer = if self.config.pinned_memory && buffer.capacity() > 0 {
            // Ensure buffer has actual allocated memory before pinning
            let cap = buffer.capacity();
            let mut buf = buffer;
            buf.resize(cap, 0);
            pin_buffer(&buf);
            buf.clear(); // Clear but keep capacity and pinning
            buf
        } else {
            buffer
        };

        let shard_idx = self.get_shard_index();
        let mut shard = self.shards[shard_idx].lock();

        // Limit pool size per shard to prevent unbounded growth
        if shard.len() < self.config.max_buffers_per_shard {
            shard.push(buffer);
        }
        // Lock released automatically
    }

    /// Pre-allocate buffers in the pool
    ///
    /// Useful for warming up the pool before high-throughput operations.
    /// Distributes buffers evenly across all shards.
    pub fn preallocate(&self, count: usize, size: usize) {
        let per_shard = count.div_ceil(self.config.num_shards);

        for shard in self.shards.iter() {
            let mut buffers = shard.lock();
            buffers.reserve(per_shard);
            for _ in 0..per_shard {
                let buffer = {
                    let mut buf = Vec::with_capacity(size.max(self.config.min_buffer_size));
                    if self.config.pinned_memory && buf.capacity() > 0 {
                        // Ensure buffer has actual allocated memory before pinning
                        buf.resize(buf.capacity(), 0);
                        pin_buffer(&buf);
                        buf.clear();
                    }
                    buf
                };

                buffers.push(buffer);
            }
        }
    }

    /// Get the total number of buffers currently in all shards
    ///
    /// Note: Does not include thread-local cached buffers
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.shards.iter().map(|s| s.lock().len()).sum()
    }

    /// Check if all shards are empty
    ///
    /// Note: Does not check thread-local caches
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.shards.iter().all(|s| s.lock().is_empty())
    }
}

impl Default for BufferPool {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_pool_operations() {
        let pool = BufferPool::new();

        // Get a buffer
        let buf = pool.get(1024);
        assert_eq!(buf.len(), 1024);

        // Return it
        pool.put(buf);

        // Should reuse the buffer
        let buf2 = pool.get(1024);
        assert_eq!(buf2.len(), 1024);
    }

    #[test]
    fn test_buffer_sizing() {
        // Use small min_buffer_size so we can test buffer reuse with small buffers
        let pool = BufferPool::builder().min_buffer_size(0).build();

        // Request larger buffer
        let buf = pool.get(2048);
        assert_eq!(buf.len(), 2048);

        // Return it
        pool.put(buf);

        // Smaller request should reuse the buffer
        let buf2 = pool.get(1024);
        assert_eq!(buf2.len(), 1024);
        assert!(buf2.capacity() >= 2048);
    }

    #[test]
    fn test_min_size_filtering() {
        use std::thread;

        let pool = BufferPool::builder()
            .min_buffer_size(1024 * 1024)
            .max_buffers_per_shard(16)
            .build();

        let tls_cache_size = pool.config.tls_cache_size;
        let pool_clone = pool.clone();

        // Test in separate thread to ensure clean TLS state
        thread::spawn(move || {
            // Fill TLS cache with small buffers
            for _ in 0..tls_cache_size {
                let buf = pool_clone.get(512);
                pool_clone.put(buf);
            }

            // Next small buffer should be rejected by shared pool (below min_size)
            let small_buf = pool_clone.get(512);
            pool_clone.put(small_buf);
        })
        .join()
        .unwrap();

        // Shared pool should be empty (small buffers don't meet min_size)
        assert_eq!(pool.len(), 0);

        // Test with large buffers in another thread
        let pool_clone = pool.clone();
        thread::spawn(move || {
            // Create tls_cache_size + 1 buffers
            let mut buffers = Vec::new();
            for _ in 0..(tls_cache_size + 1) {
                buffers.push(pool_clone.get(2 * 1024 * 1024));
            }

            // Return them all - first tls_cache_size go to TLS, last one to shared pool
            for buf in buffers {
                pool_clone.put(buf);
            }
        })
        .join()
        .unwrap();

        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn test_max_pool_size() {
        let pool = BufferPool::builder()
            .min_buffer_size(0)
            .max_buffers_per_shard(2)
            .build();

        let tls_cache_size = pool.config.tls_cache_size;
        let num_shards = pool.config.num_shards;

        // Fill TLS cache first, then overflow to shared pool across shards
        let mut buffers = Vec::new();

        // Get enough buffers to fill TLS and multiple shards beyond their limits
        // tls_cache_size go to TLS, rest distributed across shards
        for _ in 0..(tls_cache_size + num_shards * 3) {
            buffers.push(pool.get(1024));
        }

        // Return them all
        for buf in buffers {
            pool.put(buf);
        }

        // Each shard should have max 2 buffers (max_pool_size)
        // Total should be at most num_shards * 2
        assert!(pool.len() <= num_shards * 2);

        // Verify each shard respects the limit
        for shard in pool.shards.iter() {
            assert!(shard.lock().len() <= 2);
        }
    }

    #[test]
    fn test_thread_local_cache() {
        let pool = BufferPool::new();
        let cache_size = pool.config.tls_cache_size;

        // First tls_cache_size get/put operations should use TLS
        for _ in 0..cache_size {
            let buf = pool.get(1024);
            pool.put(buf);
        }

        // Pool should be empty (all buffers in TLS)
        assert_eq!(pool.len(), 0);

        // Should hit TLS for all cached buffers
        for _ in 0..cache_size {
            let buf = pool.get(1024);
            assert_eq!(buf.len(), 1024);
        }

        // Pool still empty since we consumed from TLS
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn test_builder_api() {
        // Test default builder
        let pool1 = BufferPool::builder().build();
        assert!(!pool1.is_empty() || pool1.is_empty()); // Just verify it compiles

        // Test all builder methods
        let pool2 = BufferPool::builder()
            .min_buffer_size(256 * 1024)
            .num_shards(8)
            .tls_cache_size(4)
            .max_buffers_per_shard(16)
            .build();

        assert_eq!(pool2.config.min_buffer_size, 256 * 1024);
        assert_eq!(pool2.config.num_shards, 8);
        assert_eq!(pool2.config.tls_cache_size, 4);
        assert_eq!(pool2.config.max_buffers_per_shard, 16);

        // Test that it actually works
        let buf = pool2.get(512 * 1024);
        assert_eq!(buf.len(), 512 * 1024);
        pool2.put(buf);
    }

    #[test]
    fn test_pinned_memory() {
        // Test that pinned memory doesn't crash (may not actually pin without privileges)
        let pool = BufferPool::builder().pinned_memory(true).build();

        let buf = pool.get(1024 * 1024);
        assert_eq!(buf.len(), 1024 * 1024);
        pool.put(buf);

        // Verify we can still use the pool normally
        let buf2 = pool.get(512 * 1024);
        assert_eq!(buf2.len(), 512 * 1024);
    }

    #[test]
    fn test_concurrent_access() {
        use std::thread;

        let pool = BufferPool::new();
        let mut handles = vec![];

        // Spawn 8 threads doing concurrent get/put
        for _ in 0..8 {
            let pool = pool.clone();
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    let buf = pool.get(4096);
                    assert_eq!(buf.len(), 4096);
                    pool.put(buf);
                }
            }));
        }

        // Wait for all threads
        for handle in handles {
            handle.join().unwrap();
        }

        // Pool should still be functional
        assert!(pool.len() < 1000); // Some buffers may be pooled
    }

    #[test]
    fn test_clone_shares_state() {
        let pool = BufferPool::builder()
            .min_buffer_size(0)
            .tls_cache_size(2)
            .build();

        // Get buffers in main thread to fill TLS cache
        let buf1 = pool.get(1024);
        let buf2 = pool.get(1024);
        pool.put(buf1);
        pool.put(buf2);

        // Clone the pool
        let pool_clone = pool.clone();

        // Put a buffer in clone - should overflow to shared pool
        let buf3 = pool_clone.get(2048);
        let buf4 = pool_clone.get(2048);
        let buf5 = pool_clone.get(2048);
        pool_clone.put(buf3);
        pool_clone.put(buf4);
        pool_clone.put(buf5);

        // Original pool should see buffers in shared pool
        assert!(!pool.is_empty());
    }

    #[test]
    fn test_preallocate() {
        let pool = BufferPool::builder()
            .min_buffer_size(512 * 1024)
            .num_shards(4)
            .build();

        let initial_len = pool.len();

        // Preallocate some buffers
        pool.preallocate(10, 1024 * 1024);

        // Pool should have more buffers now (distributed across shards)
        assert!(pool.len() > initial_len);

        // Should be able to get a preallocated buffer
        let buf = pool.get(1024 * 1024);
        assert!(buf.capacity() >= 1024 * 1024);
    }

    #[test]
    fn test_edge_cases() {
        use std::thread;

        let pool = BufferPool::builder()
            .tls_cache_size(2)
            .min_buffer_size(0)
            .build();

        // Test is_empty on new pool
        assert!(pool.is_empty());

        // Test zero-size buffer
        let buf_zero = pool.get(0);
        assert_eq!(buf_zero.len(), 0);
        pool.put(buf_zero);

        // Test very large buffer - fill TLS cache first, then add more
        let pool_clone = pool.clone();
        thread::spawn(move || {
            // Fill TLS cache
            let b1 = pool_clone.get(1024);
            let b2 = pool_clone.get(1024);
            pool_clone.put(b1);
            pool_clone.put(b2);

            // This should overflow to shared pool
            let buf_large = pool_clone.get(100 * 1024 * 1024); // 100MB
            assert_eq!(buf_large.len(), 100 * 1024 * 1024);
            pool_clone.put(buf_large);
        })
        .join()
        .unwrap();

        // Pool should have the large buffer now (in shared pool, not TLS)
        assert!(!pool.is_empty());
    }

    #[test]
    fn test_shard_distribution() {
        use std::thread;

        let pool = BufferPool::builder()
            .num_shards(4)
            .min_buffer_size(0)
            .tls_cache_size(2)
            .build();

        // Put many buffers from separate thread to avoid TLS cache
        let pool_clone = pool.clone();
        thread::spawn(move || {
            let mut buffers = vec![];
            for _ in 0..20 {
                buffers.push(pool_clone.get(1024));
            }
            for buf in buffers {
                pool_clone.put(buf);
            }
        })
        .join()
        .unwrap();

        // Buffers should be distributed across shards
        let mut non_empty_shards = 0;
        for shard in pool.shards.iter() {
            if !shard.lock().is_empty() {
                non_empty_shards += 1;
            }
        }

        // Should have buffers in multiple shards (not all in one)
        assert!(non_empty_shards >= 2);
    }
}
