//! Multi-threaded get/put profiling target for callgrind (4 threads).

use std::hint::black_box;
use std::thread;
use zeropool::BufferPool;

fn main() {
    let pool = BufferPool::new().min_buffer_size(0);

    let handles: Vec<_> = (0..4)
        .map(|_| {
            let p = pool.clone();
            thread::spawn(move || {
                for _ in 0..500_000 {
                    let buf = p.get(black_box(65536));
                    drop(black_box(buf));
                }
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }
}
