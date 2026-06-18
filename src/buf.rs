//! RAII buffer with automatic deallocation back to the pool.

use std::fmt;
use std::io;
use std::ops::{Deref, DerefMut};

use crate::ZeroPool;

/// An allocated byte buffer that deallocates back to the pool on drop.
///
/// Provides transparent access to the underlying byte slice through `Deref`
/// and `DerefMut`. The `Deref` target is `[u8]` (not `Vec<u8>`), preventing
/// callers from accidentally resizing the buffer and violating pool invariants.
///
/// Use [`into_inner()`](Self::into_inner) or [`into_vec()`](Self::into_vec)
/// to extract the underlying `Vec<u8>` without returning it to the pool.
///
/// # Example
///
/// ```
/// use zeropool::ZeroPool;
///
/// let pool = ZeroPool::new();
/// {
///     let mut buf = pool.alloc(1024);
///     buf[0] = 42;
///     // automatically deallocated back to pool here
/// }
/// ```
pub struct Buf<'a> {
    buffer: Option<Vec<u8>>,
    pool: &'a ZeroPool,
    class_idx: u8,
}

// ── Private infallible accessors ───────────────────────────────────────
//
// `buffer` is `Some` for the entire public lifetime of `Buf`.
// `into_inner()` consumes `self`; `Drop` is the only path to `None`.
impl Buf<'_> {
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

impl<'a> Buf<'a> {
    pub(crate) fn new(buffer: Vec<u8>, pool: &'a ZeroPool, class_idx: u8) -> Self {
        Self { buffer: Some(buffer), pool, class_idx }
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

    /// Consumes the buffer and returns the underlying `Vec<u8>`.
    ///
    /// The buffer is **not** returned to the pool.
    ///
    /// # Example
    ///
    /// ```
    /// use zeropool::ZeroPool;
    ///
    /// let pool = ZeroPool::new();
    /// let buf = pool.alloc(1024);
    /// let vec: Vec<u8> = buf.into_inner();
    /// ```
    #[must_use]
    pub fn into_inner(mut self) -> Vec<u8> {
        self.buffer.take().expect("Buf already consumed")
    }

    /// Alias for [`into_inner()`](Self::into_inner).
    #[inline]
    #[must_use]
    pub fn into_vec(self) -> Vec<u8> {
        self.into_inner()
    }
}

// ── Deref to [u8], not Vec<u8> ─────────────────────────────────────────

impl Deref for Buf<'_> {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &[u8] {
        self.vec()
    }
}

impl DerefMut for Buf<'_> {
    #[inline]
    fn deref_mut(&mut self) -> &mut [u8] {
        self.vec_mut()
    }
}

impl Drop for Buf<'_> {
    #[inline(always)]
    fn drop(&mut self) {
        if let Some(buffer) = self.buffer.take() {
            self.pool.dealloc(buffer, self.class_idx);
        }
    }
}

impl fmt::Debug for Buf<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Buf")
            .field("len", &self.len())
            .field("capacity", &self.capacity())
            .finish()
    }
}

impl AsRef<[u8]> for Buf<'_> {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        self.vec()
    }
}

impl AsMut<[u8]> for Buf<'_> {
    #[inline]
    fn as_mut(&mut self) -> &mut [u8] {
        self.vec_mut()
    }
}

impl From<Buf<'_>> for Vec<u8> {
    fn from(buf: Buf<'_>) -> Self {
        buf.into_inner()
    }
}

impl io::Write for Buf<'_> {
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

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use crate::ZeroPool;

    #[test]
    fn test_buf_deref() {
        let pool = ZeroPool::new();
        let mut buf = pool.alloc(1024);

        assert_eq!(buf.len(), 1024);
        buf[0] = 42;
        assert_eq!(buf[0], 42);
    }

    #[test]
    fn test_buf_auto_return() {
        let pool = ZeroPool::new().min_buffer_size(0);
        let cap = {
            let buf = pool.alloc(4096);
            buf.capacity()
        };
        let buf2 = pool.alloc(4096);
        assert_eq!(buf2.capacity(), cap, "Buffer was not reused");
    }

    #[test]
    fn test_buf_explicit_drop() {
        let pool = ZeroPool::new().min_buffer_size(0);
        let cap = {
            let buf = pool.alloc(4096);
            let cap = buf.capacity();
            drop(buf);
            cap
        };
        let buf2 = pool.alloc(4096);
        assert_eq!(buf2.capacity(), cap, "Buffer was not reused");
    }

    #[test]
    fn test_buf_as_ref() {
        let pool = ZeroPool::new();
        let mut buf = pool.alloc(10);
        buf[0] = 42;
        let slice: &[u8] = buf.as_ref();
        assert_eq!(slice[0], 42);
    }

    #[test]
    fn test_buf_as_mut() {
        let pool = ZeroPool::new();
        let mut buf = pool.alloc(10);
        let slice: &mut [u8] = buf.as_mut();
        slice[0] = 42;
        assert_eq!(buf[0], 42);
    }

    #[test]
    fn test_buf_debug() {
        let pool = ZeroPool::new();
        let buf = pool.alloc(1024);
        let debug_str = format!("{buf:?}");
        assert!(debug_str.contains("Buf"));
        assert!(debug_str.contains("len"));
        assert!(debug_str.contains("capacity"));
    }

    #[test]
    fn test_into_inner() {
        let pool = ZeroPool::new();
        let mut buf = pool.alloc(10);
        buf[0] = 42;
        let vec = buf.into_inner();
        assert_eq!(vec.len(), 10);
        assert_eq!(vec[0], 42);
    }

    #[test]
    fn test_into_vec() {
        let pool = ZeroPool::new();
        let mut buf = pool.alloc(5);
        buf[0] = 123;
        let vec = buf.into_vec();
        assert_eq!(vec.len(), 5);
        assert_eq!(vec[0], 123);
    }

    #[test]
    fn test_into_inner_consumes() {
        let pool = ZeroPool::new();
        let buf = pool.alloc(1024);
        let vec = buf.into_inner();
        assert_eq!(vec.len(), 1024);
    }

    #[test]
    fn test_from_buf_for_vec() {
        let pool = ZeroPool::new();
        let mut buf = pool.alloc(10);
        buf[0] = 99;
        let vec: Vec<u8> = buf.into();
        assert_eq!(vec[0], 99);
    }

    #[test]
    fn test_io_write() {
        use std::io::Write;
        let pool = ZeroPool::new();
        let mut buf = pool.alloc(0);
        buf.write_all(b"hello").unwrap();
        assert_eq!(&*buf, b"hello");
    }
}
