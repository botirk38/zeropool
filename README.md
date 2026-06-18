# ZeroPool

A user-space byte allocator for Rust.

[![Crates.io](https://img.shields.io/crates/v/zeropool.svg)](https://crates.io/crates/zeropool)
[![Docs](https://docs.rs/zeropool/badge.svg)](https://docs.rs/zeropool)
[![CI](https://github.com/botirk38/zeropool/workflows/CI/badge.svg)](https://github.com/botirk38/zeropool/actions)

## Why

Every `Vec::with_capacity(n)` asks the kernel for memory, and every `drop` gives it back. Under load — especially multi-threaded — this becomes the bottleneck: page faults, mmap syscalls, allocator contention.

ZeroPool recycles buffers so the kernel work happens once.

## Benchmarks

Every "realistic" benchmark writes to every page of the buffer. This is what networking, serialization, and file I/O actually do. Pure alloc/drop microbenchmarks hide the page-fault cost that dominates real workloads.

Full transparent numbers and reproducibility steps live in [`BENCHMARKS.md`](./BENCHMARKS.md).

### Headline: realistic 64 KiB write, 200 ops/thread

| Threads | ZeroPool | `Vec` | `opool` | `object_pool` |
|---------|---------:|------:|--------:|--------------:|
| 2       |  35 µs   | 272 µs | 40 µs  |  45 µs |
| 4       |  54 µs   | 299 µs | 84 µs  | 102 µs |
| 8       |  89 µs   | 388 µs | 199 µs | 295 µs |
| 16      | 157 µs   | 624 µs | 371 µs | 1,476 µs |

ZeroPool is `2.2×`–`9.4×` faster than `object_pool` and within `1.1×`–`2.4×` of `opool` as contention grows. Compared to raw `Vec`, ZeroPool is `4.0×`–`7.7×` faster across the same range.

### Sustained: 10 000 ops/thread, 64 KiB write, 8 s measurement

| Threads | ZeroPool | `Vec` | `object_pool` |
|---------:|---------:|------:|--------------:|
| 1  |  558 µs | 10.79 ms |  527 µs |
| 4  |  656 µs | 11.51 ms | 5.27 ms |
| 8  |  677 µs | 13.29 ms | 22.81 ms |
| 16 | 1.01 ms | 18.11 ms | 118.3 ms |
| 32 | 2.35 ms | 39.67 ms | 286.1 ms |

`object_pool` collapses past 4 threads because its `Mutex`-guarded shared queue serializes all producers. ZeroPool's lock-free `ArrayQueue` + per-thread cache scales linearly up to 16 threads on this 20-thread box.

### Mixed sizes: one pool handles many buffer sizes

| Crate | Pools needed | 7 sizes, round-robin | Zipf-like 1 KiB–256 KiB |
|---|--:|---:|---:|
| **ZeroPool** | 1 | **144 ns** | **1.95 µs** |
| `object_pool` (one per size) | 7 | 140 ns | 2.08 µs |
| `opool` (one per size) | 7 | 107 ns | — |
| `Vec` (no pool) | 0 | 96 µs | 2.47 µs |

ZeroPool is the same order of magnitude as N typed pools while managing a single object — and **660× faster than raw `Vec` on the round-robin case**.

### Single-thread hot path (no page writes)

| 64 KiB | Time |
|---|---:|
| `opool` | 15.2 ns |
| **ZeroPool** | 20.2 ns |
| `object_pool` | 21.1 ns |
| `bytes::BytesMut` | 45.8 ns |
| `Vec::with_capacity` | 44.5 ns |

`opool` is still the single-thread microbenchmark winner. ZeroPool is within 5 ns of it and is now within `1 ns` of `object_pool`. Stats tracking adds ~10 ns — opt in only when you need it (see [`stats_overhead`](#stats)).

### Burst: allocate N × 64 KiB, write, drop all

| N    | ZeroPool | `Vec` | `opool` | `object_pool` |
|-----:|---------:|------:|--------:|--------------:|
| 10   | 327 ns  | 11.1 µs | 214 ns | 250 ns |
| 50   | 2.14 µs | 58.4 µs | 1.07 µs | 1.25 µs |
| 100  | 5.58 µs | 127 µs  | 2.09 µs | 2.39 µs |
| 1000 | 92.9 µs | 6.85 ms | 21.6 µs | 24.7 µs |

Burst is where raw `Vec` falls furthest behind. For server workloads that grab a batch of buffers, ZeroPool is `~74×` faster than `Vec` at `N=1000`.

### Reproduce

```bash
cargo bench                     # ZeroPool-only timing
cargo bench --features bench    # full comparison suite
cargo bench -- realistic        # just the write workloads
cargo bench -- hot_path         # pure alloc/drop microbenchmarks
cargo bench -- scale            # sustained throughput
cargo bench -- mixed            # mixed-size workloads
cargo bench -- burst            # burst allocation
cargo bench -- contention       # TLS depth × thread count
cargo bench -- stats            # stats overhead
```

## Usage

```rust
use zeropool::ZeroPool;

let pool = ZeroPool::new();

let mut buf = pool.alloc(1024 * 1024); // 1 MB — RAII guard
buf[0] = 42;                            // Deref<Target = [u8]>
// returned to pool on drop
```

## How it works

8 power-of-two size classes (4 KB → 64 MB). Each thread keeps a local cache per class; on miss, a batch is moved from the lock-free shared queue (`crossbeam::ArrayQueue`). Pool IDs prevent TLS cross-contamination.

**Hot path** (~20 ns): TLS pop — no locks, no atomics by default.

## Configuration

```rust
let pool = ZeroPool::new()
    .tls_cache_size(8)           // per-class per-thread cache depth
    .max_buffers_per_class(64)   // shared pool capacity per class
    .min_buffer_size(4096)       // skip pooling for small buffers
    .batch_size(4)               // TLS ↔ shared transfer size
    .track_stats(true)           // opt into atomic counters
    .pinned_memory(true);        // mlock buffers (no swap)
```

## Pluggable allocator

```rust
use zeropool::{Allocator, ZeroPool};

struct HugePageAllocator;
impl Allocator for HugePageAllocator {
    fn allocate(&self, capacity: usize) -> Vec<u8> {
        let mut buf = Vec::with_capacity(capacity);
        buf.resize(capacity, 0); // pre-fault
        buf.clear();
        buf
    }
}

let pool = ZeroPool::new().allocator(HugePageAllocator);
```

## Stats

```rust
let pool = ZeroPool::new().track_stats(true);
let s = pool.stats();
println!("{s}");
// gets: 1 | puts: 1 | hit_rate: 0.0%
//   tls_hits: 0 (0.0%) | shared_hits: 0 | allocations: 1 | discards: 0 | oversize: 0
```

Stats are disabled by default because even `Relaxed` atomics are measurable in tight allocation loops. Enable them with `.track_stats(true)` when you need counters.

## Thread safety

`ZeroPool` is `Send + Sync`. Share it across threads via `&pool` (scoped threads) or `Arc<ZeroPool>`:

```rust
use std::sync::Arc;
use std::thread;
use zeropool::ZeroPool;

// Scoped threads — borrow directly
let pool = ZeroPool::new();
thread::scope(|s| {
    s.spawn(|| { let buf = pool.alloc(4096); });
    s.spawn(|| { let buf = pool.alloc(4096); });
});

// Owned threads — wrap in Arc
let pool = Arc::new(ZeroPool::new());
let p = Arc::clone(&pool);
thread::spawn(move || { let buf = p.alloc(4096); });
```

Each thread gets its own TLS cache automatically. `Buf<'_>` is lifetime-bound to the pool — use `buf.into_vec()` when you need an owned `Vec<u8>`.

## License

Apache-2.0 OR MIT
