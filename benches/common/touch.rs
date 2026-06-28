//! Page-touch helpers. All "realistic" benchmarks touch every page
//! of the buffer to model real workloads (networking, file I/O, etc).

use std::hint::black_box;
use std::mem::MaybeUninit;

/// Write one byte per 4 KiB page to force page faults on a fresh buffer.
#[inline]
pub fn touch_pages(buf: &mut [u8]) {
    let step = 4096;
    let mut i = 0;
    while i < buf.len() {
        buf[i] = 0xAB;
        i += step;
    }
    black_box(buf);
}

/// Write every byte. This models full-overwrite call sites that can safely use
/// `alloc_uninit()` or `Vec::with_capacity()` before initialization.
#[inline]
pub fn write_all(buf: &mut [u8]) {
    buf.fill(0xAB);
    black_box(buf);
}

/// Initialize every byte in an uninitialized buffer.
#[inline]
pub fn write_all_uninit(buf: &mut [MaybeUninit<u8>]) {
    for byte in &mut *buf {
        byte.write(0xAB);
    }
    black_box(buf);
}
