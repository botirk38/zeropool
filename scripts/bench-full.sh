#!/usr/bin/env bash
# Run the full ZeroPool benchmark suite with the comparison crates enabled.
#
# Usage:
#   scripts/bench-full.sh
#   ZEROPOOL_SCALE_OPS=100000 scripts/bench-full.sh
#
# Each group is run separately so the output stays readable. Groups that
# require the `bench` feature are flagged.

set -euo pipefail

cd "$(dirname "$0")/.."

# 1. Hot path: requires bench feature for opool/object_pool/bytes.
cargo bench --features bench -- hot_path

# 2. Realistic single-thread and multi-thread: requires bench feature.
cargo bench --features bench -- realistic_write_st
cargo bench --features bench -- realistic_write_mt

# 3. Sustained throughput: requires bench feature.
ZEROPOOL_SCALE_OPS="${ZEROPOOL_SCALE_OPS:-10000}" \
    cargo bench --features bench -- scale_sustained

# 4. Mixed sizes: requires bench feature.
cargo bench --features bench -- mixed_uniform
cargo bench --features bench -- mixed_zipf

# 5. Burst: requires bench feature.
cargo bench --features bench -- burst

# 6. Contention: ZeroPool only, no comparison crates needed.
cargo bench -- contention

# 7. Stats overhead: ZeroPool only.
cargo bench -- stats_overhead

echo
echo "Done. Estimates written to target/criterion/. See BENCHMARKS.md for"
echo "how to read the JSON files and how to update the README tables."
