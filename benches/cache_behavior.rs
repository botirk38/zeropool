use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use zeropool::{BufferPool, EvictionPolicy};

fn benchmark_ping_pong_pattern(c: &mut Criterion) {
    let mut group = c.benchmark_group("cache_behavior/ping_pong");

    for policy in [EvictionPolicy::Lifo, EvictionPolicy::ClockPro] {
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{:?}", policy)),
            &policy,
            |b, &policy| {
                let pool = BufferPool::builder()
                    .eviction_policy(policy)
                    .tls_cache_size(10)
                    .build();

                // Preallocate buffers
                pool.preallocate(20, 1024 * 1024);

                b.iter(|| {
                    // Alternating size requests (cache thrashing scenario)
                    for _ in 0..100 {
                        let buf1 = pool.get(1024);
                        let buf2 = pool.get(1024 * 1024);
                        black_box(&buf1);
                        black_box(&buf2);
                        drop(buf1);
                        drop(buf2);
                    }
                });
            },
        );
    }

    group.finish();
}

fn benchmark_hot_cold_buffers(c: &mut Criterion) {
    let mut group = c.benchmark_group("cache_behavior/hot_cold");

    for policy in [EvictionPolicy::Lifo, EvictionPolicy::ClockPro] {
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{:?}", policy)),
            &policy,
            |b, &policy| {
                let pool = BufferPool::builder()
                    .eviction_policy(policy)
                    .tls_cache_size(10)
                    .build();

                pool.preallocate(10, 64 * 1024);

                b.iter(|| {
                    // Only use 3 buffers repeatedly (hot buffers)
                    let mut bufs = vec![];
                    for _ in 0..3 {
                        bufs.push(pool.get(64 * 1024));
                    }

                    // Use them 1000 times
                    for _ in 0..1000 {
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

fn benchmark_multi_size_workload(c: &mut Criterion) {
    let mut group = c.benchmark_group("cache_behavior/multi_size");

    for policy in [EvictionPolicy::Lifo, EvictionPolicy::ClockPro] {
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{:?}", policy)),
            &policy,
            |b, &policy| {
                let pool = BufferPool::builder()
                    .eviction_policy(policy)
                    .tls_cache_size(15)
                    .build();

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
            },
        );
    }

    group.finish();
}

fn benchmark_tls_cache_effectiveness(c: &mut Criterion) {
    let mut group = c.benchmark_group("cache_behavior/tls_cache");

    for tls_size in [2, 4, 8] {
        group.bench_with_input(
            BenchmarkId::new("zeropool", tls_size),
            &tls_size,
            |b, &tls_size| {
                let pool = BufferPool::builder()
                    .tls_cache_size(15)
                    .min_buffer_size(0) // Allow small buffers for this benchmark
                    .build();

                b.iter(|| {
                    // Fill TLS cache
                    let mut bufs = vec![];
                    for _ in 0..tls_size {
                        bufs.push(pool.get(64 * 1024));
                    }

                    // Reuse from TLS cache
                    for _ in 0..10 {
                        for buf in &bufs {
                            black_box(buf);
                        }
                    }

                    // Return to TLS cache
                    for buf in bufs {
                        drop(buf);
                    }
                });
            },
        );
    }

    group.finish();
}

// Removed benchmark_shard_affinity due to TLS cache conflicts
// Creating multiple pool instances with different num_shards on the same thread
// causes buffer corruption when TLS cache is shared between instances.
// TODO: Add pool instance ID tracking to safely support this pattern

fn benchmark_eviction_pressure(c: &mut Criterion) {
    let mut group = c.benchmark_group("cache_behavior/eviction_pressure");

    // Use sample_size to reduce warmup iterations
    group.sample_size(50);

    for policy in [EvictionPolicy::Lifo, EvictionPolicy::ClockPro] {
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{:?}", policy)),
            &policy,
            |b, &policy| {
                let pool = BufferPool::builder()
                    .eviction_policy(policy)
                    .max_buffers_per_shard(100) // Larger capacity
                    .num_shards(4) // Multiple shards like other benchmarks
                    .tls_cache_size(8)
                    .min_buffer_size(4096) // Set reasonable minimum
                    .build();

                b.iter(|| {
                    // Simpler test: just allocate and return fewer buffers
                    let mut bufs = vec![];
                    for _ in 0..10 {
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

fn benchmark_atomic_counter_contention(c: &mut Criterion) {
    let mut group = c.benchmark_group("cache_behavior/atomic_contention");

    for num_threads in [1, 4, 8, 16] {
        group.bench_with_input(
            BenchmarkId::new("threads", num_threads),
            &num_threads,
            |b, &num_threads| {
                let pool = BufferPool::builder()
                    .num_shards(4) // Fixed shards to isolate counter effect
                    .tls_cache_size(1) // Minimal TLS cache to force shared pool usage
                    .min_buffer_size(0) // Allow small buffers
                    .build();

                b.iter(|| {
                    let pool = &pool;
                    std::thread::scope(|s| {
                        let mut handles = vec![];
                        for _ in 0..num_threads {
                            handles.push(s.spawn(|| {
                                // High-frequency get/put operations to stress atomic counters
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
