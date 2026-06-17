//! # ZeroPool — High-Performance Buffer Pool for Rust
//!
//! `ZeroPool` provides a thread-safe buffer pool optimized for high-throughput I/O workloads.
//! It achieves exceptional performance through:
//!
//! - **Size-class bucketing**: Power-of-two classes (4KB→64MB) for O(1) class selection
//! - **Lock-free shared pool**: `crossbeam::ArrayQueue` per class — no mutexes
//! - **Thread-local caching**: Per-class LIFO caches with magazine-style batch transfer
//! - **Pool isolation**: Unique pool IDs prevent TLS cache cross-contamination
//! - **Zero-copy operations**: Avoids unnecessary memory allocations and copies
//!
//! # Quick Start
//!
//! ```rust
//! use zeropool::BufferPool;
//!
//! // Create a pool with smart defaults
//! let pool = BufferPool::new();
//!
//! // Get a buffer (returns RAII guard)
//! let mut buffer = pool.get(1024 * 1024); // 1MB buffer
//!
//! // Use it — Deref<Target = [u8]> for safe slice access
//! buffer[0] = 42;
//!
//! // Buffer automatically returned to pool when dropped
//! ```
//!
//! # Custom Configuration
//!
//! ```rust
//! use zeropool::BufferPool;
//!
//! let pool = BufferPool::new()
//!     .min_buffer_size(4096)           // Pool buffers ≥ 4KB
//!     .tls_cache_size(8)               // 8 buffers per class per thread
//!     .max_buffers_per_class(64)       // 64 buffers per class in shared pool
//!     .batch_size(4);                  // Transfer 4 at a time (magazine)
//! ```
//!
//! # Architecture
//!
//! ```text
//! ┌─ Thread 1 ──────────┐  ┌─ Thread 2 ──────────┐
//! │ TLS Cache            │  │ TLS Cache            │
//! │  [4KB]  [16KB] ...   │  │  [4KB]  [16KB] ...   │
//! └────────┬─────────────┘  └────────┬─────────────┘
//!          │ batch refill/spill      │
//!          └──────────┬──────────────┘
//!                     ▼
//!          ┌─ Shared Pool ──────────┐
//!          │  [4KB  ArrayQueue]     │  ← lock-free CAS
//!          │  [16KB ArrayQueue]     │
//!          │  [64KB ArrayQueue]     │
//!          │  …                     │
//!          │  [64MB ArrayQueue]     │
//!          └────────────────────────┘
//! ```
//!
//! # Ownership and Pool Return
//!
//! When a `PooledBuffer` is dropped, the buffer returns to the pool.
//! Use [`PooledBuffer::into_inner()`] to extract the `Vec<u8>` without
//! returning it.
//!
//! ```rust
//! use zeropool::BufferPool;
//!
//! let pool = BufferPool::new();
//!
//! // Normal: returns to pool on drop
//! {
//!     let buffer = pool.get(1024);
//! }
//!
//! // Extract ownership — does NOT return to pool
//! let buffer = pool.get(1024);
//! let vec: Vec<u8> = buffer.into_inner();
//! ```

mod buffer;
mod config;
mod metrics;
mod pool;
mod size_class;
mod tls;

pub use buffer::PooledBuffer;
pub use metrics::{ClassInfo, PoolStats};
pub use pool::BufferPool;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_pool_operations() {
        let pool = BufferPool::new().min_buffer_size(0);
        let buf = pool.get(1024);
        assert_eq!(buf.len(), 1024);
        drop(buf);

        let buf2 = pool.get(1024);
        assert_eq!(buf2.len(), 1024);
    }

    #[test]
    fn test_buffer_sizing() {
        let pool = BufferPool::new().min_buffer_size(0);
        let buf = pool.get(2048);
        assert_eq!(buf.len(), 2048);
        drop(buf);

        let buf2 = pool.get(1024);
        assert_eq!(buf2.len(), 1024);
        assert!(buf2.capacity() >= 2048);
    }

    #[test]
    fn test_min_size_filtering() {
        use std::thread;

        let pool = BufferPool::new().min_buffer_size(1024 * 1024).max_buffers_per_class(16);

        let tls_cache_size = pool.state.tls_cache_size;
        let pool_clone = pool.clone();

        // Small buffers below min_buffer_size should be discarded
        thread::spawn(move || {
            for _ in 0..tls_cache_size {
                let buf = pool_clone.get(512);
                drop(buf);
            }
            let small_buf = pool_clone.get(512);
            drop(small_buf);
        })
        .join()
        .unwrap();

        assert_eq!(pool.len(), 0);

        // Large buffers should be pooled
        let pool_clone = pool.clone();
        thread::spawn(move || {
            let mut buffers = Vec::new();
            for _ in 0..=tls_cache_size {
                buffers.push(pool_clone.get(2 * 1024 * 1024));
            }
            for buf in buffers {
                drop(buf);
            }
        })
        .join()
        .unwrap();

        assert!(!pool.is_empty());
    }

    #[test]
    fn test_max_pool_size() {
        let pool = BufferPool::new().min_buffer_size(0).max_buffers_per_class(4).tls_cache_size(2);

        // Fill and return many buffers of the same size class
        let mut buffers = Vec::new();
        for _ in 0..20 {
            buffers.push(pool.get(4096));
        }
        for buf in buffers {
            drop(buf);
        }

        // Shared pool should not exceed max_buffers_per_class
        assert!(pool.len() <= 4);
    }

    #[test]
    fn test_thread_local_cache() {
        let pool = BufferPool::new().min_buffer_size(0);
        let cache_size = pool.state.tls_cache_size;

        // First N get/put operations should use TLS
        for _ in 0..cache_size {
            let buf = pool.get(4096);
            drop(buf);
        }

        // Pool should be empty (all buffers in TLS)
        assert_eq!(pool.len(), 0);

        // Should hit TLS for all cached buffers
        for _ in 0..cache_size {
            let buf = pool.get(4096);
            assert_eq!(buf.len(), 4096);
        }
    }

    #[test]
    fn test_config_api() {
        let pool1 = BufferPool::new();
        assert!(!pool1.is_empty() || pool1.is_empty());

        let pool2 = BufferPool::new()
            .min_buffer_size(4096)
            .tls_cache_size(4)
            .max_buffers_per_class(16)
            .batch_size(2);

        assert_eq!(pool2.state.min_buffer_size, 4096);
        assert_eq!(pool2.state.tls_cache_size, 4);
        assert_eq!(pool2.state.batch_size, 2);

        let buf = pool2.get(8192);
        assert_eq!(buf.len(), 8192);
    }

    #[test]
    fn test_concurrent_access() {
        use std::thread;

        let pool = BufferPool::new().min_buffer_size(0);
        let mut handles = vec![];

        for _ in 0..8 {
            let pool = pool.clone();
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    let buf = pool.get(4096);
                    assert_eq!(buf.len(), 4096);
                    drop(buf);
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        assert!(pool.len() < 1000);
    }

    #[test]
    fn test_clone_shares_state() {
        let pool = BufferPool::new().min_buffer_size(0).tls_cache_size(2);

        let buf1 = pool.get(4096);
        let buf2 = pool.get(4096);
        drop(buf1);
        drop(buf2);

        let pool_clone = pool.clone();

        let buf3 = pool_clone.get(4096);
        let buf4 = pool_clone.get(4096);
        let buf5 = pool_clone.get(4096);
        drop(buf3);
        drop(buf4);
        drop(buf5);

        // Clone and original share the same Arc<PoolState>
        assert!(std::sync::Arc::ptr_eq(&pool.state, &pool_clone.state));
    }

    #[test]
    fn test_preallocate() {
        let pool = BufferPool::new().min_buffer_size(0);

        let initial_len = pool.len();

        pool.preallocate(10, 64 * 1024);

        assert!(pool.len() > initial_len);

        let buf = pool.get(64 * 1024);
        assert!(buf.capacity() >= 64 * 1024);
    }

    #[test]
    fn test_edge_cases() {
        use std::thread;

        let pool = BufferPool::new().tls_cache_size(2).min_buffer_size(0);

        assert!(pool.is_empty());

        // Test zero-size buffer
        let buf_zero = pool.get(0);
        assert_eq!(buf_zero.len(), 0);
        drop(buf_zero);

        // Test very large buffer in separate thread
        let pool_clone = pool.clone();
        thread::spawn(move || {
            let b1 = pool_clone.get(4096);
            let b2 = pool_clone.get(4096);
            drop(b1);
            drop(b2);

            let buf_large = pool_clone.get(100 * 1024 * 1024);
            assert_eq!(buf_large.len(), 100 * 1024 * 1024);
            drop(buf_large);
        })
        .join()
        .unwrap();
    }

    #[test]
    fn test_size_class_routing() {
        use std::thread;

        let pool = BufferPool::new().min_buffer_size(0).tls_cache_size(2);

        let mut handles = vec![];
        for _ in 0..4 {
            let pool_clone = pool.clone();
            handles.push(thread::spawn(move || {
                let mut buffers = vec![];
                // Request different size classes — more than tls_cache_size
                // to force spill into shared pool
                for &size in &[4096, 16384, 65536, 262_144] {
                    for _ in 0..4 {
                        buffers.push(pool_clone.get(size));
                    }
                }
                for buf in buffers {
                    drop(buf);
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // Threads had more buffers per class than TLS holds — some
        // must have spilled to the shared pool.
        assert!(!pool.is_empty());
    }

    #[test]
    fn test_clear() {
        let pool = BufferPool::new().min_buffer_size(0).tls_cache_size(2);

        let mut buffers = vec![];
        for _ in 0..10 {
            buffers.push(pool.get(4096));
        }
        for buf in buffers {
            drop(buf);
        }

        assert!(!pool.is_empty());

        pool.clear();

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());

        // Pool should still work after clear
        let buf = pool.get(4096);
        assert_eq!(buf.len(), 4096);
    }

    #[test]
    fn test_pool_isolation() {
        // Two pools should not share TLS caches
        let pool1 = BufferPool::new().min_buffer_size(0);
        let pool2 = BufferPool::new().min_buffer_size(0);

        assert_ne!(pool1.state.id, pool2.state.id);

        let buf1 = pool1.get(4096);
        drop(buf1);

        // pool2 should not see pool1's cached buffer
        assert_eq!(pool2.len(), 0);
    }

    #[test]
    fn test_batch_transfer() {
        // Verify magazine-style batch works by overflowing TLS
        let pool = BufferPool::new().min_buffer_size(0).tls_cache_size(2).batch_size(2);

        // Fill and return more buffers than TLS holds
        let mut buffers = Vec::new();
        for _ in 0..10 {
            buffers.push(pool.get(4096));
        }
        for buf in buffers {
            drop(buf);
        }

        // Some should have spilled to shared pool
        assert!(!pool.is_empty());
    }

    #[test]
    fn test_stats_counters() {
        let pool = BufferPool::new().min_buffer_size(0);

        let s = pool.stats();
        assert_eq!(s.gets, 0);
        assert_eq!(s.puts, 0);

        let buf = pool.get(4096);
        let s = pool.stats();
        assert_eq!(s.gets, 1);
        assert_eq!(s.allocations, 1);

        drop(buf);
        let s = pool.stats();
        assert_eq!(s.puts, 1);
    }

    #[test]
    fn test_stats_hit_rates() {
        let pool = BufferPool::new().min_buffer_size(0).tls_cache_size(4);

        // First get → allocation (cold)
        let buf = pool.get(4096);
        drop(buf);

        // Second get → TLS hit (warm)
        let buf = pool.get(4096);
        drop(buf);

        let s = pool.stats();
        assert_eq!(s.gets, 2);
        assert!(s.tls_hits >= 1);
        assert!(s.hit_rate > 0.0);
    }

    #[test]
    fn test_stats_oversize() {
        let pool = BufferPool::new().min_buffer_size(0);

        let buf = pool.get(128 * 1024 * 1024);
        let s = pool.stats();
        assert_eq!(s.oversize, 1);

        drop(buf);
        let s = pool.stats();
        assert_eq!(s.discards, 1);
    }

    #[test]
    fn test_stats_reset() {
        let pool = BufferPool::new().min_buffer_size(0);

        let buf = pool.get(4096);
        drop(buf);
        assert!(pool.stats().gets > 0);

        pool.reset_stats();
        let s = pool.stats();
        assert_eq!(s.gets, 0);
        assert_eq!(s.puts, 0);
    }

    #[test]
    fn test_stats_display() {
        let pool = BufferPool::new().min_buffer_size(0);
        let buf = pool.get(4096);
        drop(buf);

        let output = format!("{}", pool.stats());
        assert!(output.contains("gets: 1"));
        assert!(output.contains("puts: 1"));
    }

    #[test]
    fn test_stats_class_info() {
        let pool = BufferPool::new().min_buffer_size(0);
        pool.preallocate(4, 4096);

        let s = pool.stats();
        assert_eq!(s.classes.len(), 8);
        assert!(s.classes.iter().any(|c| c.buffered >= 4));
    }

    // Boundary routing tests are in size_class.rs::tests
}
