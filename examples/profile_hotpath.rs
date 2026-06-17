//! Single-threaded get/put hot-path profiling target for callgrind.

use std::hint::black_box;
use zeropool::BufferPool;

fn main() {
    let pool = BufferPool::builder().min_buffer_size(0).build();
    // Warm up TLS
    let buf = pool.get(4096);
    drop(buf);

    // Hot loop - single thread get/put (reduced for callgrind)
    for _ in 0..1_000_000 {
        let buf = pool.get(black_box(4096));
        drop(black_box(buf));
    }
}
