# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.7.0] - 2026-06-27

### Changed
- **BREAKING**: `ZeroPool::alloc()` now returns zero-initialized readable buffers.
- **BREAKING**: `Allocator::allocate()` must return zero-initialized buffers with `len() >= capacity`.
- `HeapAllocator` now uses `vec![0; capacity]` for safe default allocations.
- Unsafe lint policy now allows documented unsafe code while denying undocumented unsafe blocks and missing safety docs.

### Added
- `ZeroPool::alloc_uninit()` for full-overwrite workloads that need the non-zeroing fast path.
- `BufUninit`, which exposes maybe-uninitialized bytes without safe readable access.
- `BufUninit::write_from_slice()`, `BufUninit::as_uninit_mut()`, and unsafe `BufUninit::assume_init()`.
- Criterion coverage for the explicit uninitialized allocation path.

### Fixed
- Prevented recycled buffer contents from being exposed through the safe `alloc()` API.
- Avoided mutating buffer length during pinned-memory locking.

## [0.3.0] - 2025-11-23

### Changed
- **BREAKING**: `BufferPool::get()` now returns `PooledBuffer` instead of `Vec<u8>`
  - `PooledBuffer` implements `Deref<Target = Vec<u8>>` for transparent access
  - Buffers automatically return to pool when dropped (RAII pattern)
  - No more manual `pool.put()` calls needed
- **BREAKING**: `BufferPool::put()` is now internal-only (`pub(crate)`)
  - Users should rely on automatic return via `Drop`
  - For explicit early return, use `drop(buffer)`

### Added
- New `PooledBuffer` type providing automatic buffer return via Drop trait
- `PooledBuffer` implements:
  - `Deref<Target = Vec<u8>>` and `DerefMut` for transparent Vec API access
  - `AsRef<[u8]>` and `AsMut<[u8]>` for slice conversions
  - `Debug` with buffer length and capacity
  - `Send` and `Sync` for thread safety
- Configurable eviction policies (LIFO, ClockPro) for fine-tuned performance
- Profiling binaries for performance analysis:
  - `ml_checkpoint_loader`: ML model loading simulation
  - `network_server`: Multi-threaded network server simulation
  - `file_pipeline`: File processing pipeline simulation
  - `stress_test`: High-stress workload testing
- Modularized codebase with separate modules for buffer, config, pool, and utilities

### Fixed
- Removed all unsafe code blocks for improved safety
- Fixed all clippy warnings and formatting issues
- Improved code organization and documentation

### Migration Guide

#### Before (v0.2.x):
```rust
let buf = pool.get(1024);
// use buf
pool.put(buf); // Manual return
```

#### After (v0.3.0):
```rust
let buf = pool.get(1024);
// use buf
// Auto-returns on drop!

// For explicit early return:
drop(buf);
```

## [0.2.0] - Previous Release

See git history for earlier changes.
