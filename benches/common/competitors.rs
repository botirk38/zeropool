//! Wrappers and factories for the comparison pool/buffer libraries.
//!
//! This module is intentionally feature-gated so the rest of the bench
//! suite can run without pulling in the comparison crates.

#![allow(dead_code)]

#[cfg(feature = "bench")]
use opool::{Pool as OpoolPool, PoolAllocator as OpoolPoolAllocator};
#[cfg(feature = "bench")]
use std::sync::Arc;

/// Default hot-path workload size for the multi-thread benchmarks.
pub const HOT_PATH_SIZE: usize = 64 * 1024;

/// Build a fresh `ZeroPool` configured for benchmark use.
pub fn zeropool_default() -> zeropool::ZeroPool {
    zeropool::ZeroPool::new()
}

/// Build a `ZeroPool` with stats tracking enabled.
pub fn zeropool_with_stats() -> zeropool::ZeroPool {
    zeropool::ZeroPool::new().track_stats(true)
}

/// Build a `ZeroPool` with `min_buffer_size(0)` so small buffers are pooled.
pub fn zeropool_no_min() -> zeropool::ZeroPool {
    zeropool::ZeroPool::new().min_buffer_size(0)
}

// ── object_pool ───────────────────────────────────────────────────────

#[cfg(feature = "bench")]
pub fn object_pool_for_size(size: usize, capacity: usize) -> Arc<object_pool::Pool<Vec<u8>>> {
    Arc::new(object_pool::Pool::new(capacity, move || vec![0u8; size]))
}

#[cfg(feature = "bench")]
pub fn object_pool_zeroed(size: usize, capacity: usize) -> Arc<object_pool::Pool<Vec<u8>>> {
    Arc::new(object_pool::Pool::new(capacity, move || vec![0u8; size]))
}

// ── opool ─────────────────────────────────────────────────────────────

#[cfg(feature = "bench")]
pub struct SizedVecAlloc(pub usize);

#[cfg(feature = "bench")]
impl OpoolPoolAllocator<Vec<u8>> for SizedVecAlloc {
    fn allocate(&self) -> Vec<u8> {
        Vec::with_capacity(self.0)
    }

    fn reset(&self, _obj: &mut Vec<u8>) {}
}

#[cfg(feature = "bench")]
pub struct ZeroedVecAlloc(pub usize);

#[cfg(feature = "bench")]
impl OpoolPoolAllocator<Vec<u8>> for ZeroedVecAlloc {
    fn allocate(&self) -> Vec<u8> {
        vec![0u8; self.0]
    }

    fn reset(&self, _obj: &mut Vec<u8>) {}
}

#[cfg(feature = "bench")]
pub fn opool_capacity(capacity: usize, size: usize) -> Arc<OpoolPool<ZeroedVecAlloc, Vec<u8>>> {
    Arc::new(OpoolPool::new(capacity, ZeroedVecAlloc(size)))
}

#[cfg(feature = "bench")]
pub fn opool_capacity_zero(capacity: usize, size: usize) -> Arc<OpoolPool<SizedVecAlloc, Vec<u8>>> {
    Arc::new(OpoolPool::new(capacity, SizedVecAlloc(size)))
}
