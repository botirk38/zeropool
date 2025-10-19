use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;
use std::sync::Arc;
use std::thread;

fn benchmark_zeropool_single_thread(c: &mut Criterion) {
    let mut group = c.benchmark_group("single_thread");

    for size in [1024, 4096, 16384, 65536, 1024 * 1024] {
        group.throughput(Throughput::Bytes(size as u64));

        group.bench_with_input(BenchmarkId::new("zeropool", size), &size, |b, &size| {
            let pool = zeropool::BufferPool::new();
            b.iter(|| {
                let mut buf = pool.get(size);
                black_box(&mut buf);
                pool.put(buf);
            });
        });

        group.bench_with_input(BenchmarkId::new("no_pool", size), &size, |b, &size| {
            b.iter(|| {
                let mut buf = Vec::<u8>::with_capacity(size);
                black_box(&mut buf);
            });
        });

        group.bench_with_input(BenchmarkId::new("sharded_slab", size), &size, |b, &size| {
            use sharded_slab::Slab;
            let slab: Slab<Vec<u8>> = Slab::new();
            b.iter(|| {
                let key = slab.insert(Vec::with_capacity(size)).unwrap();
                black_box(slab.get(key));
                slab.remove(key);
            });
        });

        group.bench_with_input(BenchmarkId::new("lifeguard", size), &size, |b, &size| {
            use lifeguard::Pool;
            let pool: Pool<Vec<u8>> = Pool::with_size(10);
            b.iter(|| {
                let mut buf = pool.new();
                buf.reserve(size);
                black_box(&mut buf);
            });
        });

        group.bench_with_input(BenchmarkId::new("object_pool", size), &size, |b, &size| {
            use object_pool::Pool;
            let pool: Pool<Vec<u8>> = Pool::new(10, || Vec::with_capacity(size));
            b.iter(|| {
                let mut buf = pool.pull(|| Vec::with_capacity(size));
                black_box(&mut buf);
            });
        });
    }

    group.finish();
}

fn benchmark_zeropool_multi_thread(c: &mut Criterion) {
    let mut group = c.benchmark_group("multi_thread");
    let size = 64 * 1024;
    const ITERATIONS_PER_THREAD: usize = 1000;

    for num_threads in [2, 4, 8] {
        group.throughput(Throughput::Bytes(
            (size * num_threads * ITERATIONS_PER_THREAD) as u64,
        ));

        group.bench_with_input(
            BenchmarkId::new("zeropool", num_threads),
            &num_threads,
            |b, &num_threads| {
                use std::sync::Barrier;
                let pool = Arc::new(zeropool::BufferPool::new());
                let barrier_start = Arc::new(Barrier::new(num_threads + 1));
                let barrier_end = Arc::new(Barrier::new(num_threads + 1));

                // Spawn long-lived threads
                let handles: Vec<_> = (0..num_threads)
                    .map(|_| {
                        let pool = Arc::clone(&pool);
                        let barrier_start = Arc::clone(&barrier_start);
                        let barrier_end = Arc::clone(&barrier_end);

                        thread::spawn(move || {
                            loop {
                                barrier_start.wait();

                                // Perform multiple get/put operations
                                for _ in 0..ITERATIONS_PER_THREAD {
                                    let mut buf = pool.get(size);
                                    black_box(&mut buf);
                                    pool.put(buf);
                                }

                                barrier_end.wait();
                            }
                        })
                    })
                    .collect();

                b.iter(|| {
                    barrier_start.wait(); // Start all threads
                    barrier_end.wait(); // Wait for completion
                });

                // Note: threads are never joined in this benchmark, they'll be cleaned up
                // when the process exits. This is acceptable for benchmarking purposes.
                drop(handles);
            },
        );

        group.bench_with_input(
            BenchmarkId::new("no_pool", num_threads),
            &num_threads,
            |b, &num_threads| {
                use std::sync::Barrier;
                let barrier_start = Arc::new(Barrier::new(num_threads + 1));
                let barrier_end = Arc::new(Barrier::new(num_threads + 1));

                let handles: Vec<_> = (0..num_threads)
                    .map(|_| {
                        let barrier_start = Arc::clone(&barrier_start);
                        let barrier_end = Arc::clone(&barrier_end);

                        thread::spawn(move || {
                            loop {
                                barrier_start.wait();

                                for _ in 0..ITERATIONS_PER_THREAD {
                                    let mut buf = Vec::<u8>::with_capacity(size);
                                    black_box(&mut buf);
                                }

                                barrier_end.wait();
                            }
                        })
                    })
                    .collect();

                b.iter(|| {
                    barrier_start.wait();
                    barrier_end.wait();
                });

                drop(handles);
            },
        );

        group.bench_with_input(
            BenchmarkId::new("sharded_slab", num_threads),
            &num_threads,
            |b, &num_threads| {
                use sharded_slab::Slab;
                use std::sync::Barrier;
                let slab: Arc<Slab<Vec<u8>>> = Arc::new(Slab::new());
                let barrier_start = Arc::new(Barrier::new(num_threads + 1));
                let barrier_end = Arc::new(Barrier::new(num_threads + 1));

                let handles: Vec<_> = (0..num_threads)
                    .map(|_| {
                        let slab = Arc::clone(&slab);
                        let barrier_start = Arc::clone(&barrier_start);
                        let barrier_end = Arc::clone(&barrier_end);

                        thread::spawn(move || {
                            loop {
                                barrier_start.wait();

                                for _ in 0..ITERATIONS_PER_THREAD {
                                    let key = slab.insert(Vec::with_capacity(size)).unwrap();
                                    black_box(slab.get(key));
                                    slab.remove(key);
                                }

                                barrier_end.wait();
                            }
                        })
                    })
                    .collect();

                b.iter(|| {
                    barrier_start.wait();
                    barrier_end.wait();
                });

                drop(handles);
            },
        );

        group.bench_with_input(
            BenchmarkId::new("object_pool", num_threads),
            &num_threads,
            |b, &num_threads| {
                use object_pool::Pool;
                use std::sync::Barrier;
                let pool: Arc<Pool<Vec<u8>>> =
                    Arc::new(Pool::new(num_threads * 2, || Vec::with_capacity(size)));
                let barrier_start = Arc::new(Barrier::new(num_threads + 1));
                let barrier_end = Arc::new(Barrier::new(num_threads + 1));

                let handles: Vec<_> = (0..num_threads)
                    .map(|_| {
                        let pool = Arc::clone(&pool);
                        let barrier_start = Arc::clone(&barrier_start);
                        let barrier_end = Arc::clone(&barrier_end);

                        thread::spawn(move || {
                            loop {
                                barrier_start.wait();

                                for _ in 0..ITERATIONS_PER_THREAD {
                                    let mut buf = pool.pull(|| Vec::with_capacity(size));
                                    black_box(&mut buf);
                                }

                                barrier_end.wait();
                            }
                        })
                    })
                    .collect();

                b.iter(|| {
                    barrier_start.wait();
                    barrier_end.wait();
                });

                drop(handles);
            },
        );
    }

    group.finish();
}

fn benchmark_allocation_patterns(c: &mut Criterion) {
    let mut group = c.benchmark_group("allocation_patterns");
    let size = 64 * 1024;

    group.bench_function("zeropool_varied_sizes", |b| {
        let pool = zeropool::BufferPool::new();
        let sizes = [1024, 4096, 16384, 65536, 256 * 1024];
        b.iter(|| {
            for &size in &sizes {
                let buf = pool.get(size);
                black_box(&buf);
                pool.put(buf);
            }
        });
    });

    group.bench_function("no_pool_varied_sizes", |b| {
        let sizes = [1024, 4096, 16384, 65536, 256 * 1024];
        b.iter(|| {
            for &size in &sizes {
                let buf = Vec::<u8>::with_capacity(size);
                black_box(&buf);
            }
        });
    });

    group.bench_function("lifeguard_varied_sizes", |b| {
        use lifeguard::Pool;
        let pool: Pool<Vec<u8>> = Pool::with_size(10);
        let sizes = [1024, 4096, 16384, 65536, 256 * 1024];
        b.iter(|| {
            for &size in &sizes {
                let mut buf = pool.new();
                buf.reserve(size);
                black_box(&buf);
            }
        });
    });

    group.bench_function("zeropool_reuse", |b| {
        let pool = zeropool::BufferPool::new();
        pool.preallocate(10, size);
        b.iter(|| {
            let mut bufs = vec![];
            for _ in 0..10 {
                bufs.push(pool.get(size));
            }
            for buf in bufs {
                black_box(&buf);
                pool.put(buf);
            }
        });
    });

    group.bench_function("no_pool_reuse", |b| {
        b.iter(|| {
            let mut bufs = vec![];
            for _ in 0..10 {
                bufs.push(Vec::<u8>::with_capacity(size));
            }
            for buf in bufs {
                black_box(&buf);
            }
        });
    });

    group.bench_function("lifeguard_reuse", |b| {
        use lifeguard::Pool;
        let pool: Pool<Vec<u8>> = Pool::with_size(10);
        b.iter(|| {
            let mut bufs = vec![];
            for _ in 0..10 {
                let mut buf = pool.new();
                buf.reserve(size);
                bufs.push(buf);
            }
            for buf in bufs {
                black_box(&buf);
            }
        });
    });

    group.finish();
}

fn benchmark_bytes_pool(c: &mut Criterion) {
    let mut group = c.benchmark_group("bytes_comparison");

    for size in [1024, 64 * 1024, 1024 * 1024] {
        group.throughput(Throughput::Bytes(size as u64));

        group.bench_with_input(BenchmarkId::new("zeropool", size), &size, |b, &size| {
            let pool = zeropool::BufferPool::new();
            b.iter(|| {
                let mut buf = pool.get(size);
                black_box(&mut buf);
                pool.put(buf);
            });
        });

        group.bench_with_input(BenchmarkId::new("bytes", size), &size, |b, &size| {
            b.iter(|| {
                let mut buf = bytes::BytesMut::with_capacity(size);
                black_box(&mut buf);
            });
        });
    }

    group.finish();
}

fn benchmark_pinned_memory(c: &mut Criterion) {
    let mut group = c.benchmark_group("pinned_memory");
    let size = 1024 * 1024; // 1MB buffers

    group.bench_function("unpinned", |b| {
        let pool = zeropool::BufferPool::builder().pinned_memory(false).build();
        b.iter(|| {
            let mut buf = pool.get(size);
            black_box(&mut buf);
            pool.put(buf);
        });
    });

    group.bench_function("pinned", |b| {
        let pool = zeropool::BufferPool::builder().pinned_memory(true).build();
        b.iter(|| {
            let mut buf = pool.get(size);
            black_box(&mut buf);
            pool.put(buf);
        });
    });

    group.bench_function("pinned_preallocated", |b| {
        let pool = zeropool::BufferPool::builder().pinned_memory(true).build();
        pool.preallocate(10, size);
        b.iter(|| {
            let mut buf = pool.get(size);
            black_box(&mut buf);
            pool.put(buf);
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    benchmark_zeropool_single_thread,
    benchmark_zeropool_multi_thread,
    benchmark_allocation_patterns,
    benchmark_bytes_pool,
    benchmark_pinned_memory
);
criterion_main!(benches);
