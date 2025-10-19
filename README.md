# ZeroPool

> A high-performance buffer pool for Rust with constant-time allocations

[![Crates.io](https://img.shields.io/crates/v/zeropool.svg)](https://crates.io/crates/zeropool)
[![Documentation](https://docs.rs/zeropool/badge.svg)](https://docs.rs/zeropool)
[![License](https://img.shields.io/crates/l/zeropool.svg)](https://github.com/botirk38/zeropool/blob/main/LICENSE)
[![CI](https://github.com/botirk38/zeropool/workflows/CI/badge.svg)](https://github.com/botirk38/zeropool/actions)

## Why ZeroPool?

Loading GPT-2 checkpoints took **200ms**. Profiling showed 70% was buffer allocation, not I/O. With ZeroPool: **53ms** (3.8x faster).

ZeroPool provides constant ~8ns allocation time regardless of buffer size, thread-safe operations, and near-linear multi-threaded scaling.

## Quick Start

```rust
use zeropool::BufferPool;

let pool = BufferPool::new();

// Get a buffer
let mut buffer = pool.get(1024 * 1024); // 1MB

// Use it
file.read(&mut buffer)?;

// Return it
pool.put(buffer);
```

## Performance Highlights

| Metric | Result |
|--------|--------|
| Allocation latency | **8.3ns** (constant, any size) |
| vs bytes (64KB+) | **5x faster** |
| Multi-threaded (8 threads) | **28 TiB/s** |
| Real workload speedup | **3.8x** (GPT-2 loading) |

## Key Features

**Constant-time allocation** - 8ns whether you need 1KB or 1MB

**Thread-safe** - Only 1ns slower than single-threaded pools, but actually concurrent

**Auto-configured** - Adapts to your CPU topology (4-core → 4 shards, 20-core → 32 shards)

**Simple API** - Just `get()` and `put()`

## Architecture

```
Thread 1     Thread 2     Thread N
┌─────────┐  ┌─────────┐  ┌─────────┐
│ TLS (4) │  │ TLS (4) │  │ TLS (4) │  ← 75% hit rate
└────┬────┘  └────┬────┘  └────┬────┘
     └──────────┬─────────────┘
                ↓
       ┌────────────────┐
       │ Sharded Pool   │              ← Power-of-2 shards
       │ [0][1]...[N]   │              ← Minimal contention
       └────────────────┘
```

**Fast path**: Thread-local cache (lock-free, ~1ns)
**Slow path**: Round-robin shard selection (minimal contention)
**Optimization**: Power-of-2 shards enable bitwise AND instead of modulo

## Configuration

```rust
use zeropool::BufferPool;

let pool = BufferPool::builder()
    .tls_cache_size(8)               // Buffers per thread
    .min_buffer_size(512 * 1024)     // Keep buffers ≥ 512KB
    .max_buffers_per_shard(32)       // Max pooled buffers
    .num_shards(16)                  // Override auto-detection
    .build();
```

**Defaults** (auto-configured based on CPU count):
- Shards: 4-128 (power-of-2, ~1 shard per 2 cores)
- TLS cache: 2-8 buffers per thread
- Min buffer size: 1MB
- Max per shard: 16-64 buffers

### Memory Pinning

Lock buffer memory in RAM to prevent swapping:

```rust
use zeropool::BufferPool;

let pool = BufferPool::builder()
    .pinned_memory(true)
    .build();
```

Useful for high-performance computing, security-sensitive data, or real-time systems. May require elevated privileges on some systems. Falls back gracefully if pinning fails.

## Benchmarks

### Single-Threaded (allocation + deallocation)

| Size  | ZeroPool | No Pool | Lifeguard | Sharded-Slab | Bytes |
|-------|----------|---------|-----------|--------------|-------|
| 1KB   | 8.4ns    | 7.2ns   | 7.0ns     | 49.8ns       | 7.5ns |
| 64KB  | 8.4ns    | 43.6ns  | 6.5ns     | 84.1ns       | 43.5ns |
| 1MB   | 8.3ns    | 35.3ns  | 6.5ns     | 76.3ns       | 44.5ns |

**Constant latency** across all sizes. **5x faster** than bytes for large buffers.

### Multi-Threaded (throughput)

| Threads | ZeroPool  | No Pool | Sharded-Slab |
|---------|-----------|---------|--------------|
| 2       | 10.3 TiB/s | 2.8 TiB/s | 1.4 TiB/s |
| 4       | 18.7 TiB/s | 5.4 TiB/s | 2.7 TiB/s |
| 8       | 28.1 TiB/s | 9.3 TiB/s | 4.8 TiB/s |

**Near-linear scaling**. 3x faster than sharded-slab under contention.

### Real-World Pattern

**Buffer reuse** (1MB buffer, 1000 get/put cycles):
- ZeroPool: 2.8µs total (2.8ns/op)
- No pool: 542ns/op
- Lifeguard: 157ns/op

**Run yourself**:
```bash
cargo bench
```

### Test System
- CPU: Intel i9-10900K @ 3.7GHz (10 cores, 20 threads, 5.3GHz turbo)
- RAM: 32GB DDR4
- OS: Linux 6.14.0

## How It Works

**Thread-local caching** (75% hit rate)
- Lock-free access to recently used buffers
- No atomic operations on fast path
- Zero cache-line bouncing

**Power-of-2 sharding**
- `shard = thread_id & (num_shards - 1)` (no modulo)
- Minimal lock contention (idle shards most of the time)
- Auto-scales with CPU count

**First-fit allocation**
- O(1) instead of O(n) best-fit
- Perfect for predictable I/O buffer sizes

**Zero-fill skip**
- Safe `unsafe` via capacity-checked `set_len()`
- I/O immediately overwrites buffers
- ~40% of the speedup vs `Vec::with_capacity`

## Thread Safety

`BufferPool` is `Clone` and thread-safe:

```rust
let pool = BufferPool::new();

for _ in 0..4 {
    let pool = pool.clone();
    std::thread::spawn(move || {
        let buf = pool.get(1024);
        // Each thread gets its own TLS cache
        pool.put(buf);
    });
}
```

## Safety Guarantees

Uses `unsafe` internally to skip zero-fills:

```rust
// SAFE: capacity checked before set_len()
if buffer.capacity() >= size {
    unsafe { buffer.set_len(size); }
}
```

**Why this is safe**:
- Capacity verified before length modification
- Buffers immediately overwritten by I/O operations
- No reads before writes in typical I/O patterns

All unsafe code is documented and audited.

## Use Cases

**High-performance I/O** - io_uring, async file loading, network buffers

**LLM inference** - Fast checkpoint loading (real-world: 200ms → 53ms for GPT-2)

**Data processing** - Predictable buffer sizes, high allocation rate

**Multi-threaded servers** - Concurrent buffer allocation without contention

## License

Dual licensed under Apache-2.0 or MIT.

## Contributing

PRs welcome! Please include benchmarks for performance changes.
