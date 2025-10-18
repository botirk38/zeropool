# ZeroPool

A high-performance, zero-overhead buffer pool for Rust.

[![Crates.io](https://img.shields.io/crates/v/zeropool.svg)](https://crates.io/crates/zeropool)
[![Documentation](https://docs.rs/zeropool/badge.svg)](https://docs.rs/zeropool)
[![License](https://img.shields.io/crates/l/zeropool.svg)](https://github.com/botir-khaltaev/PROJECT/blob/main/LICENSE)

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

Run benchmarks yourself:

```bash
cd zeropool
cargo bench
```

### Comparison with Other Memory Pool Libraries

ZeroPool is benchmarked against:
- **No pooling** (baseline `Vec::with_capacity`)
- **mempool** - Fast thread-safe pool optimized for single-threaded use
- **sharded-slab** - Lock-free concurrent slab allocator
- **lifeguard** - Object pool manager (single-threaded benchmarks only, not thread-safe)
- **object-pool** - Simple pull-based object pool
- **bytes** - Popular buffer management library from Tokio ecosystem

Benchmarks include:
- Single-threaded allocation/deallocation across various buffer sizes (1KB - 1MB)
- Multi-threaded workloads (2, 4, 8 threads) - excludes lifeguard (not thread-safe)
- Real-world allocation patterns (varied sizes, reuse patterns)
- Comparison with `bytes::BytesMut`

Results show ZeroPool's thread-local caching provides superior performance for single-threaded I/O workloads while maintaining competitive multi-threaded performance.

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.
