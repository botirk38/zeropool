# HN Submission Pack

Two title options + the post body. Pick one title when submitting.

## Title A (recommended, ~78 chars)

> ZeroPool – a Rust buffer pool that scales linearly where object_pool collapses

## Title B (alt, more descriptive)

> ZeroPool: lock-free, TLS-cached byte-buffer pool for Rust. 9× object_pool at 16 threads, 4–8× raw Vec.

## Post body

I built a Rust library for recycling byte buffers that scales linearly up to 16 threads on my 20-thread box, while `object_pool` (the most popular option) gets 10× slower going from 4 to 16 threads because of its shared `Mutex`.

The short version: 64 KiB write workload, 200 ops per thread, every page touched (the realistic case):

| Threads | ZeroPool | raw `Vec` | `opool` | `object_pool` |
|--:|--:|--:|--:|--:|
| 2  |  35 µs | 272 µs |  40 µs |   45 µs |
| 4  |  54 µs | 299 µs |  84 µs |  102 µs |
| 8  |  89 µs | 388 µs | 199 µs |  295 µs |
| 16 | 157 µs | 624 µs | 371 µs | 1,476 µs |

Sustained 10 000 ops/thread, 8 s measurement windows:

| Threads | ZeroPool | raw `Vec` | `object_pool` |
|--:|--:|--:|--:|
| 4  |   656 µs | 11.5 ms |  5.3 ms |
| 8  |   677 µs | 13.3 ms | 22.8 ms |
| 16 | 1.01 ms  | 18.1 ms |  118 ms |
| 32 | 2.35 ms  | 39.7 ms |  286 ms |

`object_pool` collapses past 4 threads because all producers serialize on one `Mutex`. ZeroPool keeps a per-thread cache per size class and only touches the shared lock-free queue on misses.

### The design in 30 seconds

- 8 power-of-two size classes (4 KiB → 64 MB), routed in O(1) with CLZ.
- Per-thread cache per class. Miss → batch refill from a `crossbeam::ArrayQueue`. No mutex anywhere on the hot path.
- `Buf<'a>` borrows the pool. No `Arc` clone on every alloc — that alone was ~5 ns shaved off `pool.get()`.
- `Allocator` trait so you can plug in huge pages, mmap, your own mlocked regions, etc.
- `track_stats(true)` for allocation counters. Disabled by default because hot-path `Relaxed` atomics were ~50% of the 8-thread write workload in my profiling.

```rust
let pool = ZeroPool::new();
let mut buf = pool.alloc(64 * 1024);  // Buf<'_> borrowing the pool
buf[0] = 42;
// returned to pool on drop
```

### What I am NOT claiming

- Faster than `jemalloc`/`mimalloc` for general-purpose allocation. They are tuned for sub-4 KiB objects; ZeroPool starts at 4 KiB.
- Single-thread hot-path dominance. `opool` is ~5 ns faster in pure microbenchmarks. Pick ZeroPool when you want one pool that handles many sizes, or when contention matters.
- Hardware-portable numbers. These are on an i9-10900K. The *shape* should hold; the absolute numbers will not.

### What I want from HN

1. Pokes at the benchmark methodology. The full bench suite, environment, and reproduction steps are in `BENCHMARKS.md`. If something is unfair, I want to know.
2. Rust API feedback. `Buf<'a>` lifetime-bound or owned `Buf` first? Currently you can `into_vec()` to detach.
3. Server-class hardware runs. My 20-thread desktop hides some contention patterns. I would love numbers from a 64+ thread box.
4. Comparisons I'm missing. I'm benchmarking against `Vec`, `opool`, `object_pool`, and `bytes`. What should I add?

Repo: https://github.com/botirk38/zeropool
Full benchmarks: https://github.com/botirk38/zeropool/blob/master/BENCHMARKS.md

35 unit + 14 doc tests, 9 Criterion groups, all behind `--features bench` so the default `cargo add zeropool` doesn't pull in anything extra.

## Submitting on HN

- Use Title A or Title B.
- HN titles can't end in a period.
- HN has an 80-char limit on titles (Title A: 78, Title B: 95 — trim if you use B).
- Post body is plain text. The markdown tables will render as plain text on HN, which is fine.
- Best time to post: Tue–Thu, 8–10 AM US Eastern. Avoid Mondays and weekends.
- After posting, be ready to answer the first three criticisms within 5 minutes. The most common ones will be: (a) "this is just a pool, what's new", (b) "your benchmarks are unfair because [X]", (c) "why not just use jemalloc".

## Things that hurt virality

- Claiming to beat `jemalloc` for general allocation. I am explicitly not claiming this.
- Skipping methodology. I am including the full table in `BENCHMARKS.md`.
- Asking for upvotes. HN downvotes that.
- Posting a "Show HN" without a working demo. We have one — `cargo add zeropool` and it works.

## Things that help

- Concrete, reproducible numbers with the environment listed.
- An honest "here's where I lose" section.
- A specific ask ("run this on a 64-thread box").
- A working repo, not a blog post about a future repo.
