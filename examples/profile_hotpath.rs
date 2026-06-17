//! Single-threaded alloc/dealloc hot-path profiling target for callgrind.

use std::hint::black_box;
use zeropool::ZeroPool;

fn main() {
    let pool = ZeroPool::new().min_buffer_size(0);
    let buf = pool.alloc(4096);
    drop(buf);

    for _ in 0..1_000_000 {
        let buf = pool.alloc(black_box(4096));
        drop(black_box(buf));
    }
}
