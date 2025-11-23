//! Allocation patterns benchmarks
#![allow(missing_docs)]
use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;

/// Benchmark allocation patterns
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
                drop(buf);
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
                drop(buf);
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

/// Benchmark burst allocations
fn benchmark_burst_allocations(c: &mut Criterion) {
    let mut group = c.benchmark_group("burst_allocations");
    let size = 64 * 1024;

    group.bench_function("zeropool_burst_100", |b| {
        let pool = zeropool::BufferPool::new();
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

    group.bench_function("no_pool_burst_100", |b| {
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

    group.bench_function("lifeguard_burst_100", |b| {
        use lifeguard::Pool;
        let pool: Pool<Vec<u8>> = Pool::with_size(100);
        b.iter(|| {
            let mut bufs = vec![];
            for _ in 0..100 {
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

/// Benchmark zipf distribution
fn benchmark_zipf_distribution(c: &mut Criterion) {
    let mut group = c.benchmark_group("zipf_distribution");
    // Simulate realistic workload: small buffers are much more common
    let sizes = [
        (1024, 50),   // 50% of requests
        (4096, 25),   // 25% of requests
        (16384, 15),  // 15% of requests
        (65536, 7),   // 7% of requests
        (262_144, 3), // 3% of requests
    ];

    group.bench_function("zeropool_zipf", |b| {
        let pool = zeropool::BufferPool::new();
        b.iter(|| {
            for &(size, count) in &sizes {
                for _ in 0..count {
                    let buf = pool.get(size);
                    black_box(&buf);
                    drop(buf);
                }
            }
        });
    });

    group.bench_function("no_pool_zipf", |b| {
        b.iter(|| {
            for &(size, count) in &sizes {
                for _ in 0..count {
                    let buf = Vec::<u8>::with_capacity(size);
                    black_box(&buf);
                }
            }
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    benchmark_allocation_patterns,
    benchmark_burst_allocations,
    benchmark_zipf_distribution
);
criterion_main!(benches);
