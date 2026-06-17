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
pub struct Buf {
    buffer: Option<Vec<u8>>,
    pool: ZeroPool,
    /// Pre-computed class index from alloc(), avoids recomputation in dealloc().
    /// `u8::MAX` means oversize (not pooled).
    class_idx: u8,
}

// ── Private infallible accessors ───────────────────────────────────────
//
// `buffer` is `Some` for the entire public lifetime of `Buf`.
// `into_inner()` consumes `self`; `Drop` is the only path to `None`.
impl Buf {
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

impl Buf {
    pub(crate) fn new(buffer: Vec<u8>, pool: ZeroPool, class_idx: u8) -> Self {
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

impl Deref for Buf {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &[u8] {
        self.vec()
    }
}

impl DerefMut for Buf {
    #[inline]
    fn deref_mut(&mut self) -> &mut [u8] {
        self.vec_mut()
    }
}

impl Drop for Buf {
    #[inline(always)]
    fn drop(&mut self) {
        if let Some(buffer) = self.buffer.take() {
            self.pool.dealloc(buffer, self.class_idx);
        }
    }
}

impl fmt::Debug for Buf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Buf")
            .field("len", &self.len())
            .field("capacity", &self.capacity())
            .finish()
    }
}

impl AsRef<[u8]> for Buf {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        self.vec()
    }
}

impl AsMut<[u8]> for Buf {
    #[inline]
    fn as_mut(&mut self) -> &mut [u8] {
        self.vec_mut()
    }
}

impl From<Buf> for Vec<u8> {
    fn from(buf: Buf) -> Self {
        buf.into_inner()
    }
}

impl io::Write for Buf {
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
    use super::Buf;
    use tokio_uring::buf::{IoBuf, IoBufMut};

    // SAFETY: Buf wraps a Vec<u8> whose backing storage is stable
    // for the entire lifetime of the struct. The Option is always Some
    // during public use; only Drop and into_inner set it to None.
    unsafe impl IoBuf for Buf {
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
    unsafe impl IoBufMut for Buf {
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

    #[cfg(all(target_os = "linux", feature = "tokio-uring"))]
    #[test]
    fn test_tokio_uring_traits() {
        use tokio_uring::buf::{IoBuf, IoBufMut};

        let pool = ZeroPool::new();
        let mut buf = pool.alloc(1024);
        assert_eq!(buf.bytes_total(), buf.capacity());
        let ptr = buf.stable_mut_ptr();
        assert!(!ptr.is_null());
    }
}
