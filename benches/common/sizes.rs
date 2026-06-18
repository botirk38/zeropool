//! Shared size and thread-count constants.

/// Standard buffer sizes used across most benchmarks.
pub const SIZES: &[usize] = &[
    4 * 1024,
    16 * 1024,
    64 * 1024,
    256 * 1024,
    1024 * 1024,
    4 * 1024 * 1024,
];

/// Mixed-size workload sizes: covers a wide range with one pool.
pub const MIXED_SIZES: &[usize] = &[
    1024,
    4 * 1024,
    16 * 1024,
    64 * 1024,
    256 * 1024,
    1024 * 1024,
    4 * 1024 * 1024,
];

/// Default thread counts for multi-threaded benchmarks.
pub const THREAD_COUNTS: &[usize] = &[2, 4, 8, 16];

/// Wider thread sweep for sustained/long-running benchmarks.
pub const SCALE_THREAD_COUNTS: &[usize] = &[1, 4, 8, 16, 32];

/// Burst sizes for batch workloads.
pub const BURST_SIZES: &[usize] = &[10, 50, 100, 1_000];

/// Zipf-like distribution: (size, relative weight).
pub const ZIPF_SIZES: &[(usize, usize)] = &[
    (1024, 50),
    (4 * 1024, 25),
    (16 * 1024, 15),
    (64 * 1024, 7),
    (256 * 1024, 3),
];
