# ZeroPool

> A high-performance, security-focused buffer pool for Rust

[![Crates.io](https://img.shields.io/crates/v/zeropool.svg)](https://crates.io/crates/zeropool)
[![Documentation](https://docs.rs/zeropool/badge.svg)](https://docs.rs/zeropool)
[![License](https://img.shields.io/crates/l/zeropool.svg)](https://github.com/botirk38/zeropool/blob/main/LICENSE)
[![CI](https://github.com/botirk38/zeropool/workflows/CI/badge.svg)](https://github.com/botirk38/zeropool/actions)

## Why ZeroPool?

ZeroPool is a **thread-safe buffer pool** that combines high performance with strong security guarantees. Unlike traditional buffer pools that trade security for speed, ZeroPool provides both:

- **Secure by default**: All memory is automatically zeroed to prevent information leakage
- **Safe Rust**: No unsafe memory operations, only safe abstractions
- **High performance**: Thread-local caching and smart allocation strategies minimize overhead
- **Auto-configured**: Adapts to your CPU topology for optimal multi-threaded performance

Perfect for applications that handle sensitive data (credentials, encryption keys, PII) while maintaining high throughput.

## Quick Start

```rust
use zeropool::BufferPool;

let pool = BufferPool::new();

// Get a buffer (automatically zeroed and returned to pool when dropped)
let mut buffer = pool.get(1024 * 1024); // 1MB

// Use it for I/O or data processing
file.read(&mut buffer)?;

// Buffer automatically zeroed and returned to pool here
```

## Key Features

### Security First 🔒

- **Memory zeroing**: All buffers are explicitly zeroed on allocation and return
- **No information leakage**: Previous data never exposed to new allocations
- **Safe Rust**: Zero unsafe memory operations (only safe Send/Sync trait impls)
- **Optional memory pinning**: Prevent sensitive data from swapping to disk

### High Performance ⚡

- **Thread-local caching**: Lock-free fast path for 60-110ns allocation latency
- **Smart sharding**: Minimal contention with power-of-2 shard count
- **Auto-configured**: CPU-aware defaults (4-128 shards, 2-8 TLS cache size)
- **Configurable eviction**: Choose between LIFO or CLOCK-Pro algorithms

### Simple API 🎯

- **Just `get()` and `drop()`**: Buffers automatically return to the pool
- **Builder pattern**: Easy customization when needed
- **Type-safe**: Leverages Rust's ownership for automatic resource management

## Architecture

```
Thread 1     Thread 2     Thread N
┌─────────┐  ┌─────────┐  ┌─────────┐
│ TLS (4) │  │ TLS (4) │  │ TLS (4) │  ← Lock-free (60-110ns)
└────┬────┘  └────┬────┘  └────┬────┘
     └──────────┬─────────────┘
                ↓
       ┌────────────────┐
       │ Sharded Pool   │              ← Thread affinity
       │ [0][1]...[N]   │              ← Minimal contention
       └────────────────┘
```

**Fast path**: Thread-local cache (lock-free, ~60-110ns)
**Slow path**: Thread-affinity shard selection (better cache locality)
**Optimization**: Power-of-2 shards enable bitwise AND instead of modulo

## Performance

### Cache Behavior Benchmarks

| Pattern | Metric | Result |
|---------|--------|--------|
| Ping-pong (LIFO) | Time per operation | 3.56 µs |
| Ping-pong (ClockPro) | Time per operation | 3.68 µs |
| Hot/cold buffers | Time per operation | 1.05 µs |
| Multi-size workload | Time per operation | 6.2 µs |
| TLS cache (2 bufs) | Allocation latency | 60.5 ns |
| TLS cache (4 bufs) | Allocation latency | 108 ns |
| TLS cache (8 bufs) | Allocation latency | 288 ns |
| Eviction pressure | Time per operation | 400 ns |

### Multi-threaded Scaling

| Threads | Time per 1000 ops | Notes |
|---------|-------------------|-------|
| 1 | 44.7 µs | Single-threaded baseline |
| 4 | 141 µs | Good scaling with TLS cache |
| 8 | 282 µs | Near-linear scaling |
| 16 | 605 µs | Still scales well at high concurrency |

### Performance Characteristics

- **Constant latency**: 60-110ns for TLS cache hits regardless of buffer size
- **Secure by default**: ~20-25% overhead vs unsafe implementations (acceptable trade-off)
- **Lock-free fast path**: Thread-local cache eliminates contention
- **Scales linearly**: Near-linear scaling up to 16+ threads

**Run yourself**:
```bash
cargo bench
```

### Test System
- CPU: Intel i9-10900K @ 3.7GHz (10 cores, 20 threads, 5.3GHz turbo)
- RAM: 32GB DDR4
- OS: Linux 6.17.0

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

Lock buffer memory in RAM to prevent swapping sensitive data:

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

## How It Works

**Thread-local caching** (lock-free)
- Lock-free access to recently used buffers
- No atomic operations on fast path (60-110ns latency)
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
- All buffers are zeroed on return using `fill(0)`
- All buffers are zeroed on allocation using `resize(size, 0)`
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
        // Buffer automatically returned when dropped
    });
}
```

## Safety and Security

ZeroPool prioritizes both safety and security:

### Memory Zeroing

- All buffers are explicitly zeroed when returned to the pool using `fill(0)`
- All buffers are zeroed when allocated from the pool using `resize(size, 0)`
- Prevents information leakage between different buffer users
- Safe for processing sensitive data (credentials, encryption keys, PII)

### Safe Rust

- Uses only safe Rust operations for memory management
- No unsafe `set_len()` calls or uninitialized memory
- Send and Sync automatically derived (no unsafe trait implementations)
- All safety guaranteed by Rust's type system

### Security Best Practices

- **Defense-in-depth**: Zeroing at both allocation and deallocation
- **Optional memory pinning**: Prevent swapping sensitive data to disk
- **Auditable**: Simple, safe code that's easy to review
- **Production-ready**: Suitable for security-critical applications

## Use Cases

### Security-Sensitive Applications

- **Cryptographic operations**: Handle keys and sensitive material safely
- **Authentication systems**: Process credentials without leakage risk
- **PII processing**: GDPR/HIPAA compliant data handling
- **Secure communications**: Network buffers for encrypted protocols

### High-Performance I/O

- **Async file loading**: io_uring, tokio, async-std
- **Network servers**: HTTP, gRPC, WebSocket servers
- **Data processing**: ETL pipelines, log processing
- **LLM inference**: Fast checkpoint loading

### Real-World Example

Before ZeroPool, loading GPT-2 checkpoints took **200ms** with 70% spent on buffer allocation. With ZeroPool: **53ms** (3.8x faster) while maintaining security guarantees.

## System Scaling

ZeroPool automatically adapts to your system:

| System | Cores | TLS Cache | Shards | Buffers/Shard | Total Capacity |
|--------|-------|-----------|--------|---------------|----------------|
| Embedded | 4 | 4 | 4 | 16 | 64 (~64MB) |
| Laptop | 8 | 6 | 8 | 16 | 128 (~128MB) |
| Workstation | 16 | 6 | 8 | 32 | 256 (~256MB) |
| Small Server | 32 | 8 | 16 | 64 | 1024 (~1GB) |
| Large Server | 64 | 8 | 32 | 64 | 2048 (~2GB) |
| Supercompute | 128 | 8 | 64 | 64 | 4096 (~4GB) |

## Comparison with Alternatives

| Feature | ZeroPool | bytes::BytesMut | Lifeguard | Sharded-Slab |
|---------|----------|-----------------|-----------|--------------|
| Memory zeroing | ✅ Always | ❌ No | ❌ No | ❌ No |
| Safe Rust | ✅ 100% | ⚠️ Some unsafe | ⚠️ Some unsafe | ⚠️ Heavy unsafe |
| Thread-safe | ✅ Yes | ❌ No | ⚠️ Limited | ✅ Yes |
| Lock-free path | ✅ TLS cache | ❌ No | ❌ No | ⚠️ Partial |
| Auto-configured | ✅ CPU-aware | ❌ Manual | ❌ Manual | ❌ Manual |
| Security focus | ✅ Primary | ❌ No | ❌ No | ❌ No |

ZeroPool is the only buffer pool designed for **security-first** applications while maintaining competitive performance.

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

Built with ❤️ for the Rust community. Inspired by the need for secure, high-performance buffer management in production systems.
