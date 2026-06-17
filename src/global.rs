//! Feature-gated global allocator backed by size-class caching.
//!
//! Enable with `features = ["global-alloc"]` in `Cargo.toml`.
//!
//! ```ignore
//! #[global_allocator]
//! static ALLOC: zeropool::ZeroPoolGlobalAlloc = zeropool::ZeroPoolGlobalAlloc::new();
//! ```

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

// ── Size classes ───────────────────────────────────────────────────────

/// Number of size classes for the global allocator.
const NUM_CLASSES: usize = 8;

/// Power-of-two size class boundaries (same layout as the pool).
const CLASS_SIZES: [usize; NUM_CLASSES] = [
    4 * 1024,         // 4KB
    16 * 1024,        // 16KB
    64 * 1024,        // 64KB
    256 * 1024,       // 256KB
    1024 * 1024,      // 1MB
    4 * 1024 * 1024,  // 4MB
    16 * 1024 * 1024, // 16MB
    64 * 1024 * 1024, // 64MB
];

/// Default per-class cache limit.
const DEFAULT_MAX_CACHED: usize = 64;

/// Minimum allocation size to store the intrusive `FreeNode` pointer.
const MIN_BLOCK_SIZE: usize = size_of::<FreeNode>();

/// Minimum alignment for cached blocks (must fit a pointer).
const MIN_BLOCK_ALIGN: usize = align_of::<FreeNode>();

// ── Intrusive free node ────────────────────────────────────────────────

/// Stored in the first bytes of a freed block. Zero additional allocation.
#[repr(C)]
struct FreeNode {
    next: *mut FreeNode,
}

// ── Lock-free Treiber stack freelist ───────────────────────────────────

/// Per-class cache using an intrusive Treiber stack.
///
/// Freed blocks store their `next` pointer in their own memory.
/// Push/pop via `AtomicPtr` CAS — no locks, no heap allocation for metadata.
struct Freelist {
    head: AtomicPtr<FreeNode>,
    count: AtomicUsize,
    max: usize,
    class_size: usize,
}

impl Freelist {
    const fn new(class_size: usize, max: usize) -> Self {
        Self {
            head: AtomicPtr::new(std::ptr::null_mut()),
            count: AtomicUsize::new(0),
            max,
            class_size,
        }
    }

    /// Try to pop a cached block from the freelist.
    #[inline]
    fn pop(&self) -> *mut u8 {
        let mut head = self.head.load(Ordering::Acquire);
        loop {
            if head.is_null() {
                return std::ptr::null_mut();
            }
            // SAFETY: `head` was pushed by `push()` which wrote a valid
            // `FreeNode` into a block whose layout satisfies alignment
            // and size requirements. The block is not concurrently accessed
            // because only one CAS winner reads `next`.
            let next = unsafe { (*head).next };
            match self.head.compare_exchange_weak(head, next, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => {
                    self.count.fetch_sub(1, Ordering::Relaxed);
                    return head.cast::<u8>();
                }
                Err(actual) => head = actual,
            }
        }
    }

    /// Try to push a freed block onto the freelist.
    ///
    /// Returns `true` if cached, `false` if the cache is full.
    ///
    /// # Safety
    ///
    /// `ptr` must point to a block with at least `MIN_BLOCK_SIZE` bytes
    /// and `MIN_BLOCK_ALIGN` alignment. The caller must not use the
    /// block after this call.
    #[inline]
    unsafe fn push(&self, ptr: *mut u8) -> bool {
        if self.count.load(Ordering::Relaxed) >= self.max {
            return false;
        }
        // SAFETY: caller guarantees ptr has MIN_BLOCK_ALIGN alignment
        // (>= align_of::<FreeNode>()) and MIN_BLOCK_SIZE bytes (>= size_of::<FreeNode>()).
        // The cast from *mut u8 to *mut FreeNode is valid because the caller
        // verifies alignment >= align_of::<FreeNode>() before calling push().
        #[allow(clippy::cast_ptr_alignment)]
        let node = ptr.cast::<FreeNode>();
        let mut head = self.head.load(Ordering::Acquire);
        loop {
            // SAFETY: caller guarantees ptr is valid, aligned, and large
            // enough for FreeNode. We own the block exclusively.
            unsafe { (*node).next = head };
            match self.head.compare_exchange_weak(head, node, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => {
                    self.count.fetch_add(1, Ordering::Relaxed);
                    return true;
                }
                Err(actual) => head = actual,
            }
        }
    }
}

// ── Route size → class ─────────────────────────────────────────────────

/// O(1) size-class routing via CLZ, matching the pool's algorithm.
#[inline]
fn route(size: usize) -> Option<usize> {
    if size == 0 || size > CLASS_SIZES[NUM_CLASSES - 1] {
        return None;
    }
    if size <= CLASS_SIZES[0] {
        return Some(0);
    }
    let bits = (usize::BITS - (size - 1).leading_zeros()) as usize;
    let idx = bits.saturating_sub(12).div_ceil(2);
    if idx < NUM_CLASSES { Some(idx) } else { None }
}

// ── ZeroPoolGlobalAlloc ────────────────────────────────────────────────

/// Global allocator with ZeroPool-style size-class caching.
///
/// Caches freed blocks in lock-free Treiber stacks per size class.
/// Delegates actual memory management to [`std::alloc::System`].
///
/// # Usage
///
/// ```ignore
/// use zeropool::ZeroPoolGlobalAlloc;
///
/// #[global_allocator]
/// static ALLOC: ZeroPoolGlobalAlloc = ZeroPoolGlobalAlloc::new();
/// ```
///
/// # How it works
///
/// - **`alloc`**: Routes `layout.size()` to a size class. If the freelist
///   has a cached block, pops it (CAS). Otherwise falls through to `System`.
/// - **`dealloc`**: Routes the size to a class. If under the per-class
///   cache limit, pushes the block onto the freelist (CAS). Otherwise
///   falls through to `System.dealloc`.
///
/// All cached blocks are allocated at the class boundary size (e.g. a
/// 5KB allocation uses an 16KB block from class 1). This wastes some
/// memory but ensures any block in a freelist can satisfy any request
/// in that class.
pub struct ZeroPoolGlobalAlloc {
    classes: [Freelist; NUM_CLASSES],
}

impl std::fmt::Debug for ZeroPoolGlobalAlloc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZeroPoolGlobalAlloc")
            .field("num_classes", &NUM_CLASSES)
            .finish()
    }
}

impl Default for ZeroPoolGlobalAlloc {
    fn default() -> Self {
        Self::new()
    }
}

impl ZeroPoolGlobalAlloc {
    /// Create a new global allocator with default cache limits (64 per class).
    pub const fn new() -> Self {
        Self::with_max_cached(DEFAULT_MAX_CACHED)
    }

    /// Create a new global allocator with a custom per-class cache limit.
    pub const fn with_max_cached(max: usize) -> Self {
        // const fn requires manual array init (no iterator).
        Self {
            classes: [
                Freelist::new(CLASS_SIZES[0], max),
                Freelist::new(CLASS_SIZES[1], max),
                Freelist::new(CLASS_SIZES[2], max),
                Freelist::new(CLASS_SIZES[3], max),
                Freelist::new(CLASS_SIZES[4], max),
                Freelist::new(CLASS_SIZES[5], max),
                Freelist::new(CLASS_SIZES[6], max),
                Freelist::new(CLASS_SIZES[7], max),
            ],
        }
    }

    /// Compute the layout to use for a given class (class boundary size, proper alignment).
    #[inline]
    fn class_layout(class_size: usize, align: usize) -> Layout {
        let effective_align = align.max(MIN_BLOCK_ALIGN);
        // SAFETY: class_size is always a power-of-two >= 4096,
        // which is always >= effective_align.
        unsafe { Layout::from_size_align_unchecked(class_size, effective_align) }
    }
}

// SAFETY: All operations delegate to `System` for actual allocation.
// Freelists use atomic CAS — no data races. Blocks are only accessed
// by one thread at a time (either cached in the freelist or owned by
// the caller). FreeNode is written only when the block is not in use.
unsafe impl GlobalAlloc for ZeroPoolGlobalAlloc {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size = layout.size();

        // Skip caching for blocks too small to hold a FreeNode or misaligned.
        if size < MIN_BLOCK_SIZE || layout.align() > MIN_BLOCK_ALIGN {
            // SAFETY: delegating to System with the original layout.
            return unsafe { System.alloc(layout) };
        }

        if let Some(idx) = route(size) {
            let freelist = &self.classes[idx];
            let ptr = freelist.pop();
            if !ptr.is_null() {
                return ptr;
            }
            // Cache miss — allocate at class boundary size for future reuse.
            let class_layout = Self::class_layout(freelist.class_size, layout.align());
            // SAFETY: class_layout has valid size (>0) and alignment.
            return unsafe { System.alloc(class_layout) };
        }

        // Oversize — pass through to System.
        // SAFETY: delegating to System with the original layout.
        unsafe { System.alloc(layout) }
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let size = layout.size();

        if size < MIN_BLOCK_SIZE || layout.align() > MIN_BLOCK_ALIGN {
            // SAFETY: ptr was allocated by System with this layout.
            unsafe { System.dealloc(ptr, layout) };
            return;
        }

        if let Some(idx) = route(size) {
            let freelist = &self.classes[idx];
            // SAFETY: ptr is valid, was allocated with at least class_size
            // bytes (>= MIN_BLOCK_SIZE) and proper alignment (>= MIN_BLOCK_ALIGN).
            if unsafe { freelist.push(ptr) } {
                return;
            }
            // Cache full — deallocate via System at the class boundary layout.
            let class_layout = Self::class_layout(freelist.class_size, layout.align());
            // SAFETY: ptr was allocated with class_layout by System.
            unsafe { System.dealloc(ptr, class_layout) };
            return;
        }

        // Oversize — pass through to System.
        // SAFETY: ptr was allocated by System with this layout.
        unsafe { System.dealloc(ptr, layout) };
    }
}

// SAFETY: ZeroPoolGlobalAlloc only contains atomic data (AtomicPtr, AtomicUsize)
// and immutable config values. All mutation is through atomic CAS.
unsafe impl Send for ZeroPoolGlobalAlloc {}
// SAFETY: Same reasoning — all shared state is atomic.
unsafe impl Sync for ZeroPoolGlobalAlloc {}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_route_boundaries() {
        assert_eq!(route(0), None);
        assert_eq!(route(1), Some(0));
        assert_eq!(route(4096), Some(0));
        assert_eq!(route(4097), Some(1));
        assert_eq!(route(16384), Some(1));
        assert_eq!(route(16385), Some(2));
        assert_eq!(route(64 * 1024), Some(2));
        assert_eq!(route(64 * 1024 + 1), Some(3));
        assert_eq!(route(64 * 1024 * 1024), Some(7));
        assert_eq!(route(64 * 1024 * 1024 + 1), None);
    }

    #[test]
    fn test_freelist_push_pop() {
        let freelist = Freelist::new(4096, 8);
        let layout = Layout::from_size_align(4096, 8).unwrap();

        // SAFETY: valid layout, System allocator.
        let ptr = unsafe { System.alloc(layout) };
        assert!(!ptr.is_null());

        // SAFETY: ptr is 4096 bytes, 8-aligned (>= MIN_BLOCK_SIZE/ALIGN).
        assert!(unsafe { freelist.push(ptr) });
        assert_eq!(freelist.count.load(Ordering::Relaxed), 1);

        let popped = freelist.pop();
        assert_eq!(popped, ptr);
        assert_eq!(freelist.count.load(Ordering::Relaxed), 0);

        assert!(freelist.pop().is_null());

        // SAFETY: ptr was allocated with this layout.
        unsafe { System.dealloc(ptr, layout) };
    }

    #[test]
    fn test_freelist_max_limit() {
        let freelist = Freelist::new(4096, 2);
        let layout = Layout::from_size_align(4096, 8).unwrap();

        // SAFETY: valid layout, System allocator.
        let p1 = unsafe { System.alloc(layout) };
        // SAFETY: valid layout, System allocator.
        let p2 = unsafe { System.alloc(layout) };
        // SAFETY: valid layout, System allocator.
        let p3 = unsafe { System.alloc(layout) };

        // SAFETY: all pointers are 4096 bytes, 8-aligned.
        assert!(unsafe { freelist.push(p1) });
        // SAFETY: same as above.
        assert!(unsafe { freelist.push(p2) });
        // SAFETY: same as above. Expected to fail (over limit).
        assert!(!unsafe { freelist.push(p3) });

        let _ = freelist.pop();
        let _ = freelist.pop();
        // SAFETY: all pointers were allocated with this layout.
        unsafe {
            System.dealloc(p1, layout);
            System.dealloc(p2, layout);
            System.dealloc(p3, layout);
        }
    }

    #[test]
    fn test_global_alloc_basic() {
        let alloc = ZeroPoolGlobalAlloc::new();
        let layout = Layout::from_size_align(4096, 8).unwrap();

        // SAFETY: valid layout, GlobalAlloc contract.
        let ptr = unsafe { alloc.alloc(layout) };
        assert!(!ptr.is_null());

        // SAFETY: ptr is valid for 4096 bytes.
        unsafe { std::ptr::write_bytes(ptr, 0xAB, 4096) };

        // SAFETY: ptr was allocated with this layout.
        unsafe { alloc.dealloc(ptr, layout) };
        assert_eq!(alloc.classes[0].count.load(Ordering::Relaxed), 1);

        // SAFETY: valid layout, GlobalAlloc contract.
        let ptr2 = unsafe { alloc.alloc(layout) };
        assert_eq!(ptr2, ptr);
        assert_eq!(alloc.classes[0].count.load(Ordering::Relaxed), 0);

        // SAFETY: ptr2 was allocated with this layout.
        unsafe { alloc.dealloc(ptr2, layout) };
    }

    #[test]
    fn test_global_alloc_oversize() {
        let alloc = ZeroPoolGlobalAlloc::new();
        let layout = Layout::from_size_align(128 * 1024 * 1024, 8).unwrap();

        // SAFETY: valid layout, GlobalAlloc contract.
        let ptr = unsafe { alloc.alloc(layout) };
        assert!(!ptr.is_null());

        // SAFETY: ptr was allocated with this layout.
        unsafe { alloc.dealloc(ptr, layout) };
        for fl in &alloc.classes {
            assert_eq!(fl.count.load(Ordering::Relaxed), 0);
        }
    }

    #[test]
    fn test_global_alloc_small() {
        let alloc = ZeroPoolGlobalAlloc::new();
        let layout = Layout::from_size_align(1, 1).unwrap();

        // SAFETY: valid layout, GlobalAlloc contract.
        let ptr = unsafe { alloc.alloc(layout) };
        assert!(!ptr.is_null());
        // SAFETY: ptr was allocated with this layout.
        unsafe { alloc.dealloc(ptr, layout) };
    }

    #[test]
    fn test_global_alloc_concurrent() {
        use std::thread;

        static ALLOC: ZeroPoolGlobalAlloc = ZeroPoolGlobalAlloc::new();

        let handles: Vec<_> = (0..4)
            .map(|_| {
                thread::spawn(|| {
                    let layout = Layout::from_size_align(4096, 8).unwrap();
                    for _ in 0..100 {
                        // SAFETY: valid layout, GlobalAlloc contract.
                        let ptr = unsafe { ALLOC.alloc(layout) };
                        assert!(!ptr.is_null());
                        // SAFETY: ptr is valid for 4096 bytes.
                        unsafe { std::ptr::write_bytes(ptr, 0, 4096) };
                        // SAFETY: ptr was allocated with this layout.
                        unsafe { ALLOC.dealloc(ptr, layout) };
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }
    }
}
