//! # ZeroPool — A User-Space Byte Allocator for Rust
//!
//! `ZeroPool` is a high-performance, thread-safe byte allocator that recycles
//! buffers through size-class bucketing and thread-local caching.
//!
//! - **Size-class bucketing**: Power-of-two classes (4KB→64MB) for O(1) class selection
//! - **Lock-free shared pool**: `crossbeam::ArrayQueue` per class — no mutexes
//! - **Thread-local caching**: Per-class LIFO caches with magazine-style batch transfer
//! - **Pool isolation**: Unique pool IDs prevent TLS cache cross-contamination
//! - **Pluggable allocator**: Custom buffer creation via the [`Allocator`] trait
//!
//! # Quick Start
//!
//! ```rust
//! use zeropool::ZeroPool;
//!
//! let pool = ZeroPool::new();
//!
//! let mut buf = pool.alloc(1024 * 1024); // 1MB — returned as RAII guard
//! buf[0] = 42;                            // Deref<Target = [u8]>
//! // automatically deallocated back to pool on drop
//! ```
//!
//! # Custom Configuration
//!
//! ```rust
//! use zeropool::ZeroPool;
//!
//! let pool = ZeroPool::new()
//!     .min_buffer_size(4096)           // discard buffers < 4KB on dealloc
//!     .tls_cache_size(8)               // 8 buffers per class per thread
//!     .max_buffers_per_class(64)       // 64 buffers per class in shared pool
//!     .batch_size(4);                  // transfer 4 at a time (magazine)
//! ```
//!
//! # Pluggable Allocator
//!
//! ```rust
//! use zeropool::{Allocator, ZeroPool};
//!
//! struct PrefaultAllocator;
//! impl Allocator for PrefaultAllocator {
//!     fn allocate(&self, capacity: usize) -> Vec<u8> {
//!         let mut buf = Vec::with_capacity(capacity);
//!         buf.resize(capacity, 0); // pre-fault pages
//!         buf.clear();
//!         buf
//!     }
//! }
//!
//! let pool = ZeroPool::new().allocator(PrefaultAllocator);
//! ```
//!
//! # Ownership
//!
//! When a [`Buf`] is dropped, it deallocates back to the pool.
//! Use [`Buf::into_inner()`] to extract the `Vec<u8>` without returning it.
//!
//! ```rust
//! use zeropool::ZeroPool;
//!
//! let pool = ZeroPool::new();
//!
//! // Normal: deallocates back to pool on drop
//! {
//!     let buf = pool.alloc(1024);
//! }
//!
//! // Extract ownership — does NOT return to pool
//! let buf = pool.alloc(1024);
//! let vec: Vec<u8> = buf.into_inner();
//! ```

mod allocator;
mod buf;
mod config;
mod pool;
mod size_class;
mod stats;
mod tls;

pub use allocator::{Allocator, HeapAllocator};
pub use buf::Buf;
pub use pool::ZeroPool;
pub use stats::{ClassInfo, Stats};

/// Allocate a buffer from a pool.
///
/// ```
/// use zeropool::{ZeroPool, pool_vec};
///
/// let pool = ZeroPool::new();
/// let buf = pool_vec!(pool, 4096);
/// assert_eq!(buf.len(), 4096);
/// ```
#[macro_export]
macro_rules! pool_vec {
    ($pool:expr, $size:expr) => {
        $pool.alloc($size)
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_operations() {
        let pool = ZeroPool::new().min_buffer_size(0).track_stats(true);
        let buf = pool.alloc(1024);
        assert_eq!(buf.len(), 1024);
        drop(buf);

        let buf2 = pool.alloc(1024);
        assert_eq!(buf2.len(), 1024);
    }

    #[test]
    fn test_buffer_sizing() {
        let pool = ZeroPool::new().min_buffer_size(0).track_stats(true);
        let buf = pool.alloc(2048);
        assert_eq!(buf.len(), 2048);
        drop(buf);

        let buf2 = pool.alloc(1024);
        assert_eq!(buf2.len(), 1024);
        assert!(buf2.capacity() >= 2048);
    }

    #[test]
    fn test_min_size_filtering() {
        use std::thread;

        let pool = ZeroPool::new().min_buffer_size(1024 * 1024).max_buffers_per_class(16);

        let tls_cache_size = pool.state.tls_cache_size;

        thread::scope(|s| {
            s.spawn(|| {
                for _ in 0..tls_cache_size {
                    let buf = pool.alloc(512);
                    drop(buf);
                }
                let small_buf = pool.alloc(512);
                drop(small_buf);
            });
        });

        assert_eq!(pool.len(), 0);

        thread::scope(|s| {
            s.spawn(|| {
                let mut buffers = Vec::new();
                for _ in 0..=tls_cache_size {
                    buffers.push(pool.alloc(2 * 1024 * 1024));
                }
                for buf in buffers {
                    drop(buf);
                }
            });
        });

        assert!(!pool.is_empty());
    }

    #[test]
    fn test_max_pool_size() {
        let pool = ZeroPool::new().min_buffer_size(0).max_buffers_per_class(4).tls_cache_size(2);

        let mut buffers = Vec::new();
        for _ in 0..20 {
            buffers.push(pool.alloc(4096));
        }
        for buf in buffers {
            drop(buf);
        }

        assert!(pool.len() <= 4);
    }

    #[test]
    fn test_thread_local_cache() {
        let pool = ZeroPool::new().min_buffer_size(0);
        let cache_size = pool.state.tls_cache_size;

        for _ in 0..cache_size {
            let buf = pool.alloc(4096);
            drop(buf);
        }

        assert_eq!(pool.len(), 0);

        for _ in 0..cache_size {
            let buf = pool.alloc(4096);
            assert_eq!(buf.len(), 4096);
        }
    }

    #[test]
    fn test_config_api() {
        let pool1 = ZeroPool::new();
        assert!(!pool1.is_empty() || pool1.is_empty());

        let pool2 = ZeroPool::new()
            .min_buffer_size(4096)
            .tls_cache_size(4)
            .max_buffers_per_class(16)
            .batch_size(2);

        assert_eq!(pool2.state.min_buffer_size, 4096);
        assert_eq!(pool2.state.tls_cache_size, 4);
        assert_eq!(pool2.state.batch_size, 2);

        let buf = pool2.alloc(8192);
        assert_eq!(buf.len(), 8192);
    }

    #[test]
    fn test_concurrent_access() {
        use std::thread;

        let pool = ZeroPool::new().min_buffer_size(0);

        thread::scope(|s| {
            for _ in 0..8 {
                s.spawn(|| {
                    for _ in 0..100 {
                        let buf = pool.alloc(4096);
                        assert_eq!(buf.len(), 4096);
                        drop(buf);
                    }
                });
            }
        });

        assert!(pool.len() < 1000);
    }

    #[test]
    fn test_warm() {
        let pool = ZeroPool::new().min_buffer_size(0);

        let initial_len = pool.len();

        pool.warm(10, 64 * 1024);

        assert!(pool.len() > initial_len);

        let buf = pool.alloc(64 * 1024);
        assert!(buf.capacity() >= 64 * 1024);
    }

    #[test]
    fn test_edge_cases() {
        use std::thread;

        let pool = ZeroPool::new().tls_cache_size(2).min_buffer_size(0);

        assert!(pool.is_empty());

        let buf_zero = pool.alloc(0);
        assert_eq!(buf_zero.len(), 0);
        drop(buf_zero);

        thread::scope(|s| {
            s.spawn(|| {
                let b1 = pool.alloc(4096);
                let b2 = pool.alloc(4096);
                drop(b1);
                drop(b2);

                let buf_large = pool.alloc(100 * 1024 * 1024);
                assert_eq!(buf_large.len(), 100 * 1024 * 1024);
                drop(buf_large);
            });
        });
    }

    #[test]
    fn test_size_class_routing() {
        use std::thread;

        let pool = ZeroPool::new().min_buffer_size(0).tls_cache_size(2);

        thread::scope(|s| {
            for _ in 0..4 {
                s.spawn(|| {
                    let mut buffers = vec![];
                    for &size in &[4096, 16384, 65536, 262_144] {
                        for _ in 0..4 {
                            buffers.push(pool.alloc(size));
                        }
                    }
                    for buf in buffers {
                        drop(buf);
                    }
                });
            }
        });

        assert!(!pool.is_empty());
    }

    #[test]
    fn test_drain() {
        let pool = ZeroPool::new().min_buffer_size(0).tls_cache_size(2);

        let mut buffers = vec![];
        for _ in 0..10 {
            buffers.push(pool.alloc(4096));
        }
        for buf in buffers {
            drop(buf);
        }

        assert!(!pool.is_empty());

        pool.drain();

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());

        let buf = pool.alloc(4096);
        assert_eq!(buf.len(), 4096);
    }

    #[test]
    fn test_pool_isolation() {
        let pool1 = ZeroPool::new().min_buffer_size(0);
        let pool2 = ZeroPool::new().min_buffer_size(0);

        assert_ne!(pool1.state.id, pool2.state.id);

        let buf1 = pool1.alloc(4096);
        drop(buf1);

        assert_eq!(pool2.len(), 0);
    }

    #[test]
    fn test_batch_transfer() {
        let pool = ZeroPool::new().min_buffer_size(0).tls_cache_size(2).batch_size(2);

        let mut buffers = Vec::new();
        for _ in 0..10 {
            buffers.push(pool.alloc(4096));
        }
        for buf in buffers {
            drop(buf);
        }

        assert!(!pool.is_empty());
    }

    #[test]
    fn test_stats_counters() {
        let pool = ZeroPool::new().min_buffer_size(0).track_stats(true);

        let s = pool.stats();
        assert_eq!(s.gets, 0);
        assert_eq!(s.puts, 0);

        let buf = pool.alloc(4096);
        let s = pool.stats();
        assert_eq!(s.gets, 1);
        assert_eq!(s.allocations, 1);

        drop(buf);
        let s = pool.stats();
        assert_eq!(s.puts, 1);
    }

    #[test]
    fn test_stats_hit_rates() {
        let pool = ZeroPool::new().min_buffer_size(0).tls_cache_size(4).track_stats(true);

        let buf = pool.alloc(4096);
        drop(buf);

        let buf = pool.alloc(4096);
        drop(buf);

        let s = pool.stats();
        assert_eq!(s.gets, 2);
        assert!(s.tls_hits >= 1);
        assert!(s.hit_rate > 0.0);
    }

    #[test]
    fn test_stats_oversize() {
        let pool = ZeroPool::new().min_buffer_size(0).track_stats(true);

        let buf = pool.alloc(128 * 1024 * 1024);
        let s = pool.stats();
        assert_eq!(s.oversize, 1);

        drop(buf);
        let s = pool.stats();
        assert_eq!(s.discards, 1);
    }

    #[test]
    fn test_stats_reset() {
        let pool = ZeroPool::new().min_buffer_size(0).track_stats(true);

        let buf = pool.alloc(4096);
        drop(buf);
        assert!(pool.stats().gets > 0);

        pool.reset_stats();
        let s = pool.stats();
        assert_eq!(s.gets, 0);
        assert_eq!(s.puts, 0);
    }

    #[test]
    fn test_stats_display() {
        let pool = ZeroPool::new().min_buffer_size(0).track_stats(true);
        let buf = pool.alloc(4096);
        drop(buf);

        let output = format!("{}", pool.stats());
        assert!(output.contains("gets: 1"));
        assert!(output.contains("puts: 1"));
    }

    #[test]
    fn test_stats_class_info() {
        let pool = ZeroPool::new().min_buffer_size(0);
        pool.warm(4, 4096);

        let s = pool.stats();
        assert_eq!(s.classes.len(), 8);
        assert!(s.classes.iter().any(|c| c.buffered >= 4));
    }

    #[test]
    fn test_pool_vec_macro() {
        let pool = ZeroPool::new();
        let buf = pool_vec!(pool, 1024);
        assert_eq!(buf.len(), 1024);
    }

    #[test]
    fn test_custom_allocator() {
        struct DoubleCapAllocator;
        impl Allocator for DoubleCapAllocator {
            fn allocate(&self, capacity: usize) -> Vec<u8> {
                Vec::with_capacity(capacity * 2)
            }
        }

        let pool = ZeroPool::new().allocator(DoubleCapAllocator).min_buffer_size(0);
        let buf = pool.alloc(4096);
        assert!(buf.capacity() >= 8192);
    }
}
