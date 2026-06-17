//! Multi-threaded alloc/dealloc profiling target for callgrind (4 threads).

use std::hint::black_box;
use std::thread;
use zeropool::ZeroPool;

fn main() {
    let pool = ZeroPool::new().min_buffer_size(0);

    let handles: Vec<_> = (0..4)
        .map(|_| {
            let p = pool.clone();
            thread::spawn(move || {
                for _ in 0..500_000 {
                    let buf = p.alloc(black_box(65536));
                    drop(black_box(buf));
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }
}
