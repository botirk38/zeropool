//! Consolidated benchmark suite for zeropool.
//!
//! Run all:           `cargo bench`
//! Run targeted:      `cargo bench -- single_thread`
//!                    `cargo bench -- multi_thread`
//!                    `cargo bench -- allocation_patterns`
//!                    `cargo bench -- cache_behavior`
//!                    `cargo bench -- memory_features`
#![allow(missing_docs)]

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;
use std::sync::Arc;
use std::thread;
use zeropool::BufferPool;

// ── Single-thread comparisons ──────────────────────────────────────────

fn single_thread(c: &mut Criterion) {
    let mut group = c.benchmark_group("single_thread");

    for size in [1024, 4096, 16384, 65536, 1024 * 1024] {
        group.throughput(Throughput::Bytes(size as u64));

        group.bench_with_input(BenchmarkId::new("zeropool", size), &size, |b, &size| {
            let pool = BufferPool::new();
            b.iter(|| {
                let mut buf = pool.get(size);
                black_box(&mut buf);
                drop(buf);
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

        group.bench_with_input(BenchmarkId::new("bytes", size), &size, |b, &size| {
            b.iter(|| {
                let mut buf = bytes::BytesMut::with_capacity(size);
                black_box(&mut buf);
            });
        });
    }

    group.finish();
}

// ── Multi-thread scaling ───────────────────────────────────────────────

fn multi_thread(c: &mut Criterion) {
    const ITERATIONS_PER_THREAD: usize = 1000;
    let mut group = c.benchmark_group("multi_thread");
    let size = 64 * 1024;

    for num_threads in [2, 4, 8] {
        group.throughput(Throughput::Bytes((size * num_threads * ITERATIONS_PER_THREAD) as u64));

        group.bench_with_input(
            BenchmarkId::new("zeropool", num_threads),
            &num_threads,
            |b, &num_threads| {
                use std::sync::Barrier;
                let pool = Arc::new(BufferPool::new());
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
                                    let mut buf = pool.get(size);
                                    black_box(&mut buf);
                                    drop(buf);
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

// ── Allocation patterns ────────────────────────────────────────────────

fn allocation_patterns(c: &mut Criterion) {
    let size = 64 * 1024;

    // Varied sizes
    {
        let mut group = c.benchmark_group("allocation_patterns/varied");

        group.bench_function("zeropool", |b| {
            let pool = BufferPool::new();
            let sizes = [1024, 4096, 16384, 65536, 256 * 1024];
            b.iter(|| {
                for &s in &sizes {
                    let buf = pool.get(s);
                    black_box(&buf);
                    drop(buf);
                }
            });
        });

        group.bench_function("no_pool", |b| {
            let sizes = [1024, 4096, 16384, 65536, 256 * 1024];
            b.iter(|| {
                for &s in &sizes {
                    let buf = Vec::<u8>::with_capacity(s);
                    black_box(&buf);
                }
            });
        });

        group.finish();
    }

    // Reuse patterns
    {
        let mut group = c.benchmark_group("allocation_patterns/reuse");

        group.bench_function("zeropool", |b| {
            let pool = BufferPool::new();
            pool.preallocate(10, size);
            b.iter(|| {
                let mut bufs = vec![];
                for _ in 0..10 {
                    bufs.push(pool.get(size));
                }
                for buf in bufs {
                    black_box(&buf);
                    drop(buf);
                }
            });
        });

        group.bench_function("no_pool", |b| {
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

        group.finish();
    }

    // Burst allocations
    {
        let mut group = c.benchmark_group("allocation_patterns/burst");

        group.bench_function("zeropool_100", |b| {
            let pool = BufferPool::new();
            b.iter(|| {
                let mut bufs = vec![];
                for _ in 0..100 {
                    bufs.push(pool.get(size));
                }
                for buf in bufs {
                    drop(buf);
                }
            });
        });

        group.bench_function("no_pool_100", |b| {
            b.iter(|| {
                let mut bufs = vec![];
                for _ in 0..100 {
                    bufs.push(Vec::<u8>::with_capacity(size));
                }
                for buf in bufs {
                    black_box(&buf);
                }
            });
        });

        group.finish();
    }

    // Zipf distribution
    {
        let mut group = c.benchmark_group("allocation_patterns/zipf");
        let sizes = [(1024, 50), (4096, 25), (16384, 15), (65536, 7), (262_144, 3)];

        group.bench_function("zeropool", |b| {
            let pool = BufferPool::new();
            b.iter(|| {
                for &(s, count) in &sizes {
                    for _ in 0..count {
                        let buf = pool.get(s);
                        black_box(&buf);
                        drop(buf);
                    }
                }
            });
        });

        group.bench_function("no_pool", |b| {
            b.iter(|| {
                for &(s, count) in &sizes {
                    for _ in 0..count {
                        let buf = Vec::<u8>::with_capacity(s);
                        black_box(&buf);
                    }
                }
            });
        });

        group.finish();
    }
}

// ── Cache behavior ─────────────────────────────────────────────────────

fn cache_behavior(c: &mut Criterion) {
    // Ping-pong
    {
        let mut group = c.benchmark_group("cache_behavior/ping_pong");
        group.bench_function("lifo", |b| {
            let pool = BufferPool::builder().tls_cache_size(10).build();
            pool.preallocate(20, 1024 * 1024);
            b.iter(|| {
                for _ in 0..100 {
                    let buf1 = pool.get(1024);
                    let buf2 = pool.get(1024 * 1024);
                    black_box(&buf1);
                    black_box(&buf2);
                    drop(buf1);
                    drop(buf2);
                }
            });
        });
        group.finish();
    }

    // Hot/cold buffers
    {
        let mut group = c.benchmark_group("cache_behavior/hot_cold");
        group.bench_function("lifo", |b| {
            let pool = BufferPool::builder().tls_cache_size(10).build();
            pool.preallocate(10, 64 * 1024);
            b.iter(|| {
                let mut bufs = vec![];
                for _ in 0..3 {
                    bufs.push(pool.get(64 * 1024));
                }
                for _ in 0..1000 {
                    for buf in &bufs {
                        black_box(buf);
                    }
                }
                for buf in bufs {
                    drop(buf);
                }
            });
        });
        group.finish();
    }

    // Multi-size workload
    {
        let mut group = c.benchmark_group("cache_behavior/multi_size");
        group.bench_function("lifo", |b| {
            let pool = BufferPool::builder().tls_cache_size(15).build();
            let sizes = [4 * 1024, 64 * 1024, 1024 * 1024];
            b.iter(|| {
                for _ in 0..100 {
                    for &s in &sizes {
                        let buf = pool.get(s);
                        black_box(&buf);
                        drop(buf);
                    }
                }
            });
        });
        group.finish();
    }

    // TLS cache effectiveness
    {
        let mut group = c.benchmark_group("cache_behavior/tls_cache");
        for tls_size in [2, 4, 8] {
            group.bench_with_input(
                BenchmarkId::new("zeropool", tls_size),
                &tls_size,
                |b, &tls_size| {
                    let pool = BufferPool::builder().tls_cache_size(15).min_buffer_size(0).build();
                    b.iter(|| {
                        let mut bufs = vec![];
                        for _ in 0..tls_size {
                            bufs.push(pool.get(64 * 1024));
                        }
                        for _ in 0..10 {
                            for buf in &bufs {
                                black_box(buf);
                            }
                        }
                        for buf in bufs {
                            drop(buf);
                        }
                    });
                },
            );
        }
        group.finish();
    }

    // Eviction pressure
    {
        let mut group = c.benchmark_group("cache_behavior/eviction_pressure");
        group.sample_size(50);
        group.bench_function("lifo", |b| {
            let pool = BufferPool::builder()
                .max_buffers_per_class(100)
                .tls_cache_size(8)
                .min_buffer_size(4096)
                .build();
            b.iter(|| {
                let mut bufs = vec![];
                for _ in 0..10 {
                    bufs.push(pool.get(64 * 1024));
                }
                for buf in bufs {
                    drop(buf);
                }
            });
        });
        group.finish();
    }

    // Atomic counter contention
    {
        let mut group = c.benchmark_group("cache_behavior/atomic_contention");
        for num_threads in [1, 4, 8, 16] {
            group.bench_with_input(
                BenchmarkId::new("threads", num_threads),
                &num_threads,
                |b, &num_threads| {
                    let pool = BufferPool::builder().tls_cache_size(1).min_buffer_size(0).build();
                    b.iter(|| {
                        let pool = &pool;
                        thread::scope(|s| {
                            let mut handles = vec![];
                            for _ in 0..num_threads {
                                handles.push(s.spawn(|| {
                                    for _ in 0..1000 {
                                        let buf = pool.get(1024);
                                        black_box(&buf);
                                        drop(buf);
                                    }
                                }));
                            }
                            for handle in handles {
                                handle.join().unwrap();
                            }
                        });
                    });
                },
            );
        }
        group.finish();
    }
}

// ── Memory features ────────────────────────────────────────────────────

fn memory_features(c: &mut Criterion) {
    // Pinned memory
    {
        let mut group = c.benchmark_group("memory_features/pinned");
        let size = 1024 * 1024;

        group.bench_function("unpinned", |b| {
            let pool = BufferPool::builder().pinned_memory(false).build();
            b.iter(|| {
                let mut buf = pool.get(size);
                black_box(&mut buf);
                drop(buf);
            });
        });

        group.bench_function("pinned", |b| {
            let pool = BufferPool::builder().pinned_memory(true).build();
            b.iter(|| {
                let mut buf = pool.get(size);
                black_box(&mut buf);
                drop(buf);
            });
        });

        group.bench_function("pinned_preallocated", |b| {
            let pool = BufferPool::builder().pinned_memory(true).build();
            pool.preallocate(10, size);
            b.iter(|| {
                let mut buf = pool.get(size);
                black_box(&mut buf);
                drop(buf);
            });
        });

        group.finish();
    }

    // Preallocation effectiveness
    {
        let mut group = c.benchmark_group("memory_features/preallocation");
        for prealloc_count in [0, 10, 50, 100] {
            group.bench_with_input(
                BenchmarkId::new("zeropool", prealloc_count),
                &prealloc_count,
                |b, &prealloc_count| {
                    let pool = BufferPool::new();
                    if prealloc_count > 0 {
                        pool.preallocate(prealloc_count, 64 * 1024);
                    }
                    b.iter(|| {
                        let mut bufs = vec![];
                        for _ in 0..10 {
                            bufs.push(pool.get(64 * 1024));
                        }
                        for buf in bufs {
                            black_box(&buf);
                            drop(buf);
                        }
                    });
                },
            );
        }
        group.finish();
    }

    // Memory pressure
    {
        let mut group = c.benchmark_group("memory_features/pressure");
        for max_buffers in [5, 10, 20] {
            group.bench_with_input(
                BenchmarkId::new("zeropool", max_buffers),
                &max_buffers,
                |b, &max_buffers| {
                    let pool = BufferPool::builder().max_buffers_per_class(max_buffers).build();
                    b.iter(|| {
                        let mut bufs = vec![];
                        for _ in 0..(max_buffers * 2) {
                            bufs.push(pool.get(64 * 1024));
                        }
                        for buf in bufs {
                            drop(buf);
                        }
                    });
                },
            );
        }
        group.finish();
    }

    // Size-class scaling
    {
        let mut group = c.benchmark_group("memory_features/size_class_scaling");
        for num_classes in [1, 3, 5, 8] {
            group.bench_with_input(
                BenchmarkId::new("zeropool", num_classes),
                &num_classes,
                |b, &num_classes| {
                    let pool = BufferPool::new();
                    let sizes: Vec<usize> =
                        [4096, 16384, 65536, 262_144, 1_048_576, 4_194_304, 16_777_216, 67_108_864]
                            .iter()
                            .take(num_classes)
                            .copied()
                            .collect();
                    b.iter(|| {
                        let mut bufs = vec![];
                        for &s in &sizes {
                            bufs.push(pool.get(s));
                        }
                        for buf in bufs {
                            black_box(&buf);
                            drop(buf);
                        }
                    });
                },
            );
        }
        group.finish();
    }

    // Min buffer size filtering
    {
        let mut group = c.benchmark_group("memory_features/min_size");
        for min_size in [0, 1024, 64 * 1024] {
            group.bench_with_input(
                BenchmarkId::new("zeropool", min_size),
                &min_size,
                |b, &min_size| {
                    let pool = BufferPool::builder().min_buffer_size(min_size).build();
                    b.iter(|| {
                        let small_buf = pool.get(512);
                        let large_buf = pool.get(128 * 1024);
                        black_box(&small_buf);
                        black_box(&large_buf);
                        drop(small_buf);
                        drop(large_buf);
                    });
                },
            );
        }
        group.finish();
    }
}

criterion_group!(
    benches,
    single_thread,
    multi_thread,
    allocation_patterns,
    cache_behavior,
    memory_features,
);
criterion_main!(benches);
