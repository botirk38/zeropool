# ZeroPool

A high-performance, lock-free buffer pool for Rust.

[![Crates.io](https://img.shields.io/crates/v/zeropool.svg)](https://crates.io/crates/zeropool)
[![Docs](https://docs.rs/zeropool/badge.svg)](https://docs.rs/zeropool)
[![CI](https://github.com/botirk38/zeropool/workflows/CI/badge.svg)](https://github.com/botirk38/zeropool/actions)

## Usage

```rust
use zeropool::BufferPool;

let pool = BufferPool::new();

let mut buf = pool.get(1024 * 1024); // 1MB — returned as RAII guard
buf[0] = 42;                         // Deref<Target = [u8]>
// automatically returned to pool on drop
```

## How it works

Buffers are routed into 8 power-of-two size classes (4KB–64MB). Each thread keeps a local cache per class; when empty, a batch of buffers is moved from the shared lock-free queue (`crossbeam::ArrayQueue`). Unique pool IDs prevent cross-pool contamination in TLS.

**Hot path** (~24ns): TLS cache pop — no locks, no atomics on the data path.

## Configuration

Defaults are auto-tuned to CPU count. Override as needed:

```rust
let pool = BufferPool::new()
    .tls_cache_size(8)           // per-class per-thread cache depth
    .max_buffers_per_class(64)   // shared pool capacity per class
    .min_buffer_size(4096)       // discard returned buffers smaller than this
    .batch_size(4)               // TLS <-> shared transfer size
    .pinned_memory(true);        // mlock buffers to prevent swapping
```

## Statistics

```rust
let s = pool.stats();
println!("{s}");
// gets: 1 | puts: 1 | hit_rate: 0.0%
//   tls_hits: 0 (0.0%) | shared_hits: 0 | allocations: 1 | discards: 0 | oversize: 0

pool.reset_stats();
```

Counters use `Relaxed` atomics — no measurable overhead on the hot path.

## Thread safety

`BufferPool` is `Clone + Send + Sync` (`Arc<PoolState>` internally). Each clone shares the same pool; each thread gets its own TLS cache automatically.

## Benchmarks

Single-threaded TLS-hit latency (2-core VM):

| Size | Latency |
|------|---------|
| 1KB–64KB | ~24 ns |
| 1MB | ~25 ns |

```bash
cargo bench                    # all
cargo bench -- single_thread   # hot path
cargo bench -- multi_thread    # contention
```

## License

Apache-2.0 OR MIT
