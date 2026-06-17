use crossbeam_queue::ArrayQueue;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Power-of-two size class boundaries.
///
/// Each class pools buffers with capacity in `(PREV_CLASS, THIS_CLASS]`.
/// Requests larger than the biggest class bypass pooling entirely.
pub(crate) const CLASS_SIZES: [usize; NUM_CLASSES] = [
    4 * 1024,         // Class 0:  ≤ 4KB
    16 * 1024,        // Class 1:  ≤ 16KB
    64 * 1024,        // Class 2:  ≤ 64KB
    256 * 1024,       // Class 3:  ≤ 256KB
    1024 * 1024,      // Class 4:  ≤ 1MB
    4 * 1024 * 1024,  // Class 5:  ≤ 4MB
    16 * 1024 * 1024, // Class 6:  ≤ 16MB
    64 * 1024 * 1024, // Class 7:  ≤ 64MB
];

/// Number of size classes.
pub(crate) const NUM_CLASSES: usize = 8;

/// Map a requested buffer size to the smallest class that can satisfy it.
///
/// Returns `None` for oversize requests (> 64MB) — these are allocated
/// directly and not pooled.
#[inline]
pub(crate) fn class_for_size(size: usize) -> Option<usize> {
    CLASS_SIZES.iter().position(|&boundary| size <= boundary)
}

/// Map a buffer's actual capacity to the largest class it can serve.
///
/// Used when returning a buffer: place it in the largest class whose
/// boundary fits within the buffer's capacity, so any future request
/// to that class is guaranteed to fit.
///
/// Returns `None` if the capacity is smaller than the smallest class.
#[inline]
pub(crate) fn class_for_capacity(capacity: usize) -> Option<usize> {
    let mut result = None;
    for (i, &boundary) in CLASS_SIZES.iter().enumerate() {
        if capacity >= boundary {
            result = Some(i);
        } else {
            break;
        }
    }
    result
}

/// A single size class with a lock-free shared freelist.
///
/// Owns both the allocation policy (class boundary) and the recycled-buffer
/// storage (`ArrayQueue`). This is the core concurrency-safe entity that
/// replaces the old `Shard` + `Mutex<Vec<BufferEntry>>` design.
#[derive(Debug)]
pub(crate) struct SizeClass {
    /// Lock-free bounded queue of pooled buffers.
    queue: ArrayQueue<Vec<u8>>,
    /// The size class boundary (all buffers have capacity ≥ this).
    pub class_size: usize,
    /// Approximate count of buffers in the queue (diagnostic).
    count: AtomicUsize,
}

impl SizeClass {
    /// Create a new size class with the given boundary and queue capacity.
    pub fn new(class_size: usize, capacity: usize) -> Self {
        Self {
            queue: ArrayQueue::new(capacity),
            class_size,
            count: AtomicUsize::new(0),
        }
    }

    /// Allocate a fresh buffer for this size class.
    ///
    /// The returned `Vec<u8>` has `capacity == class_size` and
    /// `len == requested_len`. Bytes in `[0..requested_len)` are
    /// uninitialized but valid (all `u8` bit patterns are valid).
    #[cold]
    pub fn allocate(&self, requested_len: usize) -> Vec<u8> {
        let mut buf = Vec::with_capacity(self.class_size);
        // SAFETY: class_size >= requested_len (caller ensures this via
        // class_for_size). All u8 bit patterns are valid; the caller
        // treats the content as uninitialized.
        #[allow(clippy::uninit_vec)]
        unsafe {
            buf.set_len(requested_len);
        }
        buf
    }

    /// Allocate a fresh oversize buffer that exceeds all classes.
    ///
    /// Used when the requested size is larger than the biggest class.
    #[cold]
    pub fn allocate_oversize(size: usize) -> Vec<u8> {
        let mut buf = Vec::with_capacity(size);
        // SAFETY: capacity == size; all u8 bit patterns valid.
        #[allow(clippy::uninit_vec)]
        unsafe {
            buf.set_len(size);
        }
        buf
    }

    /// Prepare a recycled buffer for reuse at the given length.
    ///
    /// Assumes the buffer was previously pooled in this class, so
    /// `buf.capacity() >= self.class_size >= requested_len`.
    #[inline(always)]
    pub fn prepare_recycled(buf: &mut Vec<u8>, requested_len: usize) {
        debug_assert!(
            requested_len <= buf.capacity(),
            "requested_len ({requested_len}) > capacity ({})",
            buf.capacity(),
        );
        // SAFETY: requested_len ≤ capacity (asserted above in debug,
        // guaranteed by class routing in release). All u8 patterns valid.
        unsafe { buf.set_len(requested_len) };
    }

    /// Prepare a buffer for memory pinning: temporarily set len = capacity
    /// so that `pin_buffer` can lock the full allocation.
    ///
    /// Caller must call `buf.clear()` afterwards.
    pub fn fill_for_pinning(buf: &mut Vec<u8>) {
        // SAFETY: capacity was just allocated; all u8 patterns valid.
        unsafe { buf.set_len(buf.capacity()) };
    }

    /// Try to pop a buffer from this class.
    #[inline]
    pub fn pop(&self) -> Option<Vec<u8>> {
        self.queue.pop().inspect(|_| {
            self.count.fetch_sub(1, Ordering::Relaxed);
        })
    }

    /// Try to push a buffer into this class.
    ///
    /// Returns the buffer back if the queue is full.
    #[inline]
    pub fn push(&self, buf: Vec<u8>) -> Result<(), Vec<u8>> {
        self.queue.push(buf).map(|()| {
            self.count.fetch_add(1, Ordering::Relaxed);
        })
    }

    /// Approximate number of buffers in this class.
    #[inline]
    pub fn len(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }

    /// Whether this class has no buffered entries.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.count.load(Ordering::Relaxed) == 0
    }

    /// Drain all buffers from this class.
    pub fn clear(&self) {
        while self.queue.pop().is_some() {}
        self.count.store(0, Ordering::Relaxed);
    }
}
