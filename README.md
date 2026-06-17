# ZeroPool

> A high-performance buffer pool for Rust — Performance First

[![Crates.io](https://img.shields.io/crates/v/zeropool.svg)](https://crates.io/crates/zeropool)
[![Documentation](https://docs.rs/zeropool/badge.svg)](https://docs.rs/zeropool)
[![License](https://img.shields.io/crates/l/zeropool.svg)](https://github.com/botirk38/zeropool/blob/main/LICENSE)
[![CI](https://github.com/botirk38/zeropool/workflows/CI/badge.svg)](https://github.com/botirk38/zeropool/actions)

## Why ZeroPool?

ZeroPool is a **high-performance buffer pool** that prioritizes speed above all else:

- **Lock-free architecture**: `crossbeam::ArrayQueue` per size class — no mutexes
- **Size-class bucketing**: 8 power-of-two classes (4KB→64MB) for O(1) class selection
- **Thread-local caching**: Per-class LIFO caches with magazine-style batch transfer
- **Pool isolation**: Unique pool IDs prevent TLS cache cross-contamination
- **Auto-configured**: Adapts to your CPU topology for optimal multi-threaded performance

## Quick Start

```rust
use zeropool::BufferPool;

let pool = BufferPool::new();

// Get a buffer (returns RAII guard)
let mut buffer = pool.get(1024 * 1024); // 1MB

// Use it — Deref<Target = [u8]> for safe slice access
buffer[0] = 42;

// Buffer automatically returned to pool when dropped
```

## Architecture

```
Thread 1            Thread 2            Thread N
┌────────────┐     ┌────────────┐     ┌────────────┐
│ TLS Cache  │     │ TLS Cache  │     │ TLS Cache  │  ← Lock-free
│ [class 0]  │     │ [class 0]  │     │ [class 0]  │    per-class
│ [class 1]  │     │ [class 1]  │     │ [class 1]  │    LIFO caches
│   ...      │     │   ...      │     │   ...      │
└─────┬──────┘     └─────┬──────┘     └─────┬──────┘
      │ batch             │ batch             │ batch
      └──────────┬───────┴───────────────────┘
                 │
         ┌───────▼────────┐
         │  Shared Pool   │
         │ (lock-free)    │
         │                │
         │ [4KB  queue]   │  ArrayQueue per class
         │ [16KB queue]   │  CAS-based push/pop
         │ [64KB queue]   │  No mutex needed
         │ [256KB queue]  │
         │ [1MB  queue]   │
         │ [4MB  queue]   │
         │ [16MB queue]   │
         │ [64MB queue]   │
         └────────────────┘
```

**Fast path**: TLS cache pop (lock-free, ~24ns)
**Medium path**: Magazine-style batch refill from shared pool (CAS-based)
**Cold path**: Fresh allocation via `SizeClass::allocate`

## Configuration

```rust
use zeropool::BufferPool;

let pool = BufferPool::new()
    .tls_cache_size(8)               // Buffers per class per thread
    .max_buffers_per_class(64)       // Max pooled per class in shared pool
    .min_buffer_size(4096)           // Pool buffers ≥ 4KB
    .batch_size(4);                  // Magazine transfer size
```

**Defaults** (auto-configured based on CPU count):
- TLS cache: 2–8 buffers per class per thread
- Max per class: 32–128 buffers
- Min buffer size: 4KB
- Batch size: half of TLS cache (min 2)

### Memory Pinning

Lock buffer memory in RAM to prevent swapping (performance optimization):

```rust
use zeropool::BufferPool;

let pool = BufferPool::new().pinned_memory(true);
```

Useful for high-performance computing or real-time systems. May require elevated privileges on some systems. Falls back gracefully if pinning fails.

### Pre-allocation

Warm up the pool before high-throughput operations:

```rust
let pool = BufferPool::new().min_buffer_size(0);
pool.preallocate(16, 64 * 1024); // 16 × 64KB buffers
```

## Statistics

`pool.stats()` returns a point-in-time snapshot of pool performance:

```rust
use zeropool::BufferPool;

let pool = BufferPool::new().min_buffer_size(0);

let buf = pool.get(4096);
drop(buf);

let s = pool.stats();
println!("gets: {}, hits: {}, hit_rate: {:.0}%", s.gets, s.tls_hits + s.shared_hits, s.hit_rate * 100.0);

// Human-readable summary
println!("{s}");
// gets: 1 | puts: 1 | hit_rate: 0.0%
//   tls_hits: 0 (0.0%) | shared_hits: 0 | allocations: 1 | discards: 0 | oversize: 0

// Per-class buffer counts
for ci in &s.classes {
    if ci.buffered > 0 {
        println!("  {} bytes: {} buffered", ci.boundary, ci.buffered);
    }
}

// Reset counters
pool.reset_stats();
```

Metrics use `Relaxed` atomics — negligible overhead on the hot path (benchmarks show no measurable regression).

## Thread Safety

`BufferPool` is `Clone` and thread-safe (`Arc<PoolState>` internally):

```rust
let pool = BufferPool::new();

for _ in 0..4 {
    let pool = pool.clone();
    std::thread::spawn(move || {
        let buf = pool.get(1024);
        // Each thread gets its own TLS cache
        // Buffer automatically returned when dropped
    });
}
```

## Ownership and Pool Return

When a `PooledBuffer` is dropped, the buffer returns to the pool. Use `into_inner()` to extract the `Vec<u8>` without returning it:

```rust
let pool = BufferPool::new();

// Normal: returns to pool on drop
{
    let buffer = pool.get(1024);
}

// Extract ownership — does NOT return to pool
let buffer = pool.get(1024);
let vec: Vec<u8> = buffer.into_inner();
```

## Benchmarks

Single-threaded hot path (TLS hit, 2-core VM):

| Buffer Size | Latency |
|------------|---------|
| 1KB | ~24 ns |
| 4KB | ~24 ns |
| 16KB | ~24 ns |
| 64KB | ~25 ns |
| 1MB | ~25 ns |

Run benchmarks:
```bash
cargo bench                           # all benchmarks
cargo bench -- single_thread          # hot path only
cargo bench -- multi_thread           # contention tests
cargo bench -- cache_behavior         # TLS cache behavior
```

## Use Cases

- **Data processing**: ETL pipelines, log processing, analytics
- **Network servers**: HTTP, gRPC, WebSocket servers with high throughput
- **File I/O**: Async file loading with io_uring, tokio, async-std
- **LLM inference**: Fast checkpoint loading and model serving
- **Real-time systems**: Low-latency buffer management
- **Big data**: High-throughput data streaming and processing

## Comparison with Alternatives

| Feature | ZeroPool | bytes::BytesMut | Lifeguard | Sharded-Slab |
|---------|----------|-----------------|-----------|--------------|
| Lock-free pool | ArrayQueue (CAS) | No | No | Partial |
| Size classes | 8 power-of-two | No | No | No |
| TLS caching | Per-class LIFO | No | No | No |
| Batch transfer | Magazine-style | No | No | No |
| Auto-configured | CPU-aware | Manual | Manual | Manual |
| Pool isolation | Unique IDs | N/A | No | N/A |
| Statistics | `pool.stats()` | No | No | No |

## License

Dual licensed under Apache-2.0 or MIT.

## Contributing

PRs welcome! Please include benchmarks for performance changes and ensure all tests pass:

```bash
cargo test
cargo bench
cargo fmt
cargo clippy
```

## Changelog

See [CHANGELOG.md](CHANGELOG.md) for version history.

## Credits

Built for the Rust community. Inspired by TCMalloc size-class design, Bonwick magazine-layer research, and modern lock-free allocator techniques.
