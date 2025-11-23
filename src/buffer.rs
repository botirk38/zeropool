//! RAII wrapper for pooled buffers with automatic return on drop

use std::fmt;
use std::ops::{Deref, DerefMut};

use crate::BufferPool;

/// RAII wrapper for a pooled buffer that automatically returns to the pool on drop.
///
/// This type provides transparent access to the underlying `Vec<u8>` through `Deref`
/// and `DerefMut`, allowing it to be used like a normal `Vec<u8>` while ensuring
/// automatic return to the pool when it goes out of scope.
///
/// # Examples
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
///
/// # Explicit Early Return
///
/// To return a buffer before the end of its scope, use `drop()`:
///
/// ```
/// use zeropool::BufferPool;
///
/// let pool = BufferPool::new();
/// let buffer = pool.get(1024);
/// // ... use buffer ...
/// drop(buffer); // Explicitly return to pool now
/// ```
pub struct PooledBuffer {
    buffer: Option<Vec<u8>>,
    pool: BufferPool,
}

impl PooledBuffer {
    /// Create a new pooled buffer wrapper.
    ///
    /// This is an internal method used by `BufferPool::get()`.
    pub(crate) fn new(buffer: Vec<u8>, pool: BufferPool) -> Self {
        Self { buffer: Some(buffer), pool }
    }

    /// Returns the length of the buffer in bytes.
    #[inline]
    pub fn len(&self) -> usize {
        self.buffer.as_ref().unwrap().len()
    }

    /// Returns the capacity of the buffer in bytes.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.buffer.as_ref().unwrap().capacity()
    }

    /// Returns `true` if the buffer has a length of 0.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.buffer.as_ref().unwrap().is_empty()
    }
}

impl Deref for PooledBuffer {
    type Target = Vec<u8>;

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.buffer.as_ref().expect("PooledBuffer already consumed")
    }
}

impl DerefMut for PooledBuffer {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.buffer.as_mut().expect("PooledBuffer already consumed")
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
        self.buffer.as_ref().unwrap().as_ref()
    }
}

impl AsMut<[u8]> for PooledBuffer {
    #[inline]
    fn as_mut(&mut self) -> &mut [u8] {
        self.buffer.as_mut().unwrap().as_mut()
    }
}

// Note: Send and Sync are automatically implemented for PooledBuffer because:
// - Vec<u8> is Send + Sync
// - BufferPool contains Arc<Vec<Shard>> which is Send + Sync
// - All fields are Send + Sync, so PooledBuffer is automatically Send + Sync

#[cfg(test)]
mod tests {

    use crate::BufferPool;

    #[test]
    fn test_pooled_buffer_deref() {
        let pool = BufferPool::new();
        let mut buffer = pool.get(1024);

        // Test Deref - can read like Vec<u8>
        assert_eq!(buffer.len(), 1024);

        // Test DerefMut - can write like Vec<u8>
        buffer[0] = 42;
        assert_eq!(buffer[0], 42);
    }

    #[test]
    fn test_pooled_buffer_auto_return() {
        let pool = BufferPool::new();

        let cap = {
            let buffer = pool.get(1024);
            buffer.capacity()
            // Buffer should auto-return on drop
        };

        // Buffer should be reusable
        let buffer2 = pool.get(1024);
        assert_eq!(buffer2.capacity(), cap, "Buffer was not reused");
    }

    #[test]
    fn test_pooled_buffer_explicit_drop() {
        let pool = BufferPool::new();

        let cap = {
            let buffer = pool.get(1024);
            let cap = buffer.capacity();
            drop(buffer); // Explicit early return
            cap
        };

        // Buffer should be reusable
        let buffer2 = pool.get(1024);
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
}
