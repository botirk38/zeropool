# ZeroPool Profiling Programs

This directory contains profiling programs designed to work with `cargo flamegraph` to identify performance hotspots in zeropool.

**Note:** These programs have been optimized to:
1. **Minimize computation overhead** - reduces non-zeropool work to make pool operations visible
2. **Maximize iteration counts** - runs longer to collect sufficient profiling samples
3. **Focus on zeropool internals** - allocation, deallocation, TLS cache, shard locking, eviction

**Why this matters:** Zeropool is so fast (~14ns allocations) that short-running programs spend most time in startup/initialization. These programs run long enough to make zeropool operations clearly visible in flamegraphs.

## Prerequisites

Install cargo-flamegraph:
```bash
cargo install flamegraph
```

On Linux, you may need to adjust perf settings:
```bash
echo -1 | sudo tee /proc/sys/kernel/perf_event_paranoid
```

## Available Programs

### 1. ML Checkpoint Loader (`ml_checkpoint_loader`)

Simulates loading large machine learning model checkpoints (2GB total).

**What it tests:**
- Large sequential allocations (128MB-256MB tensors)
- Mixed small/large buffers (4KB metadata + MB-sized weights)
- Buffer reuse across multiple epochs
- Realistic ML workload pattern

**Run:**
```bash
cargo flamegraph --profile profiling --bin ml_checkpoint_loader
```

**Configuration:** Edit constants at top of `ml_checkpoint_loader.rs`

### 2. Network Server (`network_server`)

Simulates a multi-threaded network server with concurrent connections.

**What it tests:**
- Multi-threaded contention (8 worker threads)
- Realistic packet size distribution (64B-64KB, Zipf-like)
- Request/response cycles
- TLS cache effectiveness
- Shard contention patterns

**Run:**
```bash
cargo flamegraph --profile profiling --bin network_server
```

**Configuration:** Edit constants at top of `network_server.rs`

### 3. File Pipeline (`file_pipeline`)

Simulates a file processing pipeline with Read → Transform → Write stages.

**What it tests:**
- Sequential file processing (100 files)
- Varied chunk sizes (4KB-1MB) for different file types
- Multi-stage pipeline with buffer handoff
- Buffer reuse between stages

**Run:**
```bash
cargo flamegraph --profile profiling --bin file_pipeline
```

**Configuration:** Edit constants at top of `file_pipeline.rs`

### 4. Stress Test (`stress_test`)

High-stress workload combining multiple challenging patterns.

**What it tests:**
- Burst allocations (sudden spikes)
- Cache thrashing (alternating sizes)
- Multi-threaded contention (12 threads)
- Eviction policy under pressure
- Random chaotic access patterns

**Run:**
```bash
cargo flamegraph --profile profiling --bin stress_test
```

**Configuration:** Edit constants at top of `stress_test.rs`

## Understanding Flamegraphs

After running a profiling program, a `flamegraph.svg` file will be generated. Open it in a browser.

**Key areas to look for:**
- `BufferPool::get` - Main allocation path
- `return_buffer` - Buffer return path
- TLS cache operations
- Shard selection and locking
- Eviction policy logic

**Width = CPU time spent**
- Wider boxes = more time spent
- Look for unexpectedly wide boxes in hot paths

## Custom Profiling Profile

These programs use the `profiling` Cargo profile defined in `Cargo.toml`:
```toml
[profile.profiling]
inherits = "release"
debug = true
```

This gives you:
- Release-level optimizations (realistic performance)
- Debug symbols (readable flamegraphs)
- Separate from `release` and `bench` profiles

## Tips

1. **Run time:** Each program runs for 10-60 seconds to capture meaningful profiles
2. **Multiple runs:** Compare flamegraphs across different configurations
3. **Modify workloads:** Edit the const parameters to test different scenarios
4. **Focus areas:** Use flamegraph search (Ctrl+F) to find specific functions
5. **Compare:** Run before/after profiling when making optimizations

## Using Perf Directly (Advanced)

For more detailed analysis including assembly-level annotations:

```bash
# Build the profiling binary
cargo build --profile profiling --bin stress_test

# Record with perf (99 Hz sampling, DWARF call graphs)
perf record -F 99 --call-graph dwarf -o perf.data target/profiling/stress_test

# Generate text report
perf report --stdio -n --no-children | less

# Annotate specific functions
perf annotate --stdio zeropool::pool::BufferPool::get

# Interactive TUI
perf report
```

**Note**: `perf` provides much more detail than flamegraph, including:
- Assembly-level annotations showing which instructions are hot
- Accurate call graphs with sample counts
- Kernel vs userspace breakdown
- Cache miss analysis (with additional events)

See `PERF_ANALYSIS.md` for detailed analysis results.

## Example Workflow

```bash
# Profile baseline
cargo flamegraph --profile profiling --bin stress_test
mv flamegraph.svg baseline.svg

# Make optimization changes to zeropool

# Profile after changes
cargo flamegraph --profile profiling --bin stress_test
mv flamegraph.svg optimized.svg

# Compare the two SVGs
```
