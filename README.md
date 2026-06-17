# ZeroPool

A user-space byte allocator for Rust.

[![Crates.io](https://img.shields.io/crates/v/zeropool.svg)](https://crates.io/crates/zeropool)
[![Docs](https://docs.rs/zeropool/badge.svg)](https://docs.rs/zeropool)
[![CI](https://github.com/botirk38/zeropool/workflows/CI/badge.svg)](https://github.com/botirk38/zeropool/actions)

## Why

Every `Vec::with_capacity(64 * 1024)` hits the system allocator, and every `drop` frees it back. For I/O-heavy workloads (networking, serialization, file pipelines), this is the bottleneck — page faults, mmap syscalls, allocator contention under threads.

ZeroPool recycles buffers so the kernel work happens once.

## Usage

```rust
use zeropool::ZeroPool;

let pool = ZeroPool::new();

let mut buf = pool.alloc(1024 * 1024); // 1 MB — returned as RAII guard
buf[0] = 42;                            // Deref<Target = [u8]>
// automatically returned to pool on drop
```

## Benchmarks

All numbers from `cargo bench --features bench`. Realistic benchmarks write to every page of the buffer — this is what I/O code actually does.

### Alloc → write → drop (single thread)

| Buffer size | ZeroPool | `Vec` (system alloc) | Speedup |
|-------------|----------|----------------------|---------|
| 4 KB        | 36 ns    | 105 ns               | **2.9×** |
| 64 KB       | 75 ns    | 803 ns               | **10.7×** |
| 1 MB        | 1.9 µs   | 19.7 µs              | **10.2×** |

### Alloc → write → drop (multi-threaded, 64 KB, 500 ops/thread)

| Threads | ZeroPool | `Vec` (system alloc) | Speedup |
|---------|----------|----------------------|---------|
| 2       | 151 µs   | 899 µs               | **6.0×** |
| 4       | 258 µs   | 1,808 µs             | **7.0×** |
| 8       | 579 µs   | 3,481 µs             | **6.0×** |

### Burst: allocate N × 64 KB, touch each, drop all

| N   | ZeroPool | `Vec` (system alloc) | Speedup |
|-----|----------|----------------------|---------|
| 10  | 448 ns   | 10.9 µs              | **24×** |
| 50  | 3.1 µs   | 61.5 µs              | **20×** |
| 100 | 7.7 µs   | 124.9 µs             | **16×** |

### Comparison with pool crates (64 KB, single-thread write)

| Crate | Latency | Notes |
|-------|---------|-------|
| **ZeroPool** | **75 ns** | Handles any size, RAII, stats |
| opool | 55 ns | One type per pool, no size routing |
| object_pool | 62 ns | One type per pool, mutex-based |
| `Vec` (no pool) | 803 ns | Fresh alloc + free every time |

ZeroPool is within 20% of single-type pools while handling arbitrary buffer sizes through one pool.

### Mixed sizes (1 KB → 1 MB, single-thread)

| Approach | Latency | Setup |
|----------|---------|-------|
| **ZeroPool** | **218 ns** | Single pool, any size |
| object_pool | 59 ns | 6 separate pools (one per size) |
| `Vec` (no pool) | 24.4 µs | **112× slower** |

```bash
cargo bench                     # zeropool-only benchmarks
cargo bench --features bench    # full comparison suite
cargo bench -- realistic        # just the write workloads
```

## How it works

Buffers are routed into 8 power-of-two size classes (4 KB – 64 MB). Each thread keeps a local cache per class; when empty, a batch is moved from the shared lock-free queue (`crossbeam::ArrayQueue`). Unique pool IDs prevent cross-pool contamination in TLS.

**Hot path** (~35 ns): TLS cache pop — no locks, no atomics on the data path.

## Configuration

Defaults are auto-tuned to CPU count. Override as needed:

```rust
let pool = ZeroPool::new()
    .tls_cache_size(8)           // per-class per-thread cache depth
    .max_buffers_per_class(64)   // shared pool capacity per class
    .min_buffer_size(4096)       // discard returned buffers smaller than this
    .batch_size(4)               // TLS ↔ shared transfer size
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

## License

Apache-2.0 OR MIT
