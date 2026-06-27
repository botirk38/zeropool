# ZeroPool Scripts

This directory is a small uv-managed Python project for repository maintenance scripts.

## Benchmark Charts

Generate the checked-in README charts from the benchmark numbers currently documented in `README.md`:

```bash
uv run python bench.py --current --charts
```

Run Criterion and generate charts from `target/criterion`:

```bash
uv run python bench.py --filter realistic_write_mt --charts
```

Generate charts from an existing Criterion output directory without rerunning benchmarks:

```bash
uv run python bench.py --no-run --charts --criterion-dir ../target/criterion --out ../docs/assets
```

Supported chart inputs today:

- `realistic_write_mt/*/*/base/estimates.json`
- `hot_path/*/65536/base/estimates.json`

The chart generator uses Matplotlib and writes static SVG files suitable for GitHub README rendering.
