# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.0] - 2025-01-XX

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
