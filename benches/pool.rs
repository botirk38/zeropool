//! Consolidated benchmark suite for zeropool.
//!
//! Run all:           `cargo bench`
//! Run targeted:      `cargo bench -- single_thread`
//!                    `cargo bench -- multi_thread`
//!                    `cargo bench -- realistic`
//!                    `cargo bench -- allocation_patterns`
//!                    `cargo bench -- cache_behavior`
//!                    `cargo bench -- memory_features`
#![allow(missing_docs)]

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;
use std::sync::Arc;
use std::thread;
use zeropool::ZeroPool;

// ── Single-thread hot-path ─────────────────────────────────────────────

fn single_thread(c: &mut Criterion) {
    let mut group = c.benchmark_group("single_thread");

    for size in [1024, 4096, 16384, 65536, 1024 * 1024] {
        group.throughput(Throughput::Bytes(size as u64));

        group.bench_with_input(BenchmarkId::new("zeropool", size), &size, |b, &size| {
            let pool = ZeroPool::new();
            b.iter(|| {
                let mut buf = pool.alloc(size);
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

        group.bench_with_input(BenchmarkId::new("object_pool", size), &size, |b, &size| {
            use object_pool::Pool;
            let pool: Pool<Vec<u8>> = Pool::new(10, || Vec::with_capacity(size));
            b.iter(|| {
                let mut buf = pool.pull(|| Vec::with_capacity(size));
                black_box(&mut buf);
            });
        });

        group.bench_with_input(BenchmarkId::new("opool", size), &size, |b, &size| {
            use opool::{Pool, PoolAllocator};
            struct VecAlloc(usize);
            impl PoolAllocator<Vec<u8>> for VecAlloc {
                fn allocate(&self) -> Vec<u8> {
                    Vec::with_capacity(self.0)
                }
                fn reset(&self, _obj: &mut Vec<u8>) {}
            }
            let pool = Pool::new(64, VecAlloc(size));
            b.iter(|| {
                let mut buf = pool.get();
                black_box(&mut *buf);
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

// ── Realistic: alloc + write + drop ────────────────────────────────────
//
// The critical difference vs the hot-path benchmark: we WRITE to each
// buffer, forcing page faults on cold allocations.  Recycled buffers
// already have their pages mapped → this is where pooling pays off.

fn realistic(c: &mut Criterion) {
    // ── Single-threaded write workload ─────────────────────────────
    {
        let mut group = c.benchmark_group("realistic/write_single");

        for size in [4096, 65536, 1024 * 1024] {
            group.throughput(Throughput::Bytes(size as u64));

            group.bench_with_input(BenchmarkId::new("zeropool", size), &size, |b, &size| {
                let pool = ZeroPool::new();
                // Warm the pool so all iterations reuse mapped pages.
                let warmup = pool.alloc(size);
                drop(warmup);
                b.iter(|| {
                    let mut buf = pool.alloc(size);
                    // Write every 4KB page to force faults on cold allocs.
                    for i in (0..buf.len()).step_by(4096) {
                        buf[i] = 0xAB;
                    }
                    black_box(&buf);
                    drop(buf);
                });
            });

            group.bench_with_input(BenchmarkId::new("no_pool", size), &size, |b, &size| {
                b.iter(|| {
                    let mut buf = vec![0u8; size];
                    for i in (0..buf.len()).step_by(4096) {
                        buf[i] = 0xAB;
                    }
                    black_box(&buf);
                });
            });

            group.bench_with_input(BenchmarkId::new("object_pool", size), &size, |b, &size| {
                use object_pool::Pool;
                let pool: Pool<Vec<u8>> = Pool::new(10, || vec![0u8; size]);
                b.iter(|| {
                    let mut buf = pool.pull(|| vec![0u8; size]);
                    for i in (0..size).step_by(4096) {
                        buf[i] = 0xAB;
                    }
                    black_box(&*buf);
                });
            });

            group.bench_with_input(BenchmarkId::new("opool", size), &size, |b, &size| {
                use opool::{Pool, PoolAllocator};
                struct VecAlloc(usize);
                impl PoolAllocator<Vec<u8>> for VecAlloc {
                    fn allocate(&self) -> Vec<u8> {
                        vec![0u8; self.0]
                    }
                    fn reset(&self, _obj: &mut Vec<u8>) {}
                }
                let pool = Pool::new(64, VecAlloc(size));
                b.iter(|| {
                    let mut buf = pool.get();
                    for i in (0..buf.len().min(size)).step_by(4096) {
                        buf[i] = 0xAB;
                    }
                    black_box(&*buf);
                });
            });
        }

        group.finish();
    }

    // ── Multi-threaded write workload ──────────────────────────────
    {
        let mut group = c.benchmark_group("realistic/write_multi");
        group.sample_size(50);
        let size = 64 * 1024;
        let iters_per_thread = 500;

        for num_threads in [2, 4, 8] {
            group.throughput(Throughput::Bytes((size * num_threads * iters_per_thread) as u64));

            group.bench_with_input(
                BenchmarkId::new("zeropool", num_threads),
                &num_threads,
                |b, &num_threads| {
                    let pool = ZeroPool::new();
                    b.iter(|| {
                        thread::scope(|s| {
                            for _ in 0..num_threads {
                                let pool = &pool;
                                s.spawn(move || {
                                    for _ in 0..iters_per_thread {
                                        let mut buf = pool.alloc(size);
                                        for i in (0..buf.len()).step_by(4096) {
                                            buf[i] = 0xAB;
                                        }
                                        black_box(&buf);
                                        drop(buf);
                                    }
                                });
                            }
                        });
                    });
                },
            );

            group.bench_with_input(
                BenchmarkId::new("no_pool", num_threads),
                &num_threads,
                |b, &num_threads| {
                    b.iter(|| {
                        thread::scope(|s| {
                            for _ in 0..num_threads {
                                s.spawn(|| {
                                    for _ in 0..iters_per_thread {
                                        let mut buf = vec![0u8; size];
                                        for i in (0..buf.len()).step_by(4096) {
                                            buf[i] = 0xAB;
                                        }
                                        black_box(&buf);
                                    }
                                });
                            }
                        });
                    });
                },
            );

            group.bench_with_input(
                BenchmarkId::new("object_pool", num_threads),
                &num_threads,
                |b, &num_threads| {
                    use object_pool::Pool;
                    let pool: Arc<Pool<Vec<u8>>> =
                        Arc::new(Pool::new(num_threads * 4, || vec![0u8; size]));
                    b.iter(|| {
                        thread::scope(|s| {
                            for _ in 0..num_threads {
                                let pool = &pool;
                                s.spawn(move || {
                                    for _ in 0..iters_per_thread {
                                        let mut buf = pool.pull(|| vec![0u8; size]);
                                        for i in (0..size).step_by(4096) {
                                            buf[i] = 0xAB;
                                        }
                                        black_box(&*buf);
                                    }
                                });
                            }
                        });
                    });
                },
            );
        }

        group.finish();
    }

    // ── Burst: allocate N buffers, use them, return all ────────────
    {
        let mut group = c.benchmark_group("realistic/burst");
        group.sample_size(50);

        for burst_size in [10, 50, 100] {
            let buf_size = 64 * 1024;

            group.bench_with_input(
                BenchmarkId::new("zeropool", burst_size),
                &burst_size,
                |b, &burst_size| {
                    let pool = ZeroPool::new();
                    b.iter(|| {
                        let mut bufs: Vec<_> = (0..burst_size)
                            .map(|_| {
                                let mut buf = pool.alloc(buf_size);
                                buf[0] = 0xFF;
                                buf
                            })
                            .collect();
                        black_box(&mut bufs);
                        drop(bufs);
                    });
                },
            );

            group.bench_with_input(
                BenchmarkId::new("no_pool", burst_size),
                &burst_size,
                |b, &burst_size| {
                    b.iter(|| {
                        let mut bufs: Vec<_> = (0..burst_size)
                            .map(|_| {
                                let mut buf = vec![0u8; buf_size];
                                buf[0] = 0xFF;
                                buf
                            })
                            .collect();
                        black_box(&mut bufs);
                        drop(bufs);
                    });
                },
            );
        }

        group.finish();
    }

    // ── Mixed sizes: varied allocation sizes per iteration ─────────
    {
        let mut group = c.benchmark_group("realistic/mixed_sizes");

        let sizes = [1024, 4096, 16384, 65536, 262_144, 1_048_576];

        group.bench_function("zeropool", |b| {
            let pool = ZeroPool::new();
            b.iter(|| {
                for &s in &sizes {
                    let mut buf = pool.alloc(s);
                    buf[0] = 0xAB;
                    black_box(&buf);
                    drop(buf);
                }
            });
        });

        group.bench_function("no_pool", |b| {
            b.iter(|| {
                for &s in &sizes {
                    let mut buf = vec![0u8; s];
                    buf[0] = 0xAB;
                    black_box(&buf);
                }
            });
        });

        group.bench_function("object_pool", |b| {
            // object_pool only supports one size — use separate pools
            use object_pool::Pool;
            let pools: Vec<Pool<Vec<u8>>> =
                sizes.iter().map(|&s| Pool::new(10, move || vec![0u8; s])).collect();
            b.iter(|| {
                for (i, &s) in sizes.iter().enumerate() {
                    let mut buf = pools[i].pull(|| vec![0u8; s]);
                    buf[0] = 0xAB;
                    black_box(&*buf);
                }
            });
        });

        group.finish();
    }
}

// ── Multi-thread scaling (hot path, no writes) ────────────────────────

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
                let pool = ZeroPool::new();
                b.iter(|| {
                    thread::scope(|s| {
                        for _ in 0..num_threads {
                            let pool = &pool;
                            s.spawn(move || {
                                for _ in 0..ITERATIONS_PER_THREAD {
                                    let mut buf = pool.alloc(size);
                                    black_box(&mut buf);
                                    drop(buf);
                                }
                            });
                        }
                    });
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("no_pool", num_threads),
            &num_threads,
            |b, &num_threads| {
                b.iter(|| {
                    thread::scope(|s| {
                        for _ in 0..num_threads {
                            s.spawn(|| {
                                for _ in 0..ITERATIONS_PER_THREAD {
                                    let mut buf = Vec::<u8>::with_capacity(size);
                                    black_box(&mut buf);
                                }
                            });
                        }
                    });
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("sharded_slab", num_threads),
            &num_threads,
            |b, &num_threads| {
                use sharded_slab::Slab;
                let slab: Arc<Slab<Vec<u8>>> = Arc::new(Slab::new());
                b.iter(|| {
                    thread::scope(|s| {
                        for _ in 0..num_threads {
                            let slab = &slab;
                            s.spawn(move || {
                                for _ in 0..ITERATIONS_PER_THREAD {
                                    let key = slab.insert(Vec::with_capacity(size)).unwrap();
                                    black_box(slab.get(key));
                                    slab.remove(key);
                                }
                            });
                        }
                    });
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("object_pool", num_threads),
            &num_threads,
            |b, &num_threads| {
                use object_pool::Pool;
                let pool: Arc<Pool<Vec<u8>>> =
                    Arc::new(Pool::new(num_threads * 2, || Vec::with_capacity(size)));
                b.iter(|| {
                    thread::scope(|s| {
                        for _ in 0..num_threads {
                            let pool = &pool;
                            s.spawn(move || {
                                for _ in 0..ITERATIONS_PER_THREAD {
                                    let mut buf = pool.pull(|| Vec::with_capacity(size));
                                    black_box(&mut buf);
                                }
                            });
                        }
                    });
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("opool", num_threads),
            &num_threads,
            |b, &num_threads| {
                use opool::{Pool, PoolAllocator};
                struct VecAlloc;
                impl PoolAllocator<Vec<u8>> for VecAlloc {
                    fn allocate(&self) -> Vec<u8> {
                        Vec::with_capacity(64 * 1024)
                    }
                    fn reset(&self, _obj: &mut Vec<u8>) {}
                }
                let pool = Arc::new(Pool::new(128, VecAlloc));
                b.iter(|| {
                    thread::scope(|s| {
                        for _ in 0..num_threads {
                            let pool = &pool;
                            s.spawn(move || {
                                for _ in 0..ITERATIONS_PER_THREAD {
                                    let mut buf = pool.get();
                                    black_box(&mut *buf);
                                }
                            });
                        }
                    });
                });
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
            let pool = ZeroPool::new();
            let sizes = [1024, 4096, 16384, 65536, 256 * 1024];
            b.iter(|| {
                for &s in &sizes {
                    let buf = pool.alloc(s);
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
            let pool = ZeroPool::new();
            pool.warm(10, size);
            b.iter(|| {
                let mut bufs = vec![];
                for _ in 0..10 {
                    bufs.push(pool.alloc(size));
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
            let pool = ZeroPool::new();
            b.iter(|| {
                let mut bufs = vec![];
                for _ in 0..100 {
                    bufs.push(pool.alloc(size));
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
            let pool = ZeroPool::new();
            b.iter(|| {
                for &(s, count) in &sizes {
                    for _ in 0..count {
                        let buf = pool.alloc(s);
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
            let pool = ZeroPool::new().tls_cache_size(10);
            pool.warm(20, 1024 * 1024);
            b.iter(|| {
                for _ in 0..100 {
                    let buf1 = pool.alloc(1024);
                    let buf2 = pool.alloc(1024 * 1024);
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
            let pool = ZeroPool::new().tls_cache_size(10);
            pool.warm(10, 64 * 1024);
            b.iter(|| {
                let mut bufs = vec![];
                for _ in 0..3 {
                    bufs.push(pool.alloc(64 * 1024));
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
            let pool = ZeroPool::new().tls_cache_size(15);
            let sizes = [4 * 1024, 64 * 1024, 1024 * 1024];
            b.iter(|| {
                for _ in 0..100 {
                    for &s in &sizes {
                        let buf = pool.alloc(s);
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
                    let pool = ZeroPool::new().tls_cache_size(15).min_buffer_size(0);
                    b.iter(|| {
                        let mut bufs = vec![];
                        for _ in 0..tls_size {
                            bufs.push(pool.alloc(64 * 1024));
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
            let pool = ZeroPool::new()
                .max_buffers_per_class(100)
                .tls_cache_size(8)
                .min_buffer_size(4096);
            b.iter(|| {
                let mut bufs = vec![];
                for _ in 0..10 {
                    bufs.push(pool.alloc(64 * 1024));
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
                    let pool = ZeroPool::new().tls_cache_size(1).min_buffer_size(0);
                    b.iter(|| {
                        let pool = &pool;
                        thread::scope(|s| {
                            let mut handles = vec![];
                            for _ in 0..num_threads {
                                handles.push(s.spawn(|| {
                                    for _ in 0..1000 {
                                        let buf = pool.alloc(1024);
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
            let pool = ZeroPool::new().pinned_memory(false);
            b.iter(|| {
                let mut buf = pool.alloc(size);
                black_box(&mut buf);
                drop(buf);
            });
        });

        group.bench_function("pinned", |b| {
            let pool = ZeroPool::new().pinned_memory(true);
            b.iter(|| {
                let mut buf = pool.alloc(size);
                black_box(&mut buf);
                drop(buf);
            });
        });

        group.bench_function("pinned_preallocated", |b| {
            let pool = ZeroPool::new().pinned_memory(true);
            pool.warm(10, size);
            b.iter(|| {
                let mut buf = pool.alloc(size);
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
                    let pool = ZeroPool::new();
                    if prealloc_count > 0 {
                        pool.warm(prealloc_count, 64 * 1024);
                    }
                    b.iter(|| {
                        let mut bufs = vec![];
                        for _ in 0..10 {
                            bufs.push(pool.alloc(64 * 1024));
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
                    let pool = ZeroPool::new().max_buffers_per_class(max_buffers);
                    b.iter(|| {
                        let mut bufs = vec![];
                        for _ in 0..(max_buffers * 2) {
                            bufs.push(pool.alloc(64 * 1024));
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
                    let pool = ZeroPool::new();
                    let sizes: Vec<usize> =
                        [4096, 16384, 65536, 262_144, 1_048_576, 4_194_304, 16_777_216, 67_108_864]
                            .iter()
                            .take(num_classes)
                            .copied()
                            .collect();
                    b.iter(|| {
                        let mut bufs = vec![];
                        for &s in &sizes {
                            bufs.push(pool.alloc(s));
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
                    let pool = ZeroPool::new().min_buffer_size(min_size);
                    b.iter(|| {
                        let small_buf = pool.alloc(512);
                        let large_buf = pool.alloc(128 * 1024);
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
    realistic,
    allocation_patterns,
    cache_behavior,
    memory_features,
);
criterion_main!(benches);
