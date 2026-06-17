//! Benchmark suite for zeropool.
//!
//! ZeroPool-only benchmarks:
//!     cargo bench
//!     cargo bench -- single_thread
//!     cargo bench -- multi_thread
//!     cargo bench -- realistic
//!
//! Full comparison against other crates:
//!     cargo bench --features bench
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;
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

        group.bench_with_input(BenchmarkId::new("vec_alloc", size), &size, |b, &size| {
            b.iter(|| {
                let mut buf = Vec::<u8>::with_capacity(size);
                black_box(&mut buf);
            });
        });

        #[cfg(feature = "bench")]
        {
            group.bench_with_input(BenchmarkId::new("sharded_slab", size), &size, |b, &size| {
                let slab: sharded_slab::Slab<Vec<u8>> = sharded_slab::Slab::new();
                b.iter(|| {
                    let key = slab.insert(Vec::with_capacity(size)).unwrap();
                    black_box(slab.get(key));
                    slab.remove(key);
                });
            });

            group.bench_with_input(BenchmarkId::new("object_pool", size), &size, |b, &size| {
                let pool = object_pool::Pool::new(10, || Vec::<u8>::with_capacity(size));
                b.iter(|| {
                    let mut buf = pool.pull(|| Vec::<u8>::with_capacity(size));
                    black_box(&mut buf);
                });
            });

            group.bench_with_input(BenchmarkId::new("opool", size), &size, |b, &size| {
                use opool::{Pool, PoolAllocator};
                struct Alloc(usize);
                impl PoolAllocator<Vec<u8>> for Alloc {
                    fn allocate(&self) -> Vec<u8> {
                        Vec::with_capacity(self.0)
                    }
                    fn reset(&self, _obj: &mut Vec<u8>) {}
                }
                let pool = Pool::new(64, Alloc(size));
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
    }

    group.finish();
}

// ── Realistic: alloc + write + drop ────────────────────────────────────
//
// We WRITE to each buffer, forcing page faults on fresh allocations.
// Recycled buffers already have their pages mapped — this is where
// pooling pays off.

fn realistic(c: &mut Criterion) {
    // ── Single-threaded write workload ─────────────────────────────
    {
        let mut group = c.benchmark_group("realistic/write_st");

        for size in [4096, 65536, 1024 * 1024] {
            group.throughput(Throughput::Bytes(size as u64));

            group.bench_with_input(BenchmarkId::new("zeropool", size), &size, |b, &size| {
                let pool = ZeroPool::new();
                let warmup = pool.alloc(size);
                drop(warmup);
                b.iter(|| {
                    let mut buf = pool.alloc(size);
                    for i in (0..buf.len()).step_by(4096) {
                        buf[i] = 0xAB;
                    }
                    black_box(&buf);
                    drop(buf);
                });
            });

            group.bench_with_input(BenchmarkId::new("vec_alloc", size), &size, |b, &size| {
                b.iter(|| {
                    let mut buf = vec![0u8; size];
                    for i in (0..buf.len()).step_by(4096) {
                        buf[i] = 0xAB;
                    }
                    black_box(&buf);
                });
            });

            #[cfg(feature = "bench")]
            {
                group.bench_with_input(BenchmarkId::new("object_pool", size), &size, |b, &size| {
                    let pool = object_pool::Pool::new(10, || vec![0u8; size]);
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
                    struct Alloc(usize);
                    impl PoolAllocator<Vec<u8>> for Alloc {
                        fn allocate(&self) -> Vec<u8> {
                            vec![0u8; self.0]
                        }
                        fn reset(&self, _obj: &mut Vec<u8>) {}
                    }
                    let pool = Pool::new(64, Alloc(size));
                    b.iter(|| {
                        let mut buf = pool.get();
                        for i in (0..buf.len().min(size)).step_by(4096) {
                            buf[i] = 0xAB;
                        }
                        black_box(&*buf);
                    });
                });
            }
        }

        group.finish();
    }

    // ── Multi-threaded write workload ──────────────────────────────
    {
        let mut group = c.benchmark_group("realistic/write_mt");
        group.sample_size(50);
        let size = 64 * 1024;
        let ops = 500;

        for threads in [2, 4, 8] {
            group.throughput(Throughput::Bytes((size * threads * ops) as u64));

            group.bench_with_input(
                BenchmarkId::new("zeropool", threads),
                &threads,
                |b, &threads| {
                    let pool = ZeroPool::new();
                    b.iter(|| {
                        thread::scope(|s| {
                            for _ in 0..threads {
                                let pool = &pool;
                                s.spawn(move || {
                                    for _ in 0..ops {
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
                BenchmarkId::new("vec_alloc", threads),
                &threads,
                |b, &threads| {
                    b.iter(|| {
                        thread::scope(|s| {
                            for _ in 0..threads {
                                s.spawn(|| {
                                    for _ in 0..ops {
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

            #[cfg(feature = "bench")]
            {
                group.bench_with_input(
                    BenchmarkId::new("object_pool", threads),
                    &threads,
                    |b, &threads| {
                        use std::sync::Arc;
                        let pool =
                            Arc::new(object_pool::Pool::new(threads * 4, || vec![0u8; size]));
                        b.iter(|| {
                            thread::scope(|s| {
                                for _ in 0..threads {
                                    let pool = &pool;
                                    s.spawn(move || {
                                        for _ in 0..ops {
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
        }

        group.finish();
    }

    // ── Burst: allocate N buffers, touch each, return all ──────────
    {
        let mut group = c.benchmark_group("realistic/burst");
        group.sample_size(50);
        let buf_size = 64 * 1024;

        for n in [10, 50, 100] {
            group.bench_with_input(BenchmarkId::new("zeropool", n), &n, |b, &n| {
                let pool = ZeroPool::new();
                b.iter(|| {
                    let mut bufs: Vec<_> = (0..n)
                        .map(|_| {
                            let mut buf = pool.alloc(buf_size);
                            buf[0] = 0xFF;
                            buf
                        })
                        .collect();
                    black_box(&mut bufs);
                    drop(bufs);
                });
            });

            group.bench_with_input(BenchmarkId::new("vec_alloc", n), &n, |b, &n| {
                b.iter(|| {
                    let mut bufs: Vec<_> = (0..n)
                        .map(|_| {
                            let mut buf = vec![0u8; buf_size];
                            buf[0] = 0xFF;
                            buf
                        })
                        .collect();
                    black_box(&mut bufs);
                    drop(bufs);
                });
            });
        }

        group.finish();
    }

    // ── Mixed sizes (1 KB–1 MB) ───────────────────────────────────
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

        group.bench_function("vec_alloc", |b| {
            b.iter(|| {
                for &s in &sizes {
                    let mut buf = vec![0u8; s];
                    buf[0] = 0xAB;
                    black_box(&buf);
                }
            });
        });

        #[cfg(feature = "bench")]
        group.bench_function("object_pool", |b| {
            let pools: Vec<object_pool::Pool<Vec<u8>>> = sizes
                .iter()
                .map(|&s| object_pool::Pool::new(10, move || vec![0u8; s]))
                .collect();
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
    const OPS: usize = 1000;
    let mut group = c.benchmark_group("multi_thread");
    let size = 64 * 1024;

    for threads in [2, 4, 8] {
        group.throughput(Throughput::Bytes((size * threads * OPS) as u64));

        group.bench_with_input(BenchmarkId::new("zeropool", threads), &threads, |b, &threads| {
            let pool = ZeroPool::new();
            b.iter(|| {
                thread::scope(|s| {
                    for _ in 0..threads {
                        let pool = &pool;
                        s.spawn(move || {
                            for _ in 0..OPS {
                                let mut buf = pool.alloc(size);
                                black_box(&mut buf);
                                drop(buf);
                            }
                        });
                    }
                });
            });
        });

        group.bench_with_input(BenchmarkId::new("vec_alloc", threads), &threads, |b, &threads| {
            b.iter(|| {
                thread::scope(|s| {
                    for _ in 0..threads {
                        s.spawn(|| {
                            for _ in 0..OPS {
                                let mut buf = Vec::<u8>::with_capacity(size);
                                black_box(&mut buf);
                            }
                        });
                    }
                });
            });
        });

        #[cfg(feature = "bench")]
        {
            group.bench_with_input(
                BenchmarkId::new("sharded_slab", threads),
                &threads,
                |b, &threads| {
                    use std::sync::Arc;
                    let slab: Arc<sharded_slab::Slab<Vec<u8>>> =
                        Arc::new(sharded_slab::Slab::new());
                    b.iter(|| {
                        thread::scope(|s| {
                            for _ in 0..threads {
                                let slab = &slab;
                                s.spawn(move || {
                                    for _ in 0..OPS {
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
                BenchmarkId::new("object_pool", threads),
                &threads,
                |b, &threads| {
                    use std::sync::Arc;
                    let pool = Arc::new(object_pool::Pool::new(threads * 2, || {
                        Vec::<u8>::with_capacity(size)
                    }));
                    b.iter(|| {
                        thread::scope(|s| {
                            for _ in 0..threads {
                                let pool = &pool;
                                s.spawn(move || {
                                    for _ in 0..OPS {
                                        let mut buf = pool.pull(|| Vec::with_capacity(size));
                                        black_box(&mut buf);
                                    }
                                });
                            }
                        });
                    });
                },
            );

            group.bench_with_input(BenchmarkId::new("opool", threads), &threads, |b, &threads| {
                use opool::{Pool, PoolAllocator};
                use std::sync::Arc;
                struct Alloc;
                impl PoolAllocator<Vec<u8>> for Alloc {
                    fn allocate(&self) -> Vec<u8> {
                        Vec::with_capacity(64 * 1024)
                    }
                    fn reset(&self, _obj: &mut Vec<u8>) {}
                }
                let pool = Arc::new(Pool::new(128, Alloc));
                b.iter(|| {
                    thread::scope(|s| {
                        for _ in 0..threads {
                            let pool = &pool;
                            s.spawn(move || {
                                for _ in 0..OPS {
                                    let mut buf = pool.get();
                                    black_box(&mut *buf);
                                }
                            });
                        }
                    });
                });
            });
        }
    }

    group.finish();
}

// ── Allocation patterns ────────────────────────────────────────────────

fn allocation_patterns(c: &mut Criterion) {
    let size = 64 * 1024;

    {
        let mut group = c.benchmark_group("alloc_patterns/varied_sizes");

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

        group.bench_function("vec_alloc", |b| {
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

    {
        let mut group = c.benchmark_group("alloc_patterns/reuse_10");

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

        group.bench_function("vec_alloc", |b| {
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

    {
        let mut group = c.benchmark_group("alloc_patterns/burst_100");

        group.bench_function("zeropool", |b| {
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

        group.bench_function("vec_alloc", |b| {
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

    {
        let mut group = c.benchmark_group("alloc_patterns/zipf");
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

        group.bench_function("vec_alloc", |b| {
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
    {
        let mut group = c.benchmark_group("cache/ping_pong");
        group.bench_function("zeropool", |b| {
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

    {
        let mut group = c.benchmark_group("cache/hot_cold");
        group.bench_function("zeropool", |b| {
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

    {
        let mut group = c.benchmark_group("cache/multi_size");
        group.bench_function("zeropool", |b| {
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

    {
        let mut group = c.benchmark_group("cache/tls_depth");
        for depth in [2, 4, 8] {
            group.bench_with_input(BenchmarkId::new("zeropool", depth), &depth, |b, &depth| {
                let pool = ZeroPool::new().tls_cache_size(15).min_buffer_size(0);
                b.iter(|| {
                    let mut bufs = vec![];
                    for _ in 0..depth {
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
            });
        }
        group.finish();
    }

    {
        let mut group = c.benchmark_group("cache/eviction");
        group.sample_size(50);
        group.bench_function("zeropool", |b| {
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

    {
        let mut group = c.benchmark_group("cache/contention");
        for threads in [1, 4, 8, 16] {
            group.bench_with_input(
                BenchmarkId::new("threads", threads),
                &threads,
                |b, &threads| {
                    let pool = ZeroPool::new().tls_cache_size(1).min_buffer_size(0);
                    b.iter(|| {
                        let pool = &pool;
                        thread::scope(|s| {
                            let mut handles = vec![];
                            for _ in 0..threads {
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
    {
        let mut group = c.benchmark_group("mem/pinned");
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

        group.bench_function("pinned_warm", |b| {
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

    {
        let mut group = c.benchmark_group("mem/prealloc");
        for n in [0, 10, 50, 100] {
            group.bench_with_input(BenchmarkId::new("warm", n), &n, |b, &n| {
                let pool = ZeroPool::new();
                if n > 0 {
                    pool.warm(n, 64 * 1024);
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
            });
        }
        group.finish();
    }

    {
        let mut group = c.benchmark_group("mem/pressure");
        for cap in [5, 10, 20] {
            group.bench_with_input(BenchmarkId::new("max_per_class", cap), &cap, |b, &cap| {
                let pool = ZeroPool::new().max_buffers_per_class(cap);
                b.iter(|| {
                    let mut bufs = vec![];
                    for _ in 0..(cap * 2) {
                        bufs.push(pool.alloc(64 * 1024));
                    }
                    for buf in bufs {
                        drop(buf);
                    }
                });
            });
        }
        group.finish();
    }

    {
        let mut group = c.benchmark_group("mem/size_classes");
        for n in [1, 3, 5, 8] {
            group.bench_with_input(BenchmarkId::new("classes", n), &n, |b, &n| {
                let pool = ZeroPool::new();
                let sizes: Vec<usize> =
                    [4096, 16384, 65536, 262_144, 1_048_576, 4_194_304, 16_777_216, 67_108_864]
                        .iter()
                        .take(n)
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
            });
        }
        group.finish();
    }

    {
        let mut group = c.benchmark_group("mem/min_size_filter");
        for min in [0, 1024, 64 * 1024] {
            group.bench_with_input(BenchmarkId::new("min", min), &min, |b, &min| {
                let pool = ZeroPool::new().min_buffer_size(min);
                b.iter(|| {
                    let small = pool.alloc(512);
                    let large = pool.alloc(128 * 1024);
                    black_box(&small);
                    black_box(&large);
                    drop(small);
                    drop(large);
                });
            });
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
