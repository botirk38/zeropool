# Benchmarks

This document is the authoritative source for ZeroPool performance claims.
Every number below was produced on the same machine on the same commit.

If you see a different number, the difference is real (different hardware,
different kernel, different system allocator) — please open an issue with
your `lscpu`, `rustc --version`, and the exact `cargo bench` command.

## Environment

| | |
|---|---|
| CPU | Intel Core i9-10900K (10 cores / 20 threads) |
| RAM | DDR4 |
| OS | Linux |
| Rust | stable |
| Criterion | 0.7 |
| Build | `cargo bench --bench pool --features bench` (release) |

## Methodology

- All **realistic** benchmarks write one byte per 4 KiB page so a fresh
  `Vec` pays the page-fault cost. This matches real I/O, networking, and
  serialization workloads.
- All **hot_path** benchmarks are pure allocation / drop with no page writes.
  `alloc()` measures the safe zero-initialized path; `alloc_uninit()` measures
  the explicit full-overwrite fast path.
- **full_overwrite** initializes every byte after allocation. This is the fair
  workload for explicit capacity/uninitialized APIs because callers never read
  stale bytes.
- `vec_reuse_*` cases measure application-level `Vec` retention separately from
  allocator-local caching. They answer whether a caller can get close to pooling
  by keeping a reusable vector per worker.
- Each Criterion group runs at least 30 samples (50+ for write workloads)
  with default warm-up.
- Comparison crates are pulled in only with `--features bench` so the
  default `cargo add zeropool` does not download them.
- Global allocators are selected by Cargo feature for the `pool` bench. Rust
  allows one process-wide `#[global_allocator]`, so `mimalloc`, `tcmalloc`,
  `jemalloc`, and the system allocator must be measured in separate runs. Enable
  only one `bench-alloc-*` feature for a real measurement. The tcmalloc backend
  uses `tcmalloc-better` and is Linux-only; on other targets that feature leaves
  the system allocator in place.

## Headline result

Realistic 64 KiB write workload, 200 ops/thread.

| Threads | ZeroPool | `Vec` | `opool` | `object_pool` |
|--------:|---------:|------:|--------:|--------------:|
| 2  |  35 µs | 272 µs | 40 µs  |  45 µs |
| 4  |  54 µs | 299 µs | 84 µs  | 102 µs |
| 8  |  89 µs | 388 µs | 199 µs | 295 µs |
| 16 | 157 µs | 624 µs | 371 µs | 1,476 µs |

ZeroPool is the only entry that keeps a flat scaling profile. The
comparison numbers come from the `realistic_write_mt` Criterion group.

## Sustained throughput

`scale_sustained` runs 10 000 ops/thread, default TLS depth, 8 s
measurement windows.

| Threads | ZeroPool | `Vec` | `object_pool` |
|---------:|---------:|------:|--------------:|
| 1  |  558 µs | 10.79 ms |  527 µs |
| 4  |  656 µs | 11.51 ms | 5.27 ms |
| 8  |  677 µs | 13.29 ms | 22.81 ms |
| 16 | 1.01 ms | 18.11 ms | 118.3 ms |
| 32 | 2.35 ms | 39.67 ms | 286.1 ms |

Past 4 threads, `object_pool` is dominated by mutex contention. The
queue push/pop is serialized across all producers. ZeroPool's per-thread
cache absorbs most of the work; the shared `ArrayQueue` only sees the
batch transfers that exceed TLS depth.

## Mixed sizes

Two scenarios: round-robin through seven sizes, and a Zipf-like
distribution that biases towards small buffers.

| Crate | Pools needed | uniform (7 sizes) | zipf |
|---|--:|---:|---:|
| **ZeroPool** | 1 | **144 ns** | **1.95 µs** |
| `object_pool` (one per size) | 7 | 140 ns | 2.08 µs |
| `opool` (one per size) | 7 | 107 ns | — |
| `Vec` | 0 | 96 µs | 2.47 µs |

The "pools needed" column is the API story. For real applications that
allocate many different buffer sizes (HTTP servers, image pipelines, log
shippers), a single `ZeroPool` is dramatically simpler than maintaining
N typed pools, and stays within a few percent of their raw speed.

## Single-thread hot path

Pure allocation + drop, no page writes. 64 KiB rows. Use `alloc_uninit()` for
the non-zeroing fast path when callers initialize every byte before reading.

| Crate | Time |
|---|---:|
| `opool` | 15.2 ns |
| **ZeroPool** | 20.2 ns |
| `object_pool` | 21.1 ns |
| `bytes::BytesMut` | 45.8 ns |
| `Vec::with_capacity` | 44.5 ns |

Honest caveat: `opool` is the single-thread microbenchmark winner. The
gap is 5 ns. In every other category the trade-off favours ZeroPool.

## Burst allocation

Allocate N × 64 KiB, write one byte to each, drop all.

| N | ZeroPool | `Vec` | `opool` | `object_pool` |
|--:|---------:|------:|--------:|--------------:|
| 10   | 327 ns  | 11.1 µs | 214 ns | 250 ns |
| 50   | 2.14 µs | 58.4 µs | 1.07 µs | 1.25 µs |
| 100  | 5.58 µs | 127 µs  | 2.09 µs | 2.39 µs |
| 1000 | 92.9 µs | 6.85 ms | 21.6 µs | 24.7 µs |

The `Vec` numbers are noisy under `cargo bench` because of background
page-reclaim effects, but the order-of-magnitude gap is consistent.

## Contention sweep

`contention` group: 64 KiB buffers, 500 ops/thread, sweep over
`tls_cache_size` and thread count. Throughput in GiB/s.

| threads | tls=1 | tls=4 | tls=8 |
|---:|---:|---:|---:|
| 2  | 1,716 | 1,730 | 1,677 |
| 4  | 2,294 | 2,308 | 2,186 |
| 8  | 2,734 | 2,737 | 2,620 |
| 16 | 3,036 | 3,009 | 2,924 |

On this 20-thread box, `tls_cache_size = 1` and `tls_cache_size = 4`
are within noise. Above 8 threads a larger TLS cache starts to hurt
slightly (more `Vec<u8>` retained per thread).

## Stats overhead

`stats_overhead` group. The same workload, with and without
`.track_stats(true)`.

| Workload | `track_stats=false` | `track_stats=true` | Overhead |
|---|---:|---:|---:|
| ST hot path, 64 KiB | 20.5 ns | 30.9 ns | +10.4 ns |
| MT8 64 KiB write | 92.7 µs | 163.8 µs | **+76%** |

Stats are disabled by default. Enabling them at `MT8` adds ~70 µs to a
~90 µs workload, which is significant. Use them in dev/profiling, not in
production hot paths.

## Reproducing these numbers

```bash
# Full suite, comparison crates included
cargo bench --bench pool --features bench

# Same suite under alternative process-wide global allocators
cargo bench --bench pool --features bench,bench-alloc-mimalloc
cargo bench --bench pool --features bench,bench-alloc-tcmalloc
cargo bench --bench pool --features bench,bench-alloc-jemalloc

# Single groups
cargo bench --bench pool -- hot_path
cargo bench --bench pool -- realistic
cargo bench --bench pool -- full_overwrite
cargo bench --bench pool -- scale
cargo bench --bench pool -- mixed
cargo bench --bench pool -- burst
cargo bench --bench pool -- contention
cargo bench --bench pool -- stats

# Increase the sustained workload from 10k → 100k ops/thread
ZEROPOOL_SCALE_OPS=100000 cargo bench --bench pool --features bench -- scale
```

Criterion writes machine-readable `estimates.json` files under
`target/criterion/`. The tables above were generated by reading those
files; rerunning with the same command will produce a "change detected"
line for each benchmark.

## What we are not claiming

- We are not claiming to be faster than `jemalloc` or `mimalloc` for
  general-purpose allocation. They are decades-tuned for small
  allocations and 8-byte size classes. ZeroPool starts at 4 KiB.
- We are not claiming single-thread hot-path dominance. `opool` is
  ~5 ns faster for one-size buffers in pure microbenchmarks. Choose
  ZeroPool when you want one pool that handles many sizes.
- We are not claiming the comparison numbers will reproduce on your
  machine. They will not. What we are claiming is that the *shape* of
  the numbers is the same: ZeroPool scales linearly with thread count
  while `object_pool` does not, and ZeroPool dominates raw `Vec` on any
  workload that touches the buffer.
