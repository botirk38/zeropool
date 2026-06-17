# ZeroPool

A user-space byte allocator for Rust.

[![Crates.io](https://img.shields.io/crates/v/zeropool.svg)](https://crates.io/crates/zeropool)
[![Docs](https://docs.rs/zeropool/badge.svg)](https://docs.rs/zeropool)
[![CI](https://github.com/botirk38/zeropool/workflows/CI/badge.svg)](https://github.com/botirk38/zeropool/actions)

## Usage

```rust
use zeropool::ZeroPool;

let pool = ZeroPool::new();

let mut buf = pool.alloc(1024 * 1024); // 1MB — returned as RAII guard
buf[0] = 42;                            // Deref<Target = [u8]>
// automatically deallocated back to pool on drop
```

## How it works

Buffers are routed into 8 power-of-two size classes (4KB–64MB). Each thread keeps a local cache per class; when empty, a batch of buffers is moved from the shared lock-free queue (`crossbeam::ArrayQueue`). Unique pool IDs prevent cross-pool contamination in TLS.

**Hot path** (~24ns): TLS cache pop — no locks, no atomics on the data path.

## Configuration

Defaults are auto-tuned to CPU count. Override as needed:

```rust
let pool = ZeroPool::new()
    .tls_cache_size(8)           // per-class per-thread cache depth
    .max_buffers_per_class(64)   // shared pool capacity per class
    .min_buffer_size(4096)       // discard returned buffers smaller than this
    .batch_size(4)               // TLS <-> shared transfer size
    .pinned_memory(true);        // mlock buffers to prevent swapping
```

## Pluggable Allocator

Control how buffers are created with a custom [`Allocator`](https://docs.rs/zeropool/latest/zeropool/trait.Allocator.html):

```rust
use zeropool::{Allocator, ZeroPool};

struct PrefaultAllocator;
impl Allocator for PrefaultAllocator {
    fn allocate(&self, capacity: usize) -> Vec<u8> {
        let mut buf = Vec::with_capacity(capacity);
        buf.resize(capacity, 0); // pre-fault pages
        buf.clear();
        buf
    }
}

let pool = ZeroPool::new().allocator(PrefaultAllocator);
```

## Global Allocator

Replace the process-wide allocator with ZeroPool-style size-class caching. Enable with `features = ["global-alloc"]`:

```rust
use zeropool::ZeroPoolGlobalAlloc;

#[global_allocator]
static ALLOC: ZeroPoolGlobalAlloc = ZeroPoolGlobalAlloc::new();
```

Freed blocks are held in lock-free Treiber stacks per size class and reused on subsequent allocations. Falls through to `std::alloc::System` for cache misses and oversize requests.

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

`ZeroPool` is `Clone + Send + Sync` (`Arc<State>` internally). Each clone shares the same pool; each thread gets its own TLS cache automatically.

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
