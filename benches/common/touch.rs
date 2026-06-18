//! Page-touch helpers. All "realistic" benchmarks touch every page
//! of the buffer to model real workloads (networking, file I/O, etc).

use std::hint::black_box;

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
