//! Multi-threaded alloc/dealloc profiling target for callgrind (4 threads).

use std::hint::black_box;
use std::thread;
use zeropool::ZeroPool;

fn main() {
    let pool = ZeroPool::new().min_buffer_size(0);

    thread::scope(|s| {
        for _ in 0..4 {
            s.spawn(|| {
                for _ in 0..500_000 {
                    let buf = pool.alloc(black_box(65536));
                    drop(black_box(buf));
                }
            });
        }
    });
}
