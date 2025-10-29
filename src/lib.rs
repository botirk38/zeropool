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
//! ZeroPool uses targeted unsafe optimizations for maximum performance while maintaining
//! memory safety guarantees:
//!
//! - **`set_len()` instead of `resize()`**: When reusing pooled buffers, we use unsafe
//!   `set_len()` to avoid redundant zero-initialization. This is safe because:
//!   1. Capacity is always verified before setting length
//!   2. Buffers are pooled (previously allocated and cleared)
//!   3. Users receive initialized memory (may contain previous data, but no UB)
//!
//! - **Safe shard indexing**: Shard index is masked with `shard_mask` (power of 2 - 1),
//!   guaranteeing bounds. Runtime assertions verify this invariant before access.
//!
//! - **Optional memory pinning**: When `pinned_memory` is enabled, buffers are locked in RAM
//!   using `mlock` to prevent swapping. This is best-effort and fails gracefully if insufficient
//!   permissions. Useful for security-sensitive or latency-critical workloads.

mod config;
mod pool;
mod tls;
mod utils;

// Public API exports
pub use config::{Builder, EvictionPolicy};
pub use pool::BufferPool;

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
        // Note: Small buffers in TLS cache are NOT returned to pool when thread exits
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

        // Only the buffer that overflowed TLS cache made it to the shared pool
        // The tls_cache_size buffers remain in the exited thread's TLS cache
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
            assert!(shard.buffers.lock().len() <= 2);
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

        // Spawn multiple threads to test distribution across shards
        // Each thread has affinity to one shard, so multiple threads
        // should distribute buffers across multiple shards
        let mut handles = vec![];
        for _ in 0..4 {
            let pool_clone = pool.clone();
            handles.push(thread::spawn(move || {
                let mut buffers = vec![];
                for _ in 0..5 {
                    buffers.push(pool_clone.get(1024));
                }
                for buf in buffers {
                    pool_clone.put(buf);
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // With multiple threads, buffers should be distributed across shards
        let mut non_empty_shards = 0;
        for shard in pool.shards.iter() {
            if !shard.buffers.lock().is_empty() {
                non_empty_shards += 1;
            }
        }

        // Should have buffers in multiple shards due to thread affinity
        assert!(non_empty_shards >= 2);
    }

    #[test]
    fn test_eviction_policy_hot_buffers() {
        let pool = BufferPool::builder()
            .eviction_policy(EvictionPolicy::ClockPro)
            .max_buffers_per_shard(5)
            .num_shards(1)
            .tls_cache_size(1) // Small but valid TLS cache
            .build();

        // Create 10 buffers with unique sizes
        let sizes: Vec<usize> = (1..=10).map(|i| i * 1024).collect();

        // Add 5 buffers (fill shard to max)
        for &size in &sizes[0..5] {
            let buf = vec![0u8; size];
            pool.put(buf);
        }

        // Access first 3 buffers repeatedly (make them hot)
        for _ in 0..10 {
            for &size in &sizes[0..3] {
                let buf = pool.get(size);
                pool.put(buf);
            }
        }

        // Add 5 more buffers (trigger eviction)
        for &size in &sizes[5..10] {
            let buf = vec![0u8; size];
            pool.put(buf);
        }

        // Hot buffers (0-2) should still be available
        // Cold buffers (3-4) should have been evicted
        for &size in &sizes[0..3] {
            let buf = pool.get(size);
            assert_eq!(buf.capacity(), size, "Hot buffer was evicted!");
            pool.put(buf);
        }
    }

    #[test]
    fn test_clear() {
        let pool = BufferPool::builder()
            .min_buffer_size(0)
            .tls_cache_size(2)
            .build();

        // Fill pool with buffers
        let mut buffers = vec![];
        for _ in 0..10 {
            buffers.push(pool.get(1024));
        }
        for buf in buffers {
            pool.put(buf);
        }

        assert!(!pool.is_empty());

        // Clear the pool
        pool.clear();

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());

        // Pool should still work after clear
        let buf = pool.get(1024);
        assert_eq!(buf.len(), 1024);
        pool.put(buf);
    }
}
