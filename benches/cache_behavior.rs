//! Cache behavior benchmarks
#![allow(missing_docs)]
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use zeropool::BufferPool;

/// Benchmark ping pong pattern
fn benchmark_ping_pong_pattern(c: &mut Criterion) {
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

/// Benchmark hot cold buffers
fn benchmark_hot_cold_buffers(c: &mut Criterion) {
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

/// Benchmark multi size workload
fn benchmark_multi_size_workload(c: &mut Criterion) {
    let mut group = c.benchmark_group("cache_behavior/multi_size");

    group.bench_function("lifo", |b| {
        let pool = BufferPool::builder().tls_cache_size(15).build();
        let sizes = [4 * 1024, 64 * 1024, 1024 * 1024];

        b.iter(|| {
            for _ in 0..100 {
                for &size in &sizes {
                    let buf = pool.get(size);
                    black_box(&buf);
                    drop(buf);
                }
            }
        });
    });

    group.finish();
}

/// Benchmark TLS cache effectiveness
fn benchmark_tls_cache_effectiveness(c: &mut Criterion) {
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

/// Benchmark eviction pressure
fn benchmark_eviction_pressure(c: &mut Criterion) {
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

/// Benchmark atomic counter contention
fn benchmark_atomic_counter_contention(c: &mut Criterion) {
    let mut group = c.benchmark_group("cache_behavior/atomic_contention");

    for num_threads in [1, 4, 8, 16] {
        group.bench_with_input(
            BenchmarkId::new("threads", num_threads),
            &num_threads,
            |b, &num_threads| {
                let pool = BufferPool::builder().tls_cache_size(1).min_buffer_size(0).build();

                b.iter(|| {
                    let pool = &pool;
                    std::thread::scope(|s| {
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

criterion_group!(
    benches,
    benchmark_ping_pong_pattern,
    benchmark_hot_cold_buffers,
    benchmark_multi_size_workload,
    benchmark_tls_cache_effectiveness,
    benchmark_atomic_counter_contention,
    benchmark_eviction_pressure
);
criterion_main!(benches);
