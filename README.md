# ZeroPool

A high-performance, zero-overhead buffer pool for Rust.

[![Crates.io](https://img.shields.io/crates/v/zeropool.svg)](https://crates.io/crates/zeropool)
[![Documentation](https://docs.rs/zeropool/badge.svg)](https://docs.rs/zeropool)
[![License](https://img.shields.io/crates/l/zeropool.svg)](https://github.com/botirk38/zeropool/blob/main/LICENSE)

## Features

- **70% faster** than no pooling in I/O benchmarks (176ms → 52ms for 500MB files)
- **System-aware** - Automatically adapts to CPU count and hardware topology
- **Lock-free** fast path via thread-local storage with configurable cache size
- **Sharded pool** - Reduces contention in multi-threaded workloads
- **Zero-copy** buffer reuse with no unnecessary allocations
- **Smart allocation** - O(1) first-fit algorithm
- **Configurable** - Fine-tune cache size, shard count, and memory limits
- **Simple API** - just `get()` and `put()`

## Quick Start

```rust
use zeropool::BufferPool;

// Smart defaults based on system CPU count
let pool = BufferPool::new();

// Get a buffer from the pool
let mut buffer = pool.get(1024 * 1024); // 1MB buffer

// Use it for I/O operations
file.read(&mut buffer)?;

// Return it to the pool for reuse
pool.put(buffer);
```

## Performance

In benchmarks with 500MB tensor files:

| Method | Time | Speedup |
|--------|------|---------|
| No pool | 176ms | 1.0x |
| **ZeroPool** | **52ms** | **3.36x** |

The pool achieves this through:
1. **Thread-local caching** - Lock-free fast path with 4 buffers per thread by default
2. **Sharded global pool** - 32 independent shards on 20-core system (power-of-2 based on CPU count)
3. **Zero-fill avoidance** - Safe `unsafe` with capacity-checked `set_len()`
4. **First-fit allocation** - O(1) vs O(n) best-fit
5. **Round-robin shard selection** - Fast, no hashing overhead

## Advanced Usage

### Custom Configuration

```rust
use zeropool::{BufferPool, PoolConfig};

// Fine-tune pool behavior
let config = PoolConfig::default()
    .with_tls_cache_size(8)              // 8 buffers per thread (default: 4)
    .with_min_buffer_size(512 * 1024)    // Keep buffers >= 512KB (default: 1MB)
    .with_max_buffers_per_shard(32)      // Up to 32 buffers per shard (default: 16)
    .with_num_shards(64);                // Override CPU-based default

let pool = BufferPool::with_config(config);
```

**System-aware defaults:**
- `num_shards`: Next power of 2 ≥ CPU count, clamped to [4, 64]
  - 4-core system: 4 shards
  - 20-core system: 32 shards
  - 64+ core system: 64 shards (max)
- `tls_cache_size`: 4 buffers per thread
- `max_buffers_per_shard`: 16 buffers
- `min_buffer_size`: 1MB

### Pre-warming the Pool

```rust
let pool = BufferPool::new();

// Pre-allocate 10 buffers of 10MB each
pool.preallocate(10, 10 * 1024 * 1024);
```

### Thread Safety

`BufferPool` is `Clone` and can be shared across threads. Each thread maintains its own cache for maximum performance:

```rust
use std::sync::Arc;

let pool = Arc::new(BufferPool::new());

// Share across threads - each gets its own TLS cache
for _ in 0..4 {
    let pool = Arc::clone(&pool);
    std::thread::spawn(move || {
        let buf = pool.get(1024);
        // ... use buffer ...
        pool.put(buf);
    });
}
```

## How It Works

ZeroPool uses a three-tier architecture optimized for both single-threaded and multi-threaded workloads:

```text
Thread 1            Thread 2            Thread N
┌──────────┐       ┌──────────┐       ┌──────────┐
│TLS Cache │       │TLS Cache │       │TLS Cache │
│(4 bufs)  │       │(4 bufs)  │       │(4 bufs)  │
└────┬─────┘       └────┬─────┘       └────┬─────┘
     │                  │                  │
     └──────────┬───────┴──────────────────┘
                │
        ┌───────▼────────┐
        │  Shared Pool   │
        │  (32 shards)   │  Power-of-2 based on CPU count
        │                │  (4-64 shards, adaptive)
        │ [Shard 0] ...  │
        │ [Shard 1]      │
        │ [Shard 2]      │
        │    ...         │
        └────────────────┘
```

**When you call `get()`:**
1. Check thread-local cache (lock-free, ~1ns)
2. If miss, try assigned shard (single lock, round-robin)
3. If miss, allocate new buffer

**When you call `put()`:**
1. If TLS cache has space, store there (lock-free)
2. Otherwise, return to shard pool (single lock)
3. If shard is full or buffer too small, discard

**Key optimizations:**
- Power-of-2 shard count enables `x & (n-1)` instead of `x % n` (faster modulo)
- Round-robin shard selection avoids thread ID hashing overhead
- TLS cache reduces shared pool access by ~75% in single-threaded scenarios
- Sharding reduces lock contention by 32x in multi-threaded scenarios

## Safety

ZeroPool uses `unsafe` code internally to avoid zero-filling buffers on reuse via `set_len()`. This is safe because:

- Capacity checks guarantee sufficient space before setting length
- Buffers are immediately overwritten by I/O operations
- No reads occur before writes in typical I/O patterns

All unsafe code is carefully documented and audited.

## Benchmarks

### Test System Specifications

- **CPU**: Intel Core i9-10900K @ 3.70GHz (10 cores, 20 threads)
  - Max Turbo: 5.3 GHz
  - L1 Cache: 320 KiB (10 instances)
  - L2 Cache: 2.5 MiB (10 instances)
  - L3 Cache: 20 MiB
- **RAM**: 32 GB DDR4
- **OS**: Linux 6.14.0-33-generic
- **Rust**: 1.84+ (bench profile, optimized)

### Comparison with Other Memory Pool Libraries

ZeroPool is benchmarked against:
- **No pooling** (baseline `Vec::with_capacity`)
- **mempool** - Fast thread-safe pool optimized for single-threaded use
- **sharded-slab** - Lock-free concurrent slab allocator
- **lifeguard** - Object pool manager (single-threaded benchmarks only, not thread-safe)
- **object-pool** - Simple pull-based object pool
- **bytes** - Popular buffer management library from Tokio ecosystem

### Single-Threaded Performance

Allocation and deallocation latency (lower is better):

| Buffer Size | ZeroPool | No Pool | Lifeguard | Sharded-Slab | Object-Pool | Mempool |
|------------|----------|---------|-----------|--------------|-------------|---------|
| 1 KB       | **8.4 ns** | 7.2 ns | 7.0 ns | 49.8 ns | 19.2 ns | **0.4 ps** |
| 4 KB       | **8.3 ns** | 43.8 ns | 6.6 ns | 83.7 ns | 19.1 ns | **0.4 ps** |
| 16 KB      | **8.3 ns** | 44.0 ns | 6.5 ns | 83.1 ns | 19.2 ns | **0.4 ps** |
| 64 KB      | **8.4 ns** | 43.6 ns | 6.5 ns | 84.1 ns | 19.1 ns | **0.4 ps** |
| 1 MB       | **8.3 ns** | 35.3 ns | 6.5 ns | 76.3 ns | 19.1 ns | **0.4 ps** |

**Throughput for 1 MB buffers:**
- **ZeroPool**: 116,000 GiB/s
- **Lifeguard**: 150,000 GiB/s
- **Mempool**: 2,250,000 GiB/s (theoretical; extremely low-level)
- **No Pool**: 27,000 GiB/s
- **Bytes**: 22,000 GiB/s

**Key Insights:**
- **Size independence**: ZeroPool maintains constant ~8.3ns latency regardless of buffer size (1KB to 1MB)
- **Predictable performance**: Zero variance across different allocation sizes
- **Competitive with specialized pools**: Only 28% slower than lifeguard, but thread-safe
- **Trade-off**: Slightly slower than no-pool for tiny buffers, but 76% faster for large buffers

### Multi-Threaded Performance

Throughput under concurrent load (higher is better):

| Threads | ZeroPool | No Pool | Sharded-Slab | Object-Pool | Mempool |
|---------|----------|---------|--------------|-------------|---------|
| 2       | **10.3 TiB/s** | 2.8 TiB/s | 1.4 TiB/s | 2.0 TiB/s | 4.8 TiB/s |
| 4       | **18.7 TiB/s** | 5.4 TiB/s | 2.7 TiB/s | 1.3 TiB/s | 1.4 TiB/s |
| 8       | **28.1 TiB/s** | 9.3 TiB/s | 4.8 TiB/s | 0.5 TiB/s | 0.8 TiB/s |

**Analysis:**
- **Near-linear scaling**: 3x throughput improvement from 2→8 threads
- **Best multi-threaded performance**: 3-5x faster than alternatives under high contention
- **Low contention overhead**: TLS caching reduces shared pool access by ~75%

### Real-World Allocation Patterns

**Varied sizes** (alternating 1KB, 4KB, 16KB buffers):
- ZeroPool: **36.2 ns/allocation**
- Lifeguard: 34.1 ns/allocation
- No Pool: 148.9 ns/allocation

**Buffer reuse** (single 1MB buffer, get/put 1000x):
- ZeroPool: **2.8 µs total** (2.8 ns/op)
- Lifeguard: 157 ns/op
- Mempool: 97 ns/op
- No Pool: 542 ns/op

**Comparison with `bytes::BytesMut`:**
- Small (1KB): bytes is 11% faster (7.5ns vs 8.4ns)
- Medium (64KB): ZeroPool is **5.2x faster** (8.4ns vs 43.5ns)
- Large (1MB): ZeroPool is **5.3x faster** (8.4ns vs 44.5ns)

### Run Benchmarks Yourself

```bash
cd zeropool
cargo bench
```

The benchmark suite includes:
- Single-threaded allocation/deallocation across various buffer sizes (1KB - 1MB)
- Multi-threaded workloads (2, 4, 8 threads) - excludes lifeguard (not thread-safe)
- Real-world allocation patterns (varied sizes, reuse patterns)
- Comparison with `bytes::BytesMut`

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.
