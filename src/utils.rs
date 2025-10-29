/// Pin buffer memory to RAM using mlock
///
/// Attempts to lock the buffer in physical RAM to prevent swapping.
/// This is useful for security-sensitive data or latency-critical operations.
///
/// # Best-effort
///
/// Pinning may fail due to:
/// - Insufficient permissions (needs CAP_IPC_LOCK on Linux)
/// - Resource limits (RLIMIT_MEMLOCK)
/// - Platform limitations
///
/// Failures are silently ignored as this is an optimization, not a requirement.
#[inline]
pub(crate) fn pin_buffer(buffer: &[u8]) {
    if buffer.is_empty() {
        return;
    }

    let ptr = buffer.as_ptr();
    let len = buffer.len();

    let _ = region::lock(ptr, len);
}
