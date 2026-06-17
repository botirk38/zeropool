//! Pool performance metrics with zero-cost atomic counters.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::size_class::NUM_CLASSES;

/// Atomic counters tracking pool performance.
///
/// All counters use `Relaxed` ordering — they are statistical and do not
/// synchronize other operations. Reading metrics has zero impact on the
/// hot path.
#[derive(Debug)]
pub(crate) struct Counters {
    /// Total `get()` calls.
    pub gets: AtomicU64,
    /// `get()` calls satisfied from the TLS cache (fastest path).
    pub tls_hits: AtomicU64,
    /// `get()` calls satisfied from the shared pool via batch refill.
    pub shared_hits: AtomicU64,
    /// `get()` calls that required a fresh allocation (cold path).
    pub allocations: AtomicU64,
    /// Total `put()` calls (buffers returned to the pool).
    pub puts: AtomicU64,
    /// Buffers discarded on return (below min_buffer_size or oversize).
    pub discards: AtomicU64,
    /// Oversize allocations that bypassed all classes.
    pub oversize: AtomicU64,
}

impl Counters {
    pub const fn new() -> Self {
        Self {
            gets: AtomicU64::new(0),
            tls_hits: AtomicU64::new(0),
            shared_hits: AtomicU64::new(0),
            allocations: AtomicU64::new(0),
            puts: AtomicU64::new(0),
            discards: AtomicU64::new(0),
            oversize: AtomicU64::new(0),
        }
    }

    /// Reset all counters to zero.
    pub fn reset(&self) {
        self.gets.store(0, Ordering::Relaxed);
        self.tls_hits.store(0, Ordering::Relaxed);
        self.shared_hits.store(0, Ordering::Relaxed);
        self.allocations.store(0, Ordering::Relaxed);
        self.puts.store(0, Ordering::Relaxed);
        self.discards.store(0, Ordering::Relaxed);
        self.oversize.store(0, Ordering::Relaxed);
    }
}

/// Point-in-time snapshot of pool statistics.
///
/// Returned by [`BufferPool::stats()`](crate::BufferPool::stats).
/// All values are approximate — counters use `Relaxed` ordering and
/// per-class counts may lag slightly under high concurrency.
///
/// # Example
///
/// ```
/// use zeropool::BufferPool;
///
/// let pool = BufferPool::new().min_buffer_size(0);
///
/// let buf = pool.get(4096);
/// drop(buf);
///
/// let s = pool.stats();
/// assert_eq!(s.gets, 1);
/// assert_eq!(s.puts, 1);
/// assert!(s.hit_rate >= 0.0);
/// ```
#[derive(Debug, Clone)]
pub struct PoolStats {
    // ── Aggregate counters ──────────────────────────────────────────
    /// Total `get()` calls.
    pub gets: u64,
    /// Calls satisfied from the thread-local cache (fastest path).
    pub tls_hits: u64,
    /// Calls satisfied from the shared pool via batch refill.
    pub shared_hits: u64,
    /// Calls that required a fresh heap allocation (cold path).
    pub allocations: u64,
    /// Total buffers returned to the pool.
    pub puts: u64,
    /// Buffers discarded on return (too small or oversize).
    pub discards: u64,
    /// Oversize allocations that bypassed all size classes.
    pub oversize: u64,

    // ── Derived rates ───────────────────────────────────────────────
    /// TLS cache hit rate in `[0.0, 1.0]`.
    pub tls_hit_rate: f64,
    /// Overall hit rate (TLS + shared) in `[0.0, 1.0]`.
    pub hit_rate: f64,

    // ── Per-class breakdown ─────────────────────────────────────────
    /// Approximate buffer count per size class in the shared pool.
    pub classes: [ClassInfo; NUM_CLASSES],
}

/// Per-class snapshot within [`PoolStats`].
#[derive(Debug, Clone, Copy)]
pub struct ClassInfo {
    /// Size class boundary in bytes (e.g. 4096, 16384, …).
    pub boundary: usize,
    /// Approximate buffers in the shared pool for this class.
    pub buffered: usize,
}

impl fmt::Display for PoolStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "gets: {} | puts: {} | hit_rate: {:.1}%",
            self.gets,
            self.puts,
            self.hit_rate * 100.0
        )?;
        writeln!(
            f,
            "  tls_hits: {} ({:.1}%) | shared_hits: {} | allocations: {} | discards: {} | oversize: {}",
            self.tls_hits,
            self.tls_hit_rate * 100.0,
            self.shared_hits,
            self.allocations,
            self.discards,
            self.oversize
        )?;
        for ci in &self.classes {
            if ci.buffered > 0 {
                writeln!(f, "  [{:>7}] {} buffered", format_size(ci.boundary), ci.buffered)?;
            }
        }
        Ok(())
    }
}

fn format_size(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{}MB", bytes / (1024 * 1024))
    } else if bytes >= 1024 {
        format!("{}KB", bytes / 1024)
    } else {
        format!("{bytes}B")
    }
}

/// Build a `PoolStats` snapshot from live counters + class table.
pub(crate) fn snapshot(counters: &Counters, classes: &[crate::size_class::SizeClass]) -> PoolStats {
    let gets = counters.gets.load(Ordering::Relaxed);
    let tls_hits = counters.tls_hits.load(Ordering::Relaxed);
    let shared_hits = counters.shared_hits.load(Ordering::Relaxed);

    let tls_hit_rate = if gets == 0 {
        0.0
    } else {
        tls_hits as f64 / gets as f64
    };
    let hit_rate = if gets == 0 {
        0.0
    } else {
        (tls_hits + shared_hits) as f64 / gets as f64
    };

    let mut class_info: [ClassInfo; NUM_CLASSES] =
        [ClassInfo { boundary: 0, buffered: 0 }; NUM_CLASSES];
    for (i, class) in classes.iter().enumerate() {
        class_info[i] = ClassInfo {
            boundary: class.class_size,
            buffered: class.len(),
        };
    }

    PoolStats {
        gets,
        tls_hits,
        shared_hits,
        allocations: counters.allocations.load(Ordering::Relaxed),
        puts: counters.puts.load(Ordering::Relaxed),
        discards: counters.discards.load(Ordering::Relaxed),
        oversize: counters.oversize.load(Ordering::Relaxed),
        tls_hit_rate,
        hit_rate,
        classes: class_info,
    }
}
