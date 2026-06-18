# ZeroPool

A user-space byte allocator for Rust.

[![Crates.io](https://img.shields.io/crates/v/zeropool.svg)](https://crates.io/crates/zeropool)
[![Docs](https://docs.rs/zeropool/badge.svg)](https://docs.rs/zeropool)
[![CI](https://github.com/botirk38/zeropool/workflows/CI/badge.svg)](https://github.com/botirk38/zeropool/actions)

## Why

Every `Vec::with_capacity(n)` asks the kernel for memory, and every `drop` gives it back. Under load — especially multi-threaded — this becomes the bottleneck: page faults, mmap syscalls, allocator contention.

ZeroPool recycles buffers so the kernel work happens once.

## Benchmarks

Every benchmark writes to every page of the buffer. This is realistic — it's what networking, serialization, and file I/O actually do. Raw alloc/drop microbenchmarks hide the page-fault cost that dominates real workloads.

### Multi-threaded (64 KB buffers, 500 ops/thread)

| Threads | ZeroPool | `Vec` | Speedup |
|---------|----------|-------|---------|
| 2       | 151 µs   | 899 µs   | **6×** |
| 4       | 258 µs   | 1,808 µs | **7×** |
| 8       | 579 µs   | 3,481 µs | **6×** |

Contention-free: each thread has its own TLS cache. No locks on the hot path.

### Burst allocation (allocate N × 64 KB, write, drop all)

| N   | ZeroPool | `Vec` | Speedup |
|-----|----------|-------|---------|
| 10  | 448 ns   | 10.9 µs  | **24×** |
| 50  | 3.1 µs   | 61.5 µs  | **20×** |
| 100 | 7.7 µs   | 124.9 µs | **16×** |

Common pattern in servers: grab a batch of buffers, process, release.

### Single-threaded (alloc → write every page → drop)

| Size | ZeroPool | `Vec` | Speedup |
|------|----------|-------|---------|
| 4 KB | 40 ns    | 125 ns   | **3.1×** |
| 64 KB | 73 ns   | 812 ns   | **11×** |
| 1 MB | 1.9 µs   | 17.5 µs  | **9×** |

### vs other pool crates

| Crate | 64 KB (ST) | 8 threads | Handles any size? |
|-------|-----------|-----------|-------------------|
| **ZeroPool** | 73 ns | 615 µs | **yes** — one pool, all sizes |
| opool | 57 ns | — | no — one type per pool |
| object_pool | 65 ns | 570 µs | no — one type per pool |
| `Vec` (no pool) | 812 ns | 3,500 µs | n/a |

Single-type pools are slightly faster in microbenchmarks because they skip size-class routing. In practice, ZeroPool replaces N separate pools with one — simpler code, same order of magnitude.

```bash
cargo bench                     # zeropool-only
cargo bench --features bench    # full comparison suite (includes opool, object_pool, sharded_slab, bytes)
cargo bench -- realistic        # just the write workloads
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

**Hot path** (~35 ns): TLS pop — no locks, no atomics.

## Configuration

```rust
let pool = ZeroPool::new()
    .tls_cache_size(8)           // per-class per-thread cache depth
    .max_buffers_per_class(64)   // shared pool capacity per class
    .min_buffer_size(4096)       // skip pooling for small buffers
    .batch_size(4)               // TLS ↔ shared transfer size
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
let s = pool.stats();
println!("{s}");
// gets: 1 | puts: 1 | hit_rate: 0.0%
//   tls_hits: 0 (0.0%) | shared_hits: 0 | allocations: 1 | discards: 0 | oversize: 0
```

`Relaxed` atomics — zero overhead on the hot path.

## Thread safety

`ZeroPool` is `Clone + Send + Sync`. Each clone shares the pool; each thread gets its own TLS cache.

## License

Apache-2.0 OR MIT
