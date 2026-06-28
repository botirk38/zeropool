#![allow(missing_docs)]

// Criterion benchmark suite for ZeroPool.
//
// Run a subset:
//
// ```text
// cargo bench -- hot_path
// cargo bench -- realistic
// cargo bench -- full_overwrite
// cargo bench -- scale
// cargo bench -- mixed
// cargo bench -- burst
// cargo bench -- contention
// cargo bench -- stats
// ```
//
// Run with full comparison crates (opool, object_pool, sharded_slab, bytes):
//
// ```text
// cargo bench --bench pool --features bench
// cargo bench --bench pool --features bench,bench-alloc-mimalloc
// cargo bench --bench pool --features bench,bench-alloc-tcmalloc
// cargo bench --bench pool --features bench,bench-alloc-jemalloc
// ```

#[cfg(feature = "bench-alloc-mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(all(
    not(feature = "bench-alloc-mimalloc"),
    target_os = "linux",
    feature = "bench-alloc-tcmalloc"
))]
#[global_allocator]
static GLOBAL: tcmalloc_better::TCMalloc = tcmalloc_better::TCMalloc;

#[cfg(all(
    not(feature = "bench-alloc-mimalloc"),
    any(not(feature = "bench-alloc-tcmalloc"), not(target_os = "linux")),
    unix,
    feature = "bench-alloc-jemalloc"
))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

mod common;

use std::hint::black_box;
use std::thread;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

use common::sizes::{
    BURST_SIZES, MIXED_SIZES, SCALE_THREAD_COUNTS, SIZES, THREAD_COUNTS, ZIPF_SIZES,
};
use common::touch::{touch_pages, write_all, write_all_uninit};

use zeropool::ZeroPool;

// ── hot_path ──────────────────────────────────────────────────────────
//
// Pure acquire/drop, no page writes. Honest microbenchmark.

fn hot_path(c: &mut Criterion) {
    let mut group = c.benchmark_group("hot_path");

    for &size in SIZES {
        group.throughput(Throughput::Bytes(size as u64));

        group.bench_with_input(BenchmarkId::new("zeropool", size), &size, |b, &size| {
            let pool = common::competitors::zeropool_default();
            b.iter(|| {
                let buf = pool.alloc(size);
                black_box(&buf);
                drop(buf);
            });
        });

        group.bench_with_input(BenchmarkId::new("zeropool_uninit", size), &size, |b, &size| {
            let pool = common::competitors::zeropool_default();
            b.iter(|| {
                let buf = pool.alloc_uninit(size);
                black_box(&buf);
                drop(buf);
            });
        });

        group.bench_with_input(BenchmarkId::new("zeropool_stats", size), &size, |b, &size| {
            let pool = common::competitors::zeropool_with_stats();
            b.iter(|| {
                let buf = pool.alloc(size);
                black_box(&buf);
                drop(buf);
            });
        });

        group.bench_with_input(BenchmarkId::new("vec_capacity", size), &size, |b, &size| {
            b.iter(|| {
                let buf = Vec::<u8>::with_capacity(size);
                black_box(&buf);
            });
        });

        group.bench_with_input(BenchmarkId::new("vec_reuse_capacity", size), &size, |b, &size| {
            let mut buf = Vec::<u8>::with_capacity(size);
            b.iter(|| {
                buf.clear();
                black_box(&buf);
            });
        });

        group.bench_with_input(BenchmarkId::new("vec_reuse_zeroed", size), &size, |b, &size| {
            let mut buf = vec![0u8; size];
            b.iter(|| {
                buf.clear();
                buf.resize(size, 0);
                black_box(&buf);
            });
        });

        #[cfg(feature = "bench")]
        {
            group.bench_with_input(BenchmarkId::new("object_pool", size), &size, |b, &size| {
                let pool = common::competitors::object_pool_for_size(size, 16);
                b.iter(|| {
                    let buf = pool.pull(|| Vec::<u8>::with_capacity(size));
                    black_box(&*buf);
                });
            });

            group.bench_with_input(BenchmarkId::new("opool", size), &size, |b, &size| {
                let pool = common::competitors::opool_capacity_zero(64, size);
                b.iter(|| {
                    let buf = pool.get();
                    black_box(&*buf);
                });
            });

            group.bench_with_input(BenchmarkId::new("bytes", size), &size, |b, &size| {
                b.iter(|| {
                    let buf = bytes::BytesMut::with_capacity(size);
                    black_box(&buf);
                });
            });
        }
    }

    group.finish();
}

// ── realistic ─────────────────────────────────────────────────────────
//
// Alloc + write one byte per page + drop. This is the main README number.

fn realistic(c: &mut Criterion) {
    // Single-thread
    {
        let mut group = c.benchmark_group("realistic_write_st");
        for &size in SIZES {
            group.throughput(Throughput::Bytes(size as u64));

            group.bench_with_input(BenchmarkId::new("zeropool", size), &size, |b, &size| {
                let pool = common::competitors::zeropool_no_min();
                pool.warm(1, size);
                b.iter(|| {
                    let mut buf = pool.alloc(size);
                    touch_pages(&mut buf);
                    drop(buf);
                });
            });

            group.bench_with_input(BenchmarkId::new("vec_alloc", size), &size, |b, &size| {
                b.iter(|| {
                    let mut buf = vec![0u8; size];
                    touch_pages(&mut buf);
                });
            });

            group.bench_with_input(BenchmarkId::new("vec_reuse", size), &size, |b, &size| {
                let mut buf = vec![0u8; size];
                b.iter(|| {
                    buf.clear();
                    buf.resize(size, 0);
                    touch_pages(&mut buf);
                });
            });

            #[cfg(feature = "bench")]
            {
                group.bench_with_input(BenchmarkId::new("object_pool", size), &size, |b, &size| {
                    let pool = common::competitors::object_pool_zeroed(size, 16);
                    b.iter(|| {
                        let mut buf = pool.pull(|| vec![0u8; size]);
                        touch_pages(&mut buf);
                    });
                });

                group.bench_with_input(BenchmarkId::new("opool", size), &size, |b, &size| {
                    let pool = common::competitors::opool_capacity(64, size);
                    b.iter(|| {
                        let mut buf = pool.get();
                        touch_pages(&mut buf);
                    });
                });
            }
        }
        group.finish();
    }

    // Multi-thread
    {
        let mut group = c.benchmark_group("realistic_write_mt");
        group.sample_size(30);
        let size = 64 * 1024;
        let ops = 200;

        for &threads in THREAD_COUNTS {
            group.throughput(Throughput::Bytes((size * threads * ops) as u64));

            group.bench_with_input(
                BenchmarkId::new("zeropool", threads),
                &threads,
                |b, &threads| {
                    let pool = common::competitors::zeropool_no_min();
                    b.iter(|| {
                        thread::scope(|s| {
                            for _ in 0..threads {
                                let pool = &pool;
                                s.spawn(move || {
                                    for _ in 0..ops {
                                        let mut buf = pool.alloc(size);
                                        touch_pages(&mut buf);
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
                                        touch_pages(&mut buf);
                                    }
                                });
                            }
                        });
                    });
                },
            );

            group.bench_with_input(
                BenchmarkId::new("vec_thread_local_reuse", threads),
                &threads,
                |b, &threads| {
                    b.iter(|| {
                        thread::scope(|s| {
                            for _ in 0..threads {
                                s.spawn(|| {
                                    let mut buf = vec![0u8; size];
                                    for _ in 0..ops {
                                        buf.clear();
                                        buf.resize(size, 0);
                                        touch_pages(&mut buf);
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
                        let pool = common::competitors::object_pool_zeroed(size, threads * 4);
                        b.iter(|| {
                            thread::scope(|s| {
                                for _ in 0..threads {
                                    let pool = &pool;
                                    s.spawn(move || {
                                        for _ in 0..ops {
                                            let mut buf = pool.pull(|| vec![0u8; size]);
                                            touch_pages(&mut buf);
                                        }
                                    });
                                }
                            });
                        });
                    },
                );

                group.bench_with_input(
                    BenchmarkId::new("opool", threads),
                    &threads,
                    |b, &threads| {
                        let pool = common::competitors::opool_capacity(threads * 4, size);
                        b.iter(|| {
                            thread::scope(|s| {
                                for _ in 0..threads {
                                    let pool = &pool;
                                    s.spawn(move || {
                                        for _ in 0..ops {
                                            let mut buf = pool.get();
                                            touch_pages(&mut buf);
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
}

// ── full_overwrite ─────────────────────────────────────────────────────
//
// Alloc + initialize every byte + drop. This is the fair workload for
// explicit uninitialized/capacity APIs because callers never read old bytes.

fn full_overwrite(c: &mut Criterion) {
    let mut group = c.benchmark_group("full_overwrite");

    for &size in SIZES {
        group.throughput(Throughput::Bytes(size as u64));

        group.bench_with_input(BenchmarkId::new("zeropool_zeroed", size), &size, |b, &size| {
            let pool = common::competitors::zeropool_no_min();
            pool.warm(1, size);
            b.iter(|| {
                let mut buf = pool.alloc(size);
                write_all(&mut buf);
                drop(buf);
            });
        });

        group.bench_with_input(BenchmarkId::new("zeropool_uninit", size), &size, |b, &size| {
            let pool = common::competitors::zeropool_no_min();
            pool.warm(1, size);
            b.iter(|| {
                let mut buf = pool.alloc_uninit(size);
                write_all_uninit(buf.as_uninit_mut());
                // SAFETY: `write_all_uninit` initialized the entire buffer.
                let buf = unsafe { buf.assume_init() };
                drop(buf);
            });
        });

        group.bench_with_input(BenchmarkId::new("vec_zeroed", size), &size, |b, &size| {
            b.iter(|| {
                let mut buf = vec![0u8; size];
                write_all(&mut buf);
            });
        });

        group.bench_with_input(BenchmarkId::new("vec_capacity", size), &size, |b, &size| {
            b.iter(|| {
                let mut buf = Vec::<u8>::with_capacity(size);
                write_all_uninit(&mut buf.spare_capacity_mut()[..size]);
                // SAFETY: `write_all_uninit` initializes exactly `size` bytes.
                unsafe { buf.set_len(size) };
                black_box(&buf);
            });
        });

        group.bench_with_input(BenchmarkId::new("vec_reuse_zeroed", size), &size, |b, &size| {
            let mut buf = vec![0u8; size];
            b.iter(|| {
                buf.clear();
                buf.resize(size, 0);
                write_all(&mut buf);
            });
        });

        #[cfg(feature = "bench")]
        {
            group.bench_with_input(BenchmarkId::new("object_pool", size), &size, |b, &size| {
                let pool = common::competitors::object_pool_zeroed(size, 16);
                b.iter(|| {
                    let mut buf = pool.pull(|| vec![0u8; size]);
                    write_all(&mut buf);
                });
            });

            group.bench_with_input(BenchmarkId::new("opool", size), &size, |b, &size| {
                let pool = common::competitors::opool_capacity(64, size);
                b.iter(|| {
                    let mut buf = pool.get();
                    write_all(&mut buf);
                });
            });
        }
    }

    group.finish();
}

// ── scale_sustained ───────────────────────────────────────────────────
//
// Longer-running server-like workload. Many ops/thread, default TLS depth.

fn scale_sustained(c: &mut Criterion) {
    let mut group = c.benchmark_group("scale_sustained");
    group.sample_size(20);
    group.measurement_time(Duration::from_secs(8));

    let size = 64 * 1024;
    let ops_per_thread: usize = std::env::var("ZEROPOOL_SCALE_OPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10_000);

    for &threads in SCALE_THREAD_COUNTS {
        group.throughput(Throughput::Bytes((size * threads * ops_per_thread) as u64));

        group.bench_with_input(BenchmarkId::new("zeropool", threads), &threads, |b, &threads| {
            let pool = common::competitors::zeropool_no_min();
            b.iter(|| {
                thread::scope(|s| {
                    for _ in 0..threads {
                        let pool = &pool;
                        s.spawn(move || {
                            for _ in 0..ops_per_thread {
                                let mut buf = pool.alloc(size);
                                touch_pages(&mut buf);
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
                            for _ in 0..ops_per_thread {
                                let mut buf = vec![0u8; size];
                                touch_pages(&mut buf);
                            }
                        });
                    }
                });
            });
        });

        #[cfg(feature = "bench")]
        group.bench_with_input(
            BenchmarkId::new("object_pool", threads),
            &threads,
            |b, &threads| {
                let pool = common::competitors::object_pool_zeroed(size, threads * 4);
                b.iter(|| {
                    thread::scope(|s| {
                        for _ in 0..threads {
                            let pool = &pool;
                            s.spawn(move || {
                                for _ in 0..ops_per_thread {
                                    let mut buf = pool.pull(|| vec![0u8; size]);
                                    touch_pages(&mut buf);
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

// ── mixed_sizes ───────────────────────────────────────────────────────
//
// One ZeroPool vs N competitor pools. Models real apps that allocate
// many different buffer sizes.

fn mixed_sizes(c: &mut Criterion) {
    // Uniform round-robin through all sizes.
    {
        let mut group = c.benchmark_group("mixed_uniform");

        group.bench_function("zeropool", |b| {
            let pool = common::competitors::zeropool_no_min();
            b.iter(|| {
                for &s in MIXED_SIZES {
                    let mut buf = pool.alloc(s);
                    buf[0] = 0xAB;
                    black_box(&buf);
                    drop(buf);
                }
            });
        });

        group.bench_function("vec_alloc", |b| {
            b.iter(|| {
                for &s in MIXED_SIZES {
                    let mut buf = vec![0u8; s];
                    buf[0] = 0xAB;
                    black_box(&buf);
                }
            });
        });

        #[cfg(feature = "bench")]
        {
            use std::sync::Arc;

            group.bench_function("object_pool", |b| {
                let pools: Vec<Arc<object_pool::Pool<Vec<u8>>>> = MIXED_SIZES
                    .iter()
                    .map(|&s| common::competitors::object_pool_zeroed(s, 16))
                    .collect();
                b.iter(|| {
                    for (i, &s) in MIXED_SIZES.iter().enumerate() {
                        let mut buf = pools[i].pull(|| vec![0u8; s]);
                        buf[0] = 0xAB;
                        black_box(&*buf);
                    }
                });
            });

            group.bench_function("opool", |b| {
                let pools: Vec<_> = MIXED_SIZES
                    .iter()
                    .map(|&s| common::competitors::opool_capacity(16, s))
                    .collect();
                b.iter(|| {
                    for (i, &_s) in MIXED_SIZES.iter().enumerate() {
                        let mut buf = pools[i].get();
                        buf[0] = 0xAB;
                        black_box(&*buf);
                    }
                });
            });
        }

        group.finish();
    }

    // Zipf-like distribution: small buffers are common, large ones rare.
    {
        let mut group = c.benchmark_group("mixed_zipf");

        group.bench_function("zeropool", |b| {
            let pool = common::competitors::zeropool_no_min();
            b.iter(|| {
                for &(s, count) in ZIPF_SIZES {
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
                for &(s, count) in ZIPF_SIZES {
                    for _ in 0..count {
                        let buf = Vec::<u8>::with_capacity(s);
                        black_box(&buf);
                    }
                }
            });
        });

        #[cfg(feature = "bench")]
        {
            use std::sync::Arc;

            group.bench_function("object_pool", |b| {
                let pools: Vec<Arc<object_pool::Pool<Vec<u8>>>> = ZIPF_SIZES
                    .iter()
                    .map(|&(s, _)| common::competitors::object_pool_for_size(s, 16))
                    .collect();
                b.iter(|| {
                    for (i, &(s, count)) in ZIPF_SIZES.iter().enumerate() {
                        for _ in 0..count {
                            let buf = pools[i].pull(|| Vec::<u8>::with_capacity(s));
                            black_box(&*buf);
                        }
                    }
                });
            });
        }

        group.finish();
    }
}

// ── burst ─────────────────────────────────────────────────────────────
//
// Allocate N buffers, touch each, drop all.

fn burst(c: &mut Criterion) {
    let mut group = c.benchmark_group("burst");
    group.sample_size(30);

    let size = 64 * 1024;

    for &n in BURST_SIZES {
        group.throughput(Throughput::Bytes((size * n) as u64));

        group.bench_with_input(BenchmarkId::new("zeropool", n), &n, |b, &n| {
            let pool = common::competitors::zeropool_no_min();
            b.iter(|| {
                let mut bufs: Vec<_> = (0..n)
                    .map(|_| {
                        let mut buf = pool.alloc(size);
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
                        let mut buf = vec![0u8; size];
                        buf[0] = 0xFF;
                        buf
                    })
                    .collect();
                black_box(&mut bufs);
                drop(bufs);
            });
        });

        #[cfg(feature = "bench")]
        {
            group.bench_with_input(BenchmarkId::new("object_pool", n), &n, |b, &n| {
                let pool = common::competitors::object_pool_zeroed(size, n + 8);
                b.iter(|| {
                    let mut bufs: Vec<_> = (0..n)
                        .map(|_| {
                            let mut buf = pool.pull(|| vec![0u8; size]);
                            buf[0] = 0xFF;
                            buf
                        })
                        .collect();
                    black_box(&mut bufs);
                    drop(bufs);
                });
            });

            group.bench_with_input(BenchmarkId::new("opool", n), &n, |b, &n| {
                let pool = common::competitors::opool_capacity(n + 8, size);
                b.iter(|| {
                    let mut bufs: Vec<_> = (0..n)
                        .map(|_| {
                            let mut buf = pool.get();
                            buf[0] = 0xFF;
                            buf
                        })
                        .collect();
                    black_box(&mut bufs);
                    drop(bufs);
                });
            });
        }
    }

    group.finish();
}

// ── contention ────────────────────────────────────────────────────────
//
// Stress shared fallback. Sweep TLS depth × thread count.

fn contention(c: &mut Criterion) {
    let mut group = c.benchmark_group("contention");
    group.sample_size(20);

    let size = 64 * 1024;
    let ops: usize = 500;

    for &tls_depth in &[1usize, 4, 8] {
        for &threads in THREAD_COUNTS {
            group.throughput(Throughput::Bytes((size * threads * ops) as u64));

            group.bench_with_input(
                BenchmarkId::new(format!("tls_{tls_depth}"), threads),
                &threads,
                |b, &threads| {
                    let pool = ZeroPool::new().tls_cache_size(tls_depth).min_buffer_size(0);
                    b.iter(|| {
                        thread::scope(|s| {
                            for _ in 0..threads {
                                let pool = &pool;
                                s.spawn(move || {
                                    for _ in 0..ops {
                                        let buf = pool.alloc(size);
                                        black_box(&buf);
                                        drop(buf);
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

// ── stats_overhead ────────────────────────────────────────────────────
//
// Quantify the cost of `.track_stats(true)`.

fn stats_overhead(c: &mut Criterion) {
    let mut group = c.benchmark_group("stats_overhead");

    // Hot path, single-thread.
    {
        let size = 64 * 1024;
        group.bench_function("st_hot_off", |b| {
            let pool = common::competitors::zeropool_default();
            b.iter(|| {
                let buf = pool.alloc(size);
                black_box(&buf);
                drop(buf);
            });
        });
        group.bench_function("st_hot_on", |b| {
            let pool = common::competitors::zeropool_with_stats();
            b.iter(|| {
                let buf = pool.alloc(size);
                black_box(&buf);
                drop(buf);
            });
        });
    }

    // Realistic write, 8 threads.
    {
        let size = 64 * 1024;
        let ops = 200;
        let threads = 8;

        group.bench_function("mt8_write_off", |b| {
            let pool = common::competitors::zeropool_no_min();
            b.iter(|| {
                thread::scope(|s| {
                    for _ in 0..threads {
                        let pool = &pool;
                        s.spawn(move || {
                            for _ in 0..ops {
                                let mut buf = pool.alloc(size);
                                touch_pages(&mut buf);
                                drop(buf);
                            }
                        });
                    }
                });
            });
        });

        group.bench_function("mt8_write_on", |b| {
            let pool = common::competitors::zeropool_no_min().track_stats(true);
            b.iter(|| {
                thread::scope(|s| {
                    for _ in 0..threads {
                        let pool = &pool;
                        s.spawn(move || {
                            for _ in 0..ops {
                                let mut buf = pool.alloc(size);
                                touch_pages(&mut buf);
                                drop(buf);
                            }
                        });
                    }
                });
            });
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    hot_path,
    realistic,
    full_overwrite,
    scale_sustained,
    mixed_sizes,
    burst,
    contention,
    stats_overhead
);
criterion_main!(benches);
