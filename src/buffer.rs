//! RAII wrapper for pooled buffers with automatic return on drop.

use std::fmt;
use std::io;
use std::ops::{Deref, DerefMut};

use crate::BufferPool;

/// RAII wrapper for a pooled buffer that automatically returns to the pool on drop.
///
/// Provides transparent access to the underlying byte slice through `Deref`
/// and `DerefMut`. Unlike the old API, the `Deref` target is `[u8]` (not
/// `Vec<u8>`), preventing callers from accidentally resizing the buffer and
/// violating pool invariants.
///
/// Use [`capacity()`](Self::capacity), [`len()`](Self::len), and
/// [`is_empty()`](Self::is_empty) to query buffer metadata.
/// Use [`into_inner()`](Self::into_inner) or [`into_vec()`](Self::into_vec)
/// to extract the underlying `Vec<u8>` without returning it to the pool.
///
/// # Example
///
/// ```
/// use zeropool::BufferPool;
///
/// let pool = BufferPool::new();
/// {
///     let mut buffer = pool.get(1024);
///     buffer[0] = 42;
///     // Buffer automatically returned to pool here
/// }
/// ```
pub struct PooledBuffer {
    buffer: Option<Vec<u8>>,
    pool: BufferPool,
}

// ── Private infallible accessors ───────────────────────────────────────
//
// `buffer` is `Some` for the entire public lifetime of `PooledBuffer`.
// `into_inner()` consumes `self`; `Drop` is the only path to `None`.
impl PooledBuffer {
    #[inline(always)]
    fn vec(&self) -> &Vec<u8> {
        // SAFETY: buffer is always Some during public lifetime.
        unsafe { self.buffer.as_ref().unwrap_unchecked() }
    }

    #[inline(always)]
    fn vec_mut(&mut self) -> &mut Vec<u8> {
        // SAFETY: buffer is always Some during public lifetime.
        unsafe { self.buffer.as_mut().unwrap_unchecked() }
    }
}

impl PooledBuffer {
    /// Create a new pooled buffer wrapper (internal).
    pub(crate) fn new(buffer: Vec<u8>, pool: BufferPool) -> Self {
        Self { buffer: Some(buffer), pool }
    }

    /// Returns the length of the buffer in bytes.
    #[inline]
    pub fn len(&self) -> usize {
        self.vec().len()
    }

    /// Returns the capacity of the buffer in bytes.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.vec().capacity()
    }

    /// Returns `true` if the buffer has a length of 0.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.vec().is_empty()
    }

    /// Returns a pointer to the start of the buffer.
    #[inline]
    pub fn as_ptr(&self) -> *const u8 {
        self.vec().as_ptr()
    }

    /// Returns a mutable pointer to the start of the buffer.
    #[inline]
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.vec_mut().as_mut_ptr()
    }

    /// Consumes the pooled buffer and returns the underlying `Vec<u8>`.
    ///
    /// The buffer is **not** returned to the pool.
    ///
    /// # Example
    ///
    /// ```
    /// use zeropool::BufferPool;
    ///
    /// let pool = BufferPool::new();
    /// let buffer = pool.get(1024);
    /// let vec: Vec<u8> = buffer.into_inner();
    /// ```
    #[must_use]
    pub fn into_inner(mut self) -> Vec<u8> {
        self.buffer.take().expect("PooledBuffer already consumed")
    }

    /// Alias for [`into_inner()`](Self::into_inner).
    #[inline]
    #[must_use]
    pub fn into_vec(self) -> Vec<u8> {
        self.into_inner()
    }
}

// ── Deref to [u8], not Vec<u8> ─────────────────────────────────────────
//
// Exposing `[u8]` prevents callers from calling Vec methods like `push`,
// `resize`, or `truncate` that would violate pool sizing invariants.

impl Deref for PooledBuffer {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &[u8] {
        self.vec()
    }
}

impl DerefMut for PooledBuffer {
    #[inline]
    fn deref_mut(&mut self) -> &mut [u8] {
        self.vec_mut()
    }
}

impl Drop for PooledBuffer {
    fn drop(&mut self) {
        if let Some(buffer) = self.buffer.take() {
            self.pool.put(buffer);
        }
    }
}

impl fmt::Debug for PooledBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PooledBuffer")
            .field("len", &self.len())
            .field("capacity", &self.capacity())
            .finish()
    }
}

impl AsRef<[u8]> for PooledBuffer {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        self.vec()
    }
}

impl AsMut<[u8]> for PooledBuffer {
    #[inline]
    fn as_mut(&mut self) -> &mut [u8] {
        self.vec_mut()
    }
}

impl From<PooledBuffer> for Vec<u8> {
    fn from(buf: PooledBuffer) -> Self {
        buf.into_inner()
    }
}

impl io::Write for PooledBuffer {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        io::Write::write(self.vec_mut(), buf)
    }

    #[inline]
    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        io::Write::write_all(self.vec_mut(), buf)
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

// ── tokio-uring support ────────────────────────────────────────────────

#[cfg(all(target_os = "linux", feature = "tokio-uring"))]
mod tokio_uring_support {
    use super::PooledBuffer;
    use tokio_uring::buf::{IoBuf, IoBufMut};

    // SAFETY: PooledBuffer wraps a Vec<u8> whose backing storage is stable
    // for the entire lifetime of the struct. The Option is always Some
    // during public use; only Drop and into_inner set it to None.
    unsafe impl IoBuf for PooledBuffer {
        fn stable_ptr(&self) -> *const u8 {
            self.vec().as_ptr()
        }

        fn bytes_init(&self) -> usize {
            self.vec().len()
        }

        fn bytes_total(&self) -> usize {
            self.vec().capacity()
        }
    }

    // SAFETY: Same stability guarantee as IoBuf. set_init is only called
    // by tokio-uring after it has written exactly `pos` bytes.
    unsafe impl IoBufMut for PooledBuffer {
        fn stable_mut_ptr(&mut self) -> *mut u8 {
            self.vec_mut().as_mut_ptr()
        }

        unsafe fn set_init(&mut self, pos: usize) {
            // SAFETY: tokio-uring guarantees pos bytes are initialized.
            unsafe { self.vec_mut().set_len(pos) };
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use crate::BufferPool;

    #[test]
    fn test_pooled_buffer_deref() {
        let pool = BufferPool::new();
        let mut buffer = pool.get(1024);

        assert_eq!(buffer.len(), 1024);
        buffer[0] = 42;
        assert_eq!(buffer[0], 42);
    }

    #[test]
    fn test_pooled_buffer_auto_return() {
        let pool = BufferPool::builder().min_buffer_size(0).build();
        let cap = {
            let buffer = pool.get(4096);
            buffer.capacity()
        };
        let buffer2 = pool.get(4096);
        assert_eq!(buffer2.capacity(), cap, "Buffer was not reused");
    }

    #[test]
    fn test_pooled_buffer_explicit_drop() {
        let pool = BufferPool::builder().min_buffer_size(0).build();
        let cap = {
            let buffer = pool.get(4096);
            let cap = buffer.capacity();
            drop(buffer);
            cap
        };
        let buffer2 = pool.get(4096);
        assert_eq!(buffer2.capacity(), cap, "Buffer was not reused");
    }

    #[test]
    fn test_pooled_buffer_as_ref() {
        let pool = BufferPool::new();
        let mut buffer = pool.get(10);
        buffer[0] = 42;
        let slice: &[u8] = buffer.as_ref();
        assert_eq!(slice[0], 42);
    }

    #[test]
    fn test_pooled_buffer_as_mut() {
        let pool = BufferPool::new();
        let mut buffer = pool.get(10);
        let slice: &mut [u8] = buffer.as_mut();
        slice[0] = 42;
        assert_eq!(buffer[0], 42);
    }

    #[test]
    fn test_pooled_buffer_debug() {
        let pool = BufferPool::new();
        let buffer = pool.get(1024);
        let debug_str = format!("{buffer:?}");
        assert!(debug_str.contains("PooledBuffer"));
        assert!(debug_str.contains("len"));
        assert!(debug_str.contains("capacity"));
    }

    #[test]
    fn test_into_inner() {
        let pool = BufferPool::new();
        let mut buffer = pool.get(10);
        buffer[0] = 42;
        let vec = buffer.into_inner();
        assert_eq!(vec.len(), 10);
        assert_eq!(vec[0], 42);
    }

    #[test]
    fn test_into_vec() {
        let pool = BufferPool::new();
        let mut buffer = pool.get(5);
        buffer[0] = 123;
        let vec = buffer.into_vec();
        assert_eq!(vec.len(), 5);
        assert_eq!(vec[0], 123);
    }

    #[test]
    fn test_into_inner_consumes_buffer() {
        let pool = BufferPool::new();
        let buffer = pool.get(1024);
        let vec = buffer.into_inner();
        assert_eq!(vec.len(), 1024);
    }

    #[test]
    fn test_from_pooled_buffer_for_vec() {
        let pool = BufferPool::new();
        let mut buffer = pool.get(10);
        buffer[0] = 99;
        let vec: Vec<u8> = buffer.into();
        assert_eq!(vec[0], 99);
    }

    #[test]
    fn test_io_write() {
        use std::io::Write;
        let pool = BufferPool::new();
        let mut buffer = pool.get(0);
        buffer.write_all(b"hello").unwrap();
        assert_eq!(&*buffer, b"hello");
    }

    #[cfg(all(target_os = "linux", feature = "tokio-uring"))]
    #[test]
    fn test_tokio_uring_traits() {
        use tokio_uring::buf::{IoBuf, IoBufMut};

        let pool = BufferPool::new();
        let mut buffer = pool.get(1024);
        assert_eq!(buffer.bytes_total(), buffer.capacity());
        let ptr = buffer.stable_mut_ptr();
        assert!(!ptr.is_null());
    }
}
