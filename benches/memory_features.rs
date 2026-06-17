//! Memory features benchmarks
#![allow(missing_docs)]
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use std::hint::black_box;

/// Benchmark pinned memory
fn benchmark_pinned_memory(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory_features/pinned");
    let size = 1024 * 1024; // 1MB buffers

    group.bench_function("unpinned", |b| {
        let pool = zeropool::BufferPool::builder().pinned_memory(false).build();
        b.iter(|| {
            let mut buf = pool.get(size);
            black_box(&mut buf);
            drop(buf);
        });
    });

    group.bench_function("pinned", |b| {
        let pool = zeropool::BufferPool::builder().pinned_memory(true).build();
        b.iter(|| {
            let mut buf = pool.get(size);
            black_box(&mut buf);
            drop(buf);
        });
    });

    group.bench_function("pinned_preallocated", |b| {
        let pool = zeropool::BufferPool::builder().pinned_memory(true).build();
        pool.preallocate(10, size);
        b.iter(|| {
            let mut buf = pool.get(size);
            black_box(&mut buf);
            drop(buf);
        });
    });

    group.finish();
}

/// Benchmark preallocation effectiveness
fn benchmark_preallocation_effectiveness(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory_features/preallocation");

    for prealloc_count in [0, 10, 50, 100] {
        group.bench_with_input(
            BenchmarkId::new("zeropool", prealloc_count),
            &prealloc_count,
            |b, &prealloc_count| {
                let pool = zeropool::BufferPool::new();
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

/// Benchmark memory pressure
fn benchmark_memory_pressure(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory_features/pressure");

    for max_buffers in [5, 10, 20] {
        group.bench_with_input(
            BenchmarkId::new("zeropool", max_buffers),
            &max_buffers,
            |b, &max_buffers| {
                let pool =
                    zeropool::BufferPool::builder().max_buffers_per_class(max_buffers).build();

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

/// Benchmark size-class scaling
fn benchmark_size_class_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory_features/size_class_scaling");

    for num_classes in [1, 3, 5, 8] {
        group.bench_with_input(
            BenchmarkId::new("zeropool", num_classes),
            &num_classes,
            |b, &num_classes| {
                let pool = zeropool::BufferPool::new();
                // Use different number of size classes simultaneously
                let sizes: Vec<usize> =
                    [4096, 16384, 65536, 262_144, 1_048_576, 4_194_304, 16_777_216, 67_108_864]
                        .iter()
                        .take(num_classes)
                        .copied()
                        .collect();

                b.iter(|| {
                    let mut bufs = vec![];
                    for &size in &sizes {
                        bufs.push(pool.get(size));
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

/// Benchmark min buffer size filtering
fn benchmark_min_buffer_size_filtering(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory_features/min_size");

    for min_size in [0, 1024, 64 * 1024] {
        group.bench_with_input(
            BenchmarkId::new("zeropool", min_size),
            &min_size,
            |b, &min_size| {
                let pool = zeropool::BufferPool::builder().min_buffer_size(min_size).build();

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

criterion_group!(
    benches,
    benchmark_pinned_memory,
    benchmark_preallocation_effectiveness,
    benchmark_memory_pressure,
    benchmark_size_class_scaling,
    benchmark_min_buffer_size_filtering
);
criterion_main!(benches);
