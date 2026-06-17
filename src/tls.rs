use std::cell::UnsafeCell;

use crate::size_class::{NUM_CLASSES, SizeClass};

/// Consolidated thread-local state for one pool instance.
///
/// Combines what was previously three separate `thread_local!` declarations
/// into a single struct, reducing TLS lookup overhead on every `get`/`put`.
pub(crate) struct TlsState {
    /// Which pool instance owns this cache (0 = uninitialized).
    pub pool_id: u64,
    /// Per-size-class buffer caches.
    pub caches: [Vec<Vec<u8>>; NUM_CLASSES],
    /// Max buffers per class in this thread's cache.
    pub limit: usize,
}

thread_local! {
    /// Single consolidated TLS slot for the buffer pool.
    ///
    /// Uses `UnsafeCell` instead of `RefCell` to eliminate runtime borrow
    /// checking overhead (~5ns per access). This is safe because TLS is
    /// inherently single-threaded — only one `&mut` can exist at a time.
    static TLS: UnsafeCell<TlsState> = const { UnsafeCell::new(TlsState::new()) };
}

impl TlsState {
    /// Create an uninitialized TLS state.
    pub const fn new() -> Self {
        Self {
            pool_id: 0,
            caches: [
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            ],
            limit: 0,
        }
    }

    /// Access the current thread's TLS state mutably.
    ///
    /// # Safety contract
    ///
    /// This is safe because:
    /// 1. `thread_local!` ensures only the owning thread can access the cell.
    /// 2. The closure `f` has exclusive access — we never create overlapping
    ///    `&mut` references since our call sites (pool get/put) are non-reentrant.
    #[inline(always)]
    pub fn with_current<R>(f: impl FnOnce(&mut Self) -> R) -> R {
        TLS.with(|cell| {
            // SAFETY: TLS is single-threaded. Our call sites (pool.get/put)
            // never re-enter with_current while a previous borrow is alive.
            let state = unsafe { &mut *cell.get() };
            f(state)
        })
    }

    /// Whether this TLS state belongs to the given pool.
    #[inline(always)]
    pub fn belongs_to(&self, pool_id: u64) -> bool {
        self.pool_id == pool_id
    }

    /// Initialize (or reinitialize) this state for a specific pool.
    #[inline]
    pub fn init_for(&mut self, pool_id: u64, limit: usize) {
        if self.pool_id != pool_id {
            for cache in &mut self.caches {
                cache.clear();
            }
        }
        self.pool_id = pool_id;
        self.limit = limit;
    }

    /// Magazine-style batch refill: move up to `batch` buffers from the
    /// shared [`SizeClass`] queue into this thread's local cache.
    ///
    /// Returns how many buffers were moved.
    #[inline]
    pub fn batch_refill(&mut self, class_idx: usize, class: &SizeClass, batch: usize) -> usize {
        let mut moved = 0;
        while moved < batch {
            if let Some(buf) = class.pop() {
                self.caches[class_idx].push(buf);
                moved += 1;
            } else {
                break;
            }
        }
        moved
    }

    /// Magazine-style batch spill: move up to `batch` buffers from this
    /// thread's local cache back to the shared [`SizeClass`] queue.
    #[inline]
    pub fn batch_spill(&mut self, class_idx: usize, class: &SizeClass, batch: usize) {
        for _ in 0..batch {
            if let Some(buf) = self.caches[class_idx].pop() {
                if class.push(buf).is_err() {
                    break;
                }
            } else {
                break;
            }
        }
    }
}
