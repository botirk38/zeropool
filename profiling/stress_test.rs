/// Stress Test Profiling Program
///
/// High-stress workload that combines multiple challenging patterns:
/// - Burst allocations (sudden spikes in demand)
/// - Cache thrashing (alternating sizes to stress eviction)
/// - Multi-threaded contention under heavy load
/// - Mixed allocation patterns testing edge cases
/// - Eviction policy stress testing
///
/// Usage: cargo flamegraph --profile profiling --bin stress_test

use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use zeropool::BufferPool;

// Configuration - increased for better profiling visibility
const NUM_STRESS_THREADS: usize = 12;
const STRESS_DURATION_SECS: u64 = 30;
const BURST_SIZE: usize = 200;
const THRASH_ITERATIONS: usize = 50000;  // Increased from 5000

// Size patterns for different stress scenarios
const SMALL_SIZES: &[usize] = &[64, 128, 256, 512];
const MEDIUM_SIZES: &[usize] = &[4096, 8192, 16384, 32768];
const LARGE_SIZES: &[usize] = &[65536, 131072, 262144, 524288];
const HUGE_SIZES: &[usize] = &[1048576, 2097152, 4194304];

#[derive(Clone, Copy)]
enum StressPhase {
    BurstAllocations,
    CacheThrashing,
    MixedSizes,
    EvictionPressure,
    RandomChaos,
}

impl StressPhase {
    fn name(&self) -> &'static str {
        match self {
            StressPhase::BurstAllocations => "Burst Allocations",
            StressPhase::CacheThrashing => "Cache Thrashing",
            StressPhase::MixedSizes => "Mixed Sizes",
            StressPhase::EvictionPressure => "Eviction Pressure",
            StressPhase::RandomChaos => "Random Chaos",
        }
    }

    fn all() -> &'static [StressPhase] {
        &[
            StressPhase::BurstAllocations,
            StressPhase::CacheThrashing,
            StressPhase::MixedSizes,
            StressPhase::EvictionPressure,
            StressPhase::RandomChaos,
        ]
    }
}

fn main() {
    eprintln!("=== Stress Test Profiling ===");
    eprintln!("Stress threads: {}", NUM_STRESS_THREADS);
    eprintln!("Duration: {}s", STRESS_DURATION_SECS);
    eprintln!("Phases: {}", StressPhase::all().len());
    eprintln!();

    // Create pool with aggressive settings for stress testing
    let pool = Arc::new(
        BufferPool::builder()
            .num_shards(16)
            .tls_cache_size(8)
            .max_buffers_per_shard(64)
            .min_buffer_size(64)
            .build()
    );

    let total_start = Instant::now();

    // Run through all stress phases
    for phase in StressPhase::all() {
        eprintln!("\n=== Phase: {} ===", phase.name());
        let phase_start = Instant::now();

        run_stress_phase(&pool, *phase);

        let phase_duration = phase_start.elapsed();
        eprintln!("[{}] Completed in {:.2?}", phase.name(), phase_duration);
        eprintln!("Pool stats: {} buffers", pool.len());
    }

    let total_duration = total_start.elapsed();
    eprintln!("\n=== Stress Test Complete ===");
    eprintln!("Total time: {:.2?}", total_duration);
    eprintln!("Final pool size: {} buffers", pool.len());
}

/// Run a specific stress phase
fn run_stress_phase(pool: &Arc<BufferPool>, phase: StressPhase) {
    let mut handles = Vec::new();

    // Spawn stress threads
    for thread_id in 0..NUM_STRESS_THREADS {
        let pool = Arc::clone(pool);
        let handle = thread::spawn(move || {
            stress_worker(thread_id, pool, phase)
        });
        handles.push(handle);
    }

    // Wait for all threads
    for handle in handles {
        handle.join().unwrap();
    }
}

/// Worker thread running stress patterns
fn stress_worker(thread_id: usize, pool: Arc<BufferPool>, phase: StressPhase) {
    match phase {
        StressPhase::BurstAllocations => burst_allocations(&pool, thread_id),
        StressPhase::CacheThrashing => cache_thrashing(&pool, thread_id),
        StressPhase::MixedSizes => mixed_sizes(&pool, thread_id),
        StressPhase::EvictionPressure => eviction_pressure(&pool, thread_id),
        StressPhase::RandomChaos => random_chaos(&pool, thread_id),
    }
}

/// Burst allocations: Sudden spikes in allocation demand
fn burst_allocations(pool: &BufferPool, _thread_id: usize) {
    for burst_round in 0..10 {
        // Sudden burst of allocations
        let mut buffers = Vec::new();
        for _ in 0..BURST_SIZE {
            let size = MEDIUM_SIZES[burst_round % MEDIUM_SIZES.len()];
            buffers.push(pool.get(size));
        }

        // Do work with all buffers
        for buffer in &mut buffers {
            process_buffer(buffer);
        }

        // All buffers returned when dropped
        drop(buffers);

        // Brief pause before next burst
        thread::sleep(Duration::from_millis(10));
    }
}

/// Cache thrashing: Alternating sizes to stress TLS cache and eviction
fn cache_thrashing(pool: &BufferPool, thread_id: usize) {
    let pattern = if thread_id % 2 == 0 {
        &[4096, 65536, 4096, 65536] // Small-large-small-large
    } else {
        &[16384, 262144, 16384, 262144] // Medium-huge-medium-huge
    };

    for _ in 0..THRASH_ITERATIONS {
        for &size in pattern {
            let mut buffer = pool.get(size);
            process_buffer(&mut buffer);
            // Buffer immediately returned, causing thrashing
        }
    }
}

/// Mixed sizes: Random mix of all size categories
fn mixed_sizes(pool: &BufferPool, thread_id: usize) {
    let seed = thread_id;

    for i in 0..THRASH_ITERATIONS {
        let category = (seed + i) % 4;
        let size = match category {
            0 => SMALL_SIZES[(seed * 7 + i) % SMALL_SIZES.len()],
            1 => MEDIUM_SIZES[(seed * 11 + i) % MEDIUM_SIZES.len()],
            2 => LARGE_SIZES[(seed * 13 + i) % LARGE_SIZES.len()],
            _ => HUGE_SIZES[(seed * 17 + i) % HUGE_SIZES.len()],
        };

        let mut buffer = pool.get(size);
        process_buffer(&mut buffer);
    }
}

/// Eviction pressure: Exceed max_buffers_per_shard to trigger evictions
fn eviction_pressure(pool: &BufferPool, _thread_id: usize) {
    // Create massive pressure by allocating many large buffers
    for round in 0..20 {
        let mut buffers = Vec::new();

        // Allocate way more than max_buffers_per_shard
        for i in 0..100 {
            let size = LARGE_SIZES[i % LARGE_SIZES.len()];
            buffers.push(pool.get(size));
        }

        // Process all buffers
        for buffer in &mut buffers {
            process_buffer(buffer);
        }

        // Drop half, keep half temporarily
        buffers.truncate(50);

        if round % 5 == 0 {
            // Occasionally create huge memory pressure
            for _ in 0..10 {
                let mut huge = pool.get(HUGE_SIZES[0]);
                process_buffer(&mut huge);
            }
        }

        // All remaining buffers returned
        drop(buffers);
    }
}

/// Random chaos: Completely unpredictable access pattern
fn random_chaos(pool: &BufferPool, thread_id: usize) {
    let mut pseudo_random = thread_id.wrapping_mul(1103515245).wrapping_add(12345);

    for _ in 0..THRASH_ITERATIONS {
        // Generate pseudo-random size
        pseudo_random = pseudo_random.wrapping_mul(1103515245).wrapping_add(12345);
        let size_category = (pseudo_random >> 16) % 20;

        let size = match size_category {
            0..=7 => SMALL_SIZES[(pseudo_random >> 8) as usize % SMALL_SIZES.len()],
            8..=14 => MEDIUM_SIZES[(pseudo_random >> 8) as usize % MEDIUM_SIZES.len()],
            15..=18 => LARGE_SIZES[(pseudo_random >> 8) as usize % LARGE_SIZES.len()],
            _ => HUGE_SIZES[(pseudo_random >> 8) as usize % HUGE_SIZES.len()],
        };

        let mut buffer = pool.get(size);
        process_buffer(&mut buffer);

        // Randomly hold onto some buffers
        if pseudo_random % 10 == 0 {
            let mut held_buffers = Vec::new();
            for _ in 0..5 {
                held_buffers.push(pool.get(size / 2));
            }
            // Do some work
            for buf in &mut held_buffers {
                process_buffer(buf);
            }
            // Drop them
        }
    }
}

/// Process buffer with minimal computation
fn process_buffer(buffer: &mut [u8]) {
    // Minimal work - just touch the buffer to prevent optimization
    // This focuses profiling on zeropool allocation/deallocation
    std::hint::black_box(buffer.len());
    if !buffer.is_empty() {
        buffer[0] = 1;
        if buffer.len() > 1 {
            buffer[buffer.len() - 1] = 1;
        }
    }
}
