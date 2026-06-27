from __future__ import annotations

import argparse
import json
import math
import subprocess
from dataclasses import dataclass
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_OUT = REPO_ROOT / "docs" / "assets"


@dataclass(frozen=True)
class Series:
    name: str
    values: list[float]
    color: str


@dataclass(frozen=True)
class LineChart:
    title: str
    subtitle: str
    x_label: str
    y_label: str
    x_values: list[str]
    series: list[Series]
    output: str
    unit: str


@dataclass(frozen=True)
class BarChart:
    title: str
    subtitle: str
    y_label: str
    bars: list[tuple[str, float, str]]
    output: str
    unit: str


PALETTE = {
    "zeropool": "#7c3aed",
    "vec": "#ef4444",
    "opool": "#06b6d4",
    "object_pool": "#f59e0b",
    "bytes": "#64748b",
}


CURRENT_LINE = LineChart(
    title="Realistic 64 KiB Write",
    subtitle="200 ops/thread, lower is better",
    x_label="Threads",
    y_label="Latency",
    x_values=["2", "4", "8", "16"],
    series=[
        Series("ZeroPool", [35, 54, 89, 157], PALETTE["zeropool"]),
        Series("Vec", [272, 299, 388, 624], PALETTE["vec"]),
        Series("opool", [40, 84, 199, 371], PALETTE["opool"]),
        Series("object_pool", [45, 102, 295, 1476], PALETTE["object_pool"]),
    ],
    output="realistic-write-mt.svg",
    unit="us",
)


CURRENT_HOT_PATH = BarChart(
    title="Single-Thread Hot Path",
    subtitle="64 KiB alloc/drop, no page writes, lower is better",
    y_label="Time",
    bars=[
        ("opool", 15.2, PALETTE["opool"]),
        ("ZeroPool", 20.2, PALETTE["zeropool"]),
        ("object_pool", 21.1, PALETTE["object_pool"]),
        ("bytes", 45.8, PALETTE["bytes"]),
        ("Vec", 44.5, PALETTE["vec"]),
    ],
    output="hot-path.svg",
    unit="ns",
)


def main() -> None:
    parser = argparse.ArgumentParser(
        prog="bench.py",
        description="Run Criterion benchmarks and optionally generate polished SVG charts.",
    )
    parser.add_argument(
        "--filter",
        default="",
        help="Criterion filter, for example realistic_write_mt",
    )
    parser.add_argument(
        "--features",
        default="bench",
        help="Cargo features to enable; pass an empty string for none",
    )
    parser.add_argument(
        "--charts",
        action="store_true",
        help="generate SVG charts after running benchmarks",
    )
    parser.add_argument(
        "--current",
        action="store_true",
        help="use checked-in README benchmark numbers instead of running cargo bench",
    )
    parser.add_argument(
        "--no-run",
        action="store_true",
        help="do not run cargo bench; only read --criterion-dir when --charts is set",
    )
    parser.add_argument(
        "--criterion-dir",
        type=Path,
        default=REPO_ROOT / "target" / "criterion",
        help="Criterion output directory to read when generating charts",
    )
    parser.add_argument(
        "--out",
        type=Path,
        default=DEFAULT_OUT,
        help="output directory for generated charts",
    )

    args = parser.parse_args()

    if args.current:
        if not args.charts:
            raise SystemExit("--current requires --charts")
        write_current(args.out)
        return

    if args.no_run and not args.charts:
        raise SystemExit("--no-run requires --charts")

    if not args.no_run:
        run_bench(args.filter, args.features)
    if args.charts:
        write_from_criterion(args.criterion_dir, args.out)


def write_current(out_dir: Path) -> None:
    out_dir.mkdir(parents=True, exist_ok=True)
    write_line_chart(CURRENT_LINE, out_dir / CURRENT_LINE.output)
    write_bar_chart(CURRENT_HOT_PATH, out_dir / CURRENT_HOT_PATH.output)


def write_from_criterion(criterion_dir: Path, out_dir: Path) -> None:
    criterion_dir = criterion_dir.resolve()
    out_dir.mkdir(parents=True, exist_ok=True)

    line = load_realistic_write_mt(criterion_dir)
    hot_path = load_hot_path(criterion_dir)

    if line is None and hot_path is None:
        raise SystemExit(f"no supported Criterion results found under {criterion_dir}")

    if line is not None:
        write_line_chart(line, out_dir / line.output)
    if hot_path is not None:
        write_bar_chart(hot_path, out_dir / hot_path.output)


def run_bench(filter_text: str, features: str) -> None:
    command = ["cargo", "bench"]
    if features:
        command.extend(["--features", features])
    if filter_text:
        command.extend(["--", filter_text])
    subprocess.run(command, cwd=REPO_ROOT, check=True)


def load_realistic_write_mt(root: Path) -> LineChart | None:
    group = root / "realistic_write_mt"
    names = [
        ("ZeroPool", "zeropool", PALETTE["zeropool"]),
        ("Vec", "vec_alloc", PALETTE["vec"]),
        ("opool", "opool", PALETTE["opool"]),
        ("object_pool", "object_pool", PALETTE["object_pool"]),
    ]
    threads = ["2", "4", "8", "16"]
    series: list[Series] = []
    for label, slug, color in names:
        values = []
        for thread in threads:
            estimate = read_mean_ns(group / slug / thread / "base" / "estimates.json")
            if estimate is None:
                break
            values.append(estimate / 1_000.0)
        if len(values) == len(threads):
            series.append(Series(label, values, color))

    if not series:
        return None

    return LineChart(
        title="Realistic 64 KiB Write",
        subtitle="200 ops/thread from Criterion estimates, lower is better",
        x_label="Threads",
        y_label="Latency",
        x_values=threads,
        series=series,
        output="realistic-write-mt.svg",
        unit="us",
    )


def load_hot_path(root: Path) -> BarChart | None:
    group = root / "hot_path"
    size = "65536"
    specs = [
        ("opool", "opool", PALETTE["opool"]),
        ("ZeroPool", "zeropool", PALETTE["zeropool"]),
        ("object_pool", "object_pool", PALETTE["object_pool"]),
        ("bytes", "bytes", PALETTE["bytes"]),
        ("Vec", "vec_capacity", PALETTE["vec"]),
    ]
    bars = []
    for label, slug, color in specs:
        estimate = read_mean_ns(group / slug / size / "base" / "estimates.json")
        if estimate is not None:
            bars.append((label, estimate, color))

    if not bars:
        return None

    return BarChart(
        title="Single-Thread Hot Path",
        subtitle="64 KiB alloc/drop from Criterion estimates, lower is better",
        y_label="Time",
        bars=bars,
        output="hot-path.svg",
        unit="ns",
    )


def read_mean_ns(path: Path) -> float | None:
    if not path.exists():
        return None
    with path.open("r", encoding="utf-8") as file:
        data = json.load(file)
    return float(data["mean"]["point_estimate"])


def write_line_chart(chart: LineChart, path: Path) -> None:
    width = 1120
    height = 620
    left = 86
    right = 260
    top = 108
    bottom = 92
    plot_width = width - left - right
    plot_height = height - top - bottom
    max_value = nice_max(max(value for series in chart.series for value in series.values))
    ticks = [max_value * i / 4 for i in range(5)]

    def x_at(index: int) -> float:
        if len(chart.x_values) == 1:
            return left + plot_width / 2
        return left + index * plot_width / (len(chart.x_values) - 1)

    def y_at(value: float) -> float:
        return top + plot_height - (value / max_value) * plot_height

    grid = []
    for tick in ticks:
        y = y_at(tick)
        grid.append(
            f'<line x1="{left}" y1="{y:.1f}" x2="{left + plot_width}" y2="{y:.1f}" class="grid" />'
        )
        grid.append(
            f'<text x="{left - 18}" y="{y + 5:.1f}" class="axis-label" text-anchor="end">{format_tick(tick)} {chart.unit}</text>'
        )

    x_labels = []
    for index, label in enumerate(chart.x_values):
        x = x_at(index)
        x_labels.append(
            f'<text x="{x:.1f}" y="{height - 46}" class="axis-label" text-anchor="middle">{label}</text>'
        )

    series_svg = []
    legend = []
    for series_index, series in enumerate(chart.series):
        points = [(x_at(i), y_at(v)) for i, v in enumerate(series.values)]
        path_data = " ".join(
            f'{"M" if i == 0 else "L"} {x:.1f} {y:.1f}' for i, (x, y) in enumerate(points)
        )
        area_data = (
            f"M {points[0][0]:.1f} {top + plot_height:.1f} "
            + " ".join(f"L {x:.1f} {y:.1f}" for x, y in points)
            + f" L {points[-1][0]:.1f} {top + plot_height:.1f} Z"
        )
        opacity = 0.10 if series.name == "ZeroPool" else 0.045
        series_svg.append(
            f'<path d="{area_data}" fill="{series.color}" opacity="{opacity}" />'
        )
        series_svg.append(
            f'<path d="{path_data}" fill="none" stroke="{series.color}" stroke-width="4" stroke-linecap="round" stroke-linejoin="round" />'
        )
        for x, y in points:
            series_svg.append(
                f'<circle cx="{x:.1f}" cy="{y:.1f}" r="6" fill="#0f172a" stroke="{series.color}" stroke-width="4" />'
            )
        legend_y = top + 24 + series_index * 34
        legend.append(
            f'<circle cx="{left + plot_width + 56}" cy="{legend_y}" r="6" fill="{series.color}" />'
            f'<text x="{left + plot_width + 74}" y="{legend_y + 5}" class="legend">{escape(series.name)}</text>'
        )

    svg = svg_shell(
        width,
        height,
        f"""
        <rect x="0" y="0" width="{width}" height="{height}" rx="30" fill="url(#bg)" />
        <circle cx="940" cy="60" r="190" fill="#7c3aed" opacity="0.16" />
        <circle cx="1040" cy="510" r="210" fill="#06b6d4" opacity="0.12" />
        <rect x="24" y="24" width="{width - 48}" height="{height - 48}" rx="24" fill="#0f172a" opacity="0.76" stroke="#334155" />
        <text x="56" y="66" class="title">{escape(chart.title)}</text>
        <text x="56" y="92" class="subtitle">{escape(chart.subtitle)}</text>
        {''.join(grid)}
        <line x1="{left}" y1="{top}" x2="{left}" y2="{top + plot_height}" class="axis" />
        <line x1="{left}" y1="{top + plot_height}" x2="{left + plot_width}" y2="{top + plot_height}" class="axis" />
        {''.join(x_labels)}
        <text x="{left + plot_width / 2:.1f}" y="{height - 18}" class="axis-title" text-anchor="middle">{escape(chart.x_label)}</text>
        <text x="28" y="{top + plot_height / 2:.1f}" class="axis-title" transform="rotate(-90 28 {top + plot_height / 2:.1f})" text-anchor="middle">{escape(chart.y_label)}</text>
        {''.join(series_svg)}
        <rect x="{left + plot_width + 28}" y="{top - 8}" width="198" height="{len(chart.series) * 34 + 28}" rx="16" fill="#020617" opacity="0.44" stroke="#334155" />
        {''.join(legend)}
        """,
    )
    path.write_text(clean_svg(svg), encoding="utf-8")


def write_bar_chart(chart: BarChart, path: Path) -> None:
    width = 1120
    height = 560
    left = 90
    right = 62
    top = 112
    bottom = 108
    plot_width = width - left - right
    plot_height = height - top - bottom
    max_value = nice_max(max(value for _, value, _ in chart.bars))
    bar_gap = 26
    bar_width = (plot_width - bar_gap * (len(chart.bars) - 1)) / len(chart.bars)
    ticks = [max_value * i / 4 for i in range(5)]

    grid = []
    for tick in ticks:
        y = top + plot_height - (tick / max_value) * plot_height
        grid.append(
            f'<line x1="{left}" y1="{y:.1f}" x2="{left + plot_width}" y2="{y:.1f}" class="grid" />'
        )
        grid.append(
            f'<text x="{left - 18}" y="{y + 5:.1f}" class="axis-label" text-anchor="end">{format_tick(tick)} {chart.unit}</text>'
        )

    bars = []
    for index, (label, value, color) in enumerate(chart.bars):
        x = left + index * (bar_width + bar_gap)
        bar_height = (value / max_value) * plot_height
        y = top + plot_height - bar_height
        bars.append(
            f'<rect x="{x:.1f}" y="{y:.1f}" width="{bar_width:.1f}" height="{bar_height:.1f}" rx="14" fill="{color}" />'
            f'<rect x="{x:.1f}" y="{y:.1f}" width="{bar_width:.1f}" height="{bar_height:.1f}" rx="14" fill="url(#barSheen)" opacity="0.38" />'
            f'<text x="{x + bar_width / 2:.1f}" y="{y - 14:.1f}" class="value" text-anchor="middle">{format_tick(value)} {chart.unit}</text>'
            f'<text x="{x + bar_width / 2:.1f}" y="{height - 48}" class="axis-label" text-anchor="middle">{escape(label)}</text>'
        )

    svg = svg_shell(
        width,
        height,
        f"""
        <rect x="0" y="0" width="{width}" height="{height}" rx="30" fill="url(#bg)" />
        <circle cx="150" cy="450" r="180" fill="#7c3aed" opacity="0.14" />
        <circle cx="1010" cy="80" r="190" fill="#06b6d4" opacity="0.12" />
        <rect x="24" y="24" width="{width - 48}" height="{height - 48}" rx="24" fill="#0f172a" opacity="0.76" stroke="#334155" />
        <text x="56" y="66" class="title">{escape(chart.title)}</text>
        <text x="56" y="92" class="subtitle">{escape(chart.subtitle)}</text>
        {''.join(grid)}
        <line x1="{left}" y1="{top}" x2="{left}" y2="{top + plot_height}" class="axis" />
        <line x1="{left}" y1="{top + plot_height}" x2="{left + plot_width}" y2="{top + plot_height}" class="axis" />
        <text x="28" y="{top + plot_height / 2:.1f}" class="axis-title" transform="rotate(-90 28 {top + plot_height / 2:.1f})" text-anchor="middle">{escape(chart.y_label)}</text>
        {''.join(bars)}
        """,
    )
    path.write_text(clean_svg(svg), encoding="utf-8")


def svg_shell(width: int, height: int, body: str) -> str:
    return f"""<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}" role="img">
    <defs>
        <linearGradient id="bg" x1="0" y1="0" x2="1" y2="1">
            <stop offset="0%" stop-color="#020617" />
            <stop offset="58%" stop-color="#111827" />
            <stop offset="100%" stop-color="#1e1b4b" />
        </linearGradient>
        <linearGradient id="barSheen" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stop-color="#ffffff" />
            <stop offset="100%" stop-color="#ffffff" stop-opacity="0" />
        </linearGradient>
        <style>
            text {{ font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }}
            .title {{ fill: #f8fafc; font-size: 30px; font-weight: 800; letter-spacing: -0.03em; }}
            .subtitle {{ fill: #94a3b8; font-size: 15px; font-weight: 500; }}
            .axis-label {{ fill: #cbd5e1; font-size: 13px; font-weight: 600; }}
            .axis-title {{ fill: #94a3b8; font-size: 13px; font-weight: 800; letter-spacing: 0.08em; text-transform: uppercase; }}
            .legend {{ fill: #e2e8f0; font-size: 14px; font-weight: 700; }}
            .value {{ fill: #f8fafc; font-size: 14px; font-weight: 800; }}
            .grid {{ stroke: #334155; stroke-width: 1; opacity: 0.65; }}
            .axis {{ stroke: #64748b; stroke-width: 1.4; }}
        </style>
    </defs>
    {body}
</svg>
"""


def clean_svg(svg: str) -> str:
    return "\n".join(line.rstrip() for line in svg.splitlines()) + "\n"


def nice_max(value: float) -> float:
    if value <= 0:
        return 1
    exponent = math.floor(math.log10(value))
    fraction = value / 10**exponent
    if fraction <= 1:
        nice = 1
    elif fraction <= 2:
        nice = 2
    elif fraction <= 5:
        nice = 5
    else:
        nice = 10
    return nice * 10**exponent


def format_tick(value: float) -> str:
    if value >= 100:
        return f"{value:,.0f}"
    if value >= 10:
        return f"{value:.1f}".rstrip("0").rstrip(".")
    return f"{value:.2f}".rstrip("0").rstrip(".")


def escape(value: str) -> str:
    return (
        value.replace("&", "&amp;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
        .replace('"', "&quot;")
    )


if __name__ == "__main__":
    main()
