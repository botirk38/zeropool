//! Pluggable buffer allocation backend.
//!
//! [`ZeroPool`](crate::ZeroPool) delegates raw buffer creation to an [`Allocator`].
//! The default [`HeapAllocator`] uses `Vec::with_capacity` (standard heap).
//! Implement the trait for custom strategies (page-aligned, huge pages, etc.).

/// Controls how the pool creates raw byte buffers.
///
/// # Contract
///
/// - `allocate(capacity)` must return a `Vec<u8>` with `capacity() >= capacity`.
/// - The returned `Vec` must have `len() == 0`.
/// - The `Vec` must be deallocatable by the standard global allocator.
///
/// # Example
///
/// ```
/// use zeropool::{Allocator, ZeroPool};
///
/// struct PrefaultAllocator;
///
/// impl Allocator for PrefaultAllocator {
///     fn allocate(&self, capacity: usize) -> Vec<u8> {
///         let mut buf = Vec::with_capacity(capacity);
///         buf.resize(capacity, 0); // pre-fault pages
///         buf.clear();
///         buf
///     }
/// }
///
/// let pool = ZeroPool::new().allocator(PrefaultAllocator);
/// ```
pub trait Allocator: Send + Sync + 'static {
    /// Allocate a buffer with at least `capacity` bytes.
    fn allocate(&self, capacity: usize) -> Vec<u8>;
}

/// Standard heap allocation via `Vec::with_capacity`.
///
/// This is the default — zero-sized, no overhead.
#[derive(Debug, Clone, Copy, Default)]
pub struct HeapAllocator;

impl Allocator for HeapAllocator {
    #[inline]
    fn allocate(&self, capacity: usize) -> Vec<u8> {
        Vec::with_capacity(capacity)
    }
}

impl std::fmt::Debug for dyn Allocator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("dyn Allocator")
    }
}
