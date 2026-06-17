use crossbeam_queue::ArrayQueue;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Number of size classes.
pub(crate) const NUM_CLASSES: usize = 8;

/// Power-of-two size class boundaries.
const CLASS_SIZES: [usize; NUM_CLASSES] = [
    4 * 1024,         // Class 0:  ≤ 4KB
    16 * 1024,        // Class 1:  ≤ 16KB
    64 * 1024,        // Class 2:  ≤ 64KB
    256 * 1024,       // Class 3:  ≤ 256KB
    1024 * 1024,      // Class 4:  ≤ 1MB
    4 * 1024 * 1024,  // Class 5:  ≤ 4MB
    16 * 1024 * 1024, // Class 6:  ≤ 16MB
    64 * 1024 * 1024, // Class 7:  ≤ 64MB
];

/// Bit-width of the smallest class boundary (4KB = 2^12).
const MIN_CLASS_BITS: u32 = 12;
/// Classes are spaced 2 bits apart (×4 per step).
const BITS_PER_STEP: u32 = 2;

// ── ClassTable ─────────────────────────────────────────────────────────

/// Routes buffer sizes to size classes using O(1) CLZ bit manipulation.
///
/// Owns the array of [`SizeClass`] queues and the power-of-two boundary
/// layout. Single source of truth for "which class handles this size?"
#[derive(Debug)]
pub(crate) struct ClassTable {
    classes: Box<[SizeClass]>,
}

impl ClassTable {
    /// Build a table with one [`SizeClass`] per boundary.
    pub fn new(max_buffers_per_class: usize) -> Self {
        let classes: Vec<SizeClass> = CLASS_SIZES
            .iter()
            .map(|&size| SizeClass::new(size, max_buffers_per_class))
            .collect();
        Self { classes: classes.into_boxed_slice() }
    }

    /// Route a requested buffer size to the smallest class that can satisfy it.
    ///
    /// Returns `(class_index, &SizeClass)` or `None` for oversize (> 64 MB).
    #[inline]
    pub fn route(&self, size: usize) -> Option<(usize, &SizeClass)> {
        let idx = route_size(size)?;
        Some((idx, &self.classes[idx]))
    }

    /// Route a buffer's actual capacity to the largest class it can serve.
    ///
    /// Returns `(class_index, &SizeClass)` or `None` if below the smallest class.
    #[inline]
    pub fn route_capacity(&self, capacity: usize) -> Option<(usize, &SizeClass)> {
        let idx = route_capacity(capacity)?;
        Some((idx, &self.classes[idx]))
    }

    /// Boundary size for a given class index.
    #[inline]
    pub fn boundary(class_idx: usize) -> usize {
        CLASS_SIZES[class_idx]
    }

    /// Allocate an oversize buffer that bypasses all classes.
    #[cold]
    pub fn oversize(size: usize) -> Vec<u8> {
        let mut buf = Vec::with_capacity(size);
        // SAFETY: capacity == size; all u8 bit patterns valid.
        #[allow(clippy::uninit_vec)]
        unsafe {
            buf.set_len(size);
        }
        buf
    }

    /// Total buffered count across all classes (approximate).
    pub fn total_buffered(&self) -> usize {
        self.classes.iter().map(SizeClass::len).sum()
    }

    /// Whether all classes are empty.
    pub fn all_empty(&self) -> bool {
        self.classes.iter().all(SizeClass::is_empty)
    }

    /// Drain all classes.
    pub fn clear_all(&self) {
        for class in &*self.classes {
            class.clear();
        }
    }
}

impl std::ops::Index<usize> for ClassTable {
    type Output = SizeClass;

    #[inline]
    fn index(&self, idx: usize) -> &SizeClass {
        &self.classes[idx]
    }
}

// ── Routing functions (private to module) ──────────────────────────────

/// Map a requested buffer size to the smallest class index.
///
/// Uses `leading_zeros` for O(1) lookup instead of a linear scan.
#[inline]
fn route_size(size: usize) -> Option<usize> {
    if size == 0 {
        return Some(0);
    }
    let bits = usize::BITS - (size - 1).leading_zeros();
    if bits <= MIN_CLASS_BITS {
        return Some(0);
    }
    let class = (bits - MIN_CLASS_BITS).div_ceil(BITS_PER_STEP) as usize;
    if class < NUM_CLASSES {
        Some(class)
    } else {
        None
    }
}

/// Map a buffer's actual capacity to the largest class index it can serve.
///
/// Uses `leading_zeros` for O(1) lookup instead of a linear scan.
#[inline]
fn route_capacity(capacity: usize) -> Option<usize> {
    if capacity < CLASS_SIZES[0] {
        return None;
    }
    let bits = usize::BITS - capacity.leading_zeros();
    let class = ((bits - 1 - MIN_CLASS_BITS) / BITS_PER_STEP) as usize;
    Some(class.min(NUM_CLASSES - 1))
}

// ── SizeClass ──────────────────────────────────────────────────────────

/// A single size class with a lock-free bounded queue.
///
/// Owns the allocation policy (class boundary) and the recycled-buffer
/// storage ([`crossbeam_queue::ArrayQueue`]).
#[derive(Debug)]
pub(crate) struct SizeClass {
    queue: ArrayQueue<Vec<u8>>,
    /// The size class boundary (all buffers have capacity ≥ this).
    pub class_size: usize,
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
        // routing). All u8 bit patterns are valid; the caller treats
        // the content as uninitialized.
        #[allow(clippy::uninit_vec)]
        unsafe {
            buf.set_len(requested_len);
        }
        buf
    }

    /// Resize a recycled buffer to the requested length.
    #[inline(always)]
    pub fn resize(buf: &mut Vec<u8>, requested_len: usize) {
        debug_assert!(
            requested_len <= buf.capacity(),
            "requested_len ({requested_len}) > capacity ({})",
            buf.capacity(),
        );
        // SAFETY: requested_len ≤ capacity (asserted above in debug,
        // guaranteed by class routing in release). All u8 patterns valid.
        unsafe { buf.set_len(requested_len) };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_size_boundaries() {
        assert_eq!(route_size(0), Some(0));
        assert_eq!(route_size(1), Some(0));

        for (i, &boundary) in CLASS_SIZES.iter().enumerate() {
            assert_eq!(route_size(boundary), Some(i), "boundary {boundary}");
            if i + 1 < NUM_CLASSES {
                assert_eq!(route_size(boundary + 1), Some(i + 1), "boundary+1 {}", boundary + 1);
            }
        }

        assert_eq!(route_size(CLASS_SIZES[NUM_CLASSES - 1] + 1), None);
        assert_eq!(route_size(usize::MAX), None);
    }

    #[test]
    fn route_capacity_boundaries() {
        assert_eq!(route_capacity(0), None);
        assert_eq!(route_capacity(CLASS_SIZES[0] - 1), None);

        for (i, &boundary) in CLASS_SIZES.iter().enumerate() {
            assert_eq!(route_capacity(boundary), Some(i), "boundary {boundary}");
        }

        assert_eq!(route_capacity(CLASS_SIZES[0] + 1), Some(0));
        assert_eq!(route_capacity(CLASS_SIZES[1] - 1), Some(0));

        assert_eq!(route_capacity(CLASS_SIZES[NUM_CLASSES - 1] + 1), Some(NUM_CLASSES - 1));
        assert_eq!(route_capacity(usize::MAX), Some(NUM_CLASSES - 1));
    }

    #[test]
    fn class_table_route_returns_correct_class() {
        let table = ClassTable::new(32);
        let (idx, class) = table.route(4096).unwrap();
        assert_eq!(idx, 0);
        assert_eq!(class.class_size, 4 * 1024);

        let (idx, class) = table.route(65536).unwrap();
        assert_eq!(idx, 2);
        assert_eq!(class.class_size, 64 * 1024);

        assert!(table.route(128 * 1024 * 1024).is_none());
    }
}
