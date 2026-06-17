use std::sync::Arc;

use crate::config::PoolConfig;
use crate::size_class::{CLASS_SIZES, SizeClass, class_for_capacity, class_for_size};
use crate::tls::TLS;
use crate::utils::pin_buffer;

/// Shared state behind a single `Arc`, avoiding per-clone copies of config.
#[derive(Debug)]
pub(crate) struct Inner {
    /// Unique pool instance ID for TLS isolation.
    pub id: u64,
    /// One lock-free queue per size class.
    pub classes: Box<[SizeClass]>,
    /// Pool configuration.
    pub config: PoolConfig,
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
    pub(crate) inner: Arc<Inner>,
}

impl BufferPool {
    /// Create a new buffer pool with system-aware defaults.
    ///
    /// # Example
    /// ```
    /// use zeropool::BufferPool;
    ///
    /// let pool = BufferPool::new();
    /// let buffer = pool.get(1024 * 1024);
    /// // Buffer automatically returned when dropped
    /// ```
    #[inline]
    pub fn new() -> Self {
        crate::Builder::default().build()
    }

    /// Create a builder for custom configuration.
    ///
    /// # Example
    /// ```
    /// use zeropool::BufferPool;
    ///
    /// let pool = BufferPool::builder()
    ///     .min_buffer_size(4096)
    ///     .tls_cache_size(4)
    ///     .build();
    /// ```
    #[inline]
    pub fn builder() -> crate::Builder {
        crate::Builder::default()
    }

    /// Create a pool from explicit configuration (internal).
    pub(crate) fn with_config(config: PoolConfig) -> Self {
        let classes: Vec<SizeClass> = CLASS_SIZES
            .iter()
            .map(|&size| SizeClass::new(size, config.max_buffers_per_class))
            .collect();

        Self {
            inner: Arc::new(Inner {
                id: config.id,
                classes: classes.into_boxed_slice(),
                config,
            }),
        }
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
        // Map size → class index; oversize bypasses pooling
        let Some(class_idx) = class_for_size(size) else {
            return crate::PooledBuffer::new(SizeClass::allocate_oversize(size), self.clone());
        };

        let class = &self.inner.classes[class_idx];

        // ── TLS fast path (lock-free) ──────────────────────────────
        let tls_result = TLS.with(|tls| {
            let mut state = tls.borrow_mut();

            // Initialize TLS for this pool on first access
            if state.pool_id == 0 {
                state.init_for(self.inner.id, self.inner.config.tls_cache_size);
            }

            // Skip TLS if it belongs to a different pool
            if !state.belongs_to(self.inner.id) {
                return None;
            }

            // LIFO pop from per-class cache
            if let Some(buf) = state.caches[class_idx].pop() {
                return Some(buf);
            }

            // Magazine-style batch refill from shared pool
            let refilled = state.batch_refill(class_idx, class, self.inner.config.batch_size);
            if refilled > 0 {
                return state.caches[class_idx].pop();
            }

            None
        });

        if let Some(mut buf) = tls_result {
            SizeClass::prepare_recycled(&mut buf, size);
            return crate::PooledBuffer::new(buf, self.clone());
        }

        // ── Shared pool direct pop (rare after batch refill) ───────
        if let Some(mut buf) = class.pop() {
            SizeClass::prepare_recycled(&mut buf, size);
            return crate::PooledBuffer::new(buf, self.clone());
        }

        // ── Cold path: fresh allocation ────────────────────────────
        crate::PooledBuffer::new(class.allocate(size), self.clone())
    }

    /// Return a buffer to the pool for reuse.
    ///
    /// Called automatically by [`PooledBuffer::drop`](crate::PooledBuffer).
    #[inline]
    pub(crate) fn put(&self, mut buffer: Vec<u8>) {
        buffer.clear();
        let cap = buffer.capacity();

        // Drop buffers below the minimum poolable size
        if cap < self.inner.config.min_buffer_size {
            return;
        }

        // Find which class this buffer belongs to by capacity
        let Some(class_idx) = class_for_capacity(cap) else {
            return; // smaller than smallest class — drop
        };

        // Pin if configured
        if self.inner.config.pinned_memory {
            pin_buffer(&buffer);
        }

        // ── TLS fast path ──────────────────────────────────────────
        let overflow = TLS.with(|tls| {
            let mut state = tls.borrow_mut();

            // Initialize TLS for this pool on first access
            if state.pool_id == 0 {
                state.init_for(self.inner.id, self.inner.config.tls_cache_size);
            }

            // Wrong pool — go straight to shared
            if !state.belongs_to(self.inner.id) {
                return Some(buffer);
            }

            let class = &self.inner.classes[class_idx];

            // Spill excess before pushing (magazine-style)
            if state.caches[class_idx].len() >= state.limit {
                state.batch_spill(class_idx, class, self.inner.config.batch_size);
            }

            // Try TLS cache
            if state.caches[class_idx].len() < state.limit {
                state.caches[class_idx].push(buffer);
                return None; // stored
            }

            Some(buffer) // TLS still full after spill
        });

        // ── Shared pool fallback ───────────────────────────────────
        if let Some(buf) = overflow {
            let _ = self.inner.classes[class_idx].push(buf);
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
    /// let pool = BufferPool::builder().min_buffer_size(0).build();
    /// pool.preallocate(16, 64 * 1024); // 16 × 64KB buffers
    /// ```
    pub fn preallocate(&self, count: usize, size: usize) {
        let Some(class_idx) = class_for_size(size) else {
            return; // oversize — nothing to preallocate into
        };
        let class = &self.inner.classes[class_idx];

        for _ in 0..count {
            let mut buf = Vec::with_capacity(class.class_size);

            if self.inner.config.pinned_memory {
                SizeClass::fill_for_pinning(&mut buf);
                pin_buffer(&buf);
                buf.clear();
            }

            if class.push(buf).is_err() {
                break; // queue full
            }
        }
    }

    /// Total number of buffers across all shared size classes.
    ///
    /// Does not include thread-local cached buffers.
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.classes.iter().map(SizeClass::len).sum()
    }

    /// Whether all shared size classes are empty.
    ///
    /// Does not check thread-local caches.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.classes.iter().all(SizeClass::is_empty)
    }

    /// Drain all buffers from all shared size classes.
    ///
    /// Thread-local caches are NOT cleared.
    pub fn clear(&self) {
        for class in &*self.inner.classes {
            class.clear();
        }
    }
}

impl Default for BufferPool {
    fn default() -> Self {
        Self::new()
    }
}
