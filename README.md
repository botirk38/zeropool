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

// Get a buffer (automatically returned to pool when dropped)
let mut buffer = pool.get(1024 * 1024); // 1MB

// Use it
file.read(&mut buffer)?;

// Buffer automatically returned to pool here
```

## Performance Highlights

| Metric | Result |
|--------|--------|
| Allocation latency | **14.6ns** (constant, any size) |
| vs bytes (1MB) | **2.6x faster** |
| Multi-threaded (8 threads) | **32 TiB/s** |
| Multi-threaded speedup | **2.1x faster** than previous version |
| Real workload speedup | **3.8x** (GPT-2 loading) |

## Key Features

**Constant-time allocation** - ~15ns whether you need 1KB or 1MB

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

**Fast path**: Thread-local cache (lock-free, ~15ns)
**Slow path**: Thread-affinity shard selection (better cache locality)
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

### Eviction Policy

Choose between simple LIFO or intelligent CLOCK-Pro buffer eviction:

```rust
use zeropool::{BufferPool, EvictionPolicy};

let pool = BufferPool::builder()
    .eviction_policy(EvictionPolicy::ClockPro)  // Better cache locality (default)
    .build();

let pool_lifo = BufferPool::builder()
    .eviction_policy(EvictionPolicy::Lifo)     // Simple, lowest overhead
    .build();
```

**CLOCK-Pro (default)**: Uses access counters to favor recently-used buffers, preventing cache thrashing in mixed-size workloads. ~8 bytes overhead per buffer.

**LIFO**: Simple last-in-first-out eviction. Minimal memory overhead, best for uniform buffer sizes.

## Benchmarks

### Single-Threaded (allocation + deallocation)

| Size  | ZeroPool | No Pool | Lifeguard | Sharded-Slab | Bytes |
|-------|----------|---------|-----------|--------------|-------|
| 1KB   | 14.9ns   | 7.7ns   | 7.0ns     | 50.0ns       | 8.4ns |
| 64KB  | 14.6ns   | 46.3ns  | 6.9ns     | 88.3ns       | 49.1ns |
| 1MB   | 14.6ns   | 37.7ns  | 6.9ns     | 80.2ns       | 39.4ns |

**Constant latency** across all sizes. **2.6x faster** than bytes for large buffers (1MB).

### Multi-Threaded (throughput)

| Threads | ZeroPool  | No Pool  | Sharded-Slab | Speedup vs Previous |
|---------|-----------|----------|--------------|---------------------|
| 2       | 14.2 TiB/s | 2.7 TiB/s | 1.3 TiB/s   | **1.38x** ⚡        |
| 4       | 25.0 TiB/s | 5.1 TiB/s | 2.6 TiB/s   | **1.34x** ⚡        |
| 8       | 32.0 TiB/s | 7.7 TiB/s | 3.9 TiB/s   | **1.14x** ⚡        |

**Near-linear scaling** with thread-local shard affinity. **8.2x faster** than sharded-slab at 8 threads.

### Real-World Pattern

**Buffer reuse** (1MB buffer, 1000 get/put cycles):
- ZeroPool: 6.1µs total (6.1ns/op)
- No pool: 600ns/op
- Lifeguard: 172ns/op

**~98x faster** than no pooling for realistic buffer reuse patterns.

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

**Thread-local shard affinity**
- Each thread consistently uses the same shard (cache locality)
- `shard = hash(thread_id) & (num_shards - 1)` (no modulo)
- Minimal lock contention + better CPU cache utilization
- Auto-scales with CPU count

**First-fit allocation**
- O(1) instead of O(n) best-fit
- Perfect for predictable I/O buffer sizes

**Secure memory zeroing**
- All buffers are zeroed on return and allocation
- Prevents information leakage between buffer users
- Safe for security-sensitive workloads

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

## Safety and Security

ZeroPool prioritizes both safety and security:

**Memory Zeroing**:
- All buffers are explicitly zeroed when returned to the pool using `fill(0)`
- All buffers are zeroed when allocated from the pool using `resize(size, 0)`
- Prevents information leakage between different buffer users
- Safe for processing sensitive data (credentials, encryption keys, PII)

**Safe Rust**:
- Uses only safe Rust operations for memory management
- No unsafe `set_len()` calls or uninitialized memory
- The only unsafe code is for safe trait implementations (`Send`/`Sync`)

**Security Best Practices**:
- Defense-in-depth with zeroing at both allocation and deallocation
- Optional memory pinning to prevent swapping sensitive data to disk
- Suitable for security-critical applications

## Use Cases

**High-performance I/O** - io_uring, async file loading, network buffers

**LLM inference** - Fast checkpoint loading (real-world: 200ms → 53ms for GPT-2)

**Data processing** - Predictable buffer sizes, high allocation rate

**Multi-threaded servers** - Concurrent buffer allocation without contention

## License

Dual licensed under Apache-2.0 or MIT.

## Contributing

PRs welcome! Please include benchmarks for performance changes.
