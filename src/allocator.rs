//! Pluggable buffer allocation backend.
//!
//! [`ZeroPool`](crate::ZeroPool) delegates raw buffer creation to an [`Allocator`].
//! The default [`HeapAllocator`] returns zero-initialized heap buffers.
//! Implement the trait for custom strategies (page-aligned, huge pages, etc.).

/// Controls how the pool creates raw byte buffers.
///
/// # Contract
///
/// - `allocate(capacity)` must return a `Vec<u8>` with `len() >= capacity`.
/// - The first `capacity` bytes must be initialized to zero.
/// - The returned `Vec` must have `capacity() >= len()`.
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
///         vec![0; capacity]
///     }
/// }
///
/// let pool = ZeroPool::new().allocator(PrefaultAllocator);
/// ```
pub trait Allocator: Send + Sync + 'static {
    /// Allocate a zero-initialized buffer with at least `capacity` bytes.
    fn allocate(&self, capacity: usize) -> Vec<u8>;
}

/// Standard heap allocation via `vec![0; capacity]`.
///
/// This is the default safe allocator used by [`ZeroPool`](crate::ZeroPool).
#[derive(Debug, Clone, Copy, Default)]
pub struct HeapAllocator;

impl Allocator for HeapAllocator {
    #[inline]
    fn allocate(&self, capacity: usize) -> Vec<u8> {
        vec![0; capacity]
    }
}

impl std::fmt::Debug for dyn Allocator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("dyn Allocator")
    }
}
