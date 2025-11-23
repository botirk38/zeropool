/// Buffer eviction policy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EvictionPolicy {
    /// Simple LIFO (current behavior, lowest overhead)
    Lifo,
    /// CLOCK-Pro with access counter (better cache locality)
    #[default]
    ClockPro,
}

use std::thread;

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
    const BASE: usize = 16;
    let scaling = match num_cpus {
        0..16 => 1,
        16..24 => 2,
        24..32 => 3,
        _ => 4,
    };
    BASE * scaling
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
    pinned_memory: Option<bool>,
    eviction_policy: Option<EvictionPolicy>,
}

impl Builder {
    /// Set the minimum buffer size to keep in the pool
    ///
    /// Buffers smaller than this will be discarded when returned to the pool.
    /// Default: 1MB (optimized for large I/O operations)
    pub fn min_buffer_size(mut self, size: usize) -> Self {
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
    pub fn tls_cache_size(mut self, size: usize) -> Self {
        self.tls_cache_size = Some(size);
        self
    }

    /// Set the maximum number of buffers per shard
    ///
    /// Total pool capacity = num_shards × max_buffers_per_shard
    /// Default: 16-64 based on CPU count
    pub fn max_buffers_per_shard(mut self, count: usize) -> Self {
        self.max_buffers_per_shard = Some(count);
        self
    }

    /// Enable pinned memory (mlock) for pooled buffers
    ///
    /// When enabled, buffers are locked in RAM and won't be swapped to disk.
    /// This is useful for security-sensitive or latency-critical applications.
    ///
    /// Requires sufficient permissions (CAP_IPC_LOCK on Linux or similar).
    /// Falls back gracefully if pinning fails.
    /// Default: false
    pub fn pinned_memory(mut self, enabled: bool) -> Self {
        self.pinned_memory = Some(enabled);
        self
    }

    /// Set buffer eviction policy
    ///
    /// # Examples
    /// ```
    /// use zeropool::{BufferPool, EvictionPolicy};
    ///
    /// let pool = BufferPool::builder()
    ///     .eviction_policy(EvictionPolicy::Lifo)  // Use simple LIFO
    ///     .build();
    /// ```
    pub fn eviction_policy(mut self, policy: EvictionPolicy) -> Self {
        self.eviction_policy = Some(policy);
        self
    }

    /// Build the `BufferPool` with the configured settings
    ///
    /// Any settings not explicitly set will use sensible system-aware defaults.
    ///
    /// # Panics
    ///
    /// Panics if `tls_cache_size` is set to 0 or `max_buffers_per_shard` is set to 0.
    pub fn build(self) -> crate::BufferPool {
        let num_cpus = thread::available_parallelism().map(std::num::NonZero::get).unwrap_or(4);

        let tls_cache_size = self
            .tls_cache_size
            .unwrap_or_else(|| calculate_default_tls_cache_size(num_cpus));

        let max_buffers_per_shard = self
            .max_buffers_per_shard
            .unwrap_or_else(|| calculate_max_buffers_per_shard(num_cpus));

        // Validate configuration
        assert!(tls_cache_size > 0, "tls_cache_size must be greater than 0");
        assert!(max_buffers_per_shard > 0, "max_buffers_per_shard must be greater than 0");

        let config = PoolConfig {
            num_shards: self.num_shards.map_or_else(
                || calculate_num_shards(num_cpus),
                |n| {
                    let normalized = next_power_of_2(n).clamp(4, 128);
                    // Verify it's a power of 2 after clamping
                    debug_assert!(normalized.is_power_of_two(), "num_shards must be power of 2");
                    normalized
                },
            ),
            tls_cache_size,
            max_buffers_per_shard,
            min_buffer_size: self.min_buffer_size.unwrap_or(DEFAULT_MIN_BUFFER_SIZE),
            pinned_memory: self.pinned_memory.unwrap_or(false),
            eviction_policy: self.eviction_policy.unwrap_or_default(),
        };

        crate::BufferPool::with_config(config)
    }
}

/// Internal configuration for buffer pool (not part of public API)
#[derive(Debug, Clone)]
pub(crate) struct PoolConfig {
    pub num_shards: usize,
    pub tls_cache_size: usize,
    pub max_buffers_per_shard: usize,
    pub min_buffer_size: usize,
    pub pinned_memory: bool,
    pub eviction_policy: EvictionPolicy,
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
        let num_cpus = thread::available_parallelism().map(std::num::NonZero::get).unwrap_or(4);

        Self {
            num_shards: calculate_num_shards(num_cpus),
            tls_cache_size: calculate_default_tls_cache_size(num_cpus),
            max_buffers_per_shard: calculate_max_buffers_per_shard(num_cpus),
            min_buffer_size: DEFAULT_MIN_BUFFER_SIZE,
            pinned_memory: false,
            eviction_policy: EvictionPolicy::default(),
        }
    }
}
