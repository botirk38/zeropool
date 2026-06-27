from __future__ import annotations

import argparse
import json
import subprocess
from dataclasses import dataclass
from pathlib import Path

import matplotlib

matplotlib.use("Agg")

import matplotlib.pyplot as plt
from matplotlib.ticker import FuncFormatter


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
    "zeropool": "#5B5FEF",
    "vec": "#EF4444",
    "opool": "#06B6D4",
    "object_pool": "#F59E0B",
    "bytes": "#64748B",
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
        ("Vec", 44.5, PALETTE["vec"]),
        ("bytes", 45.8, PALETTE["bytes"]),
    ],
    output="hot-path.svg",
    unit="ns",
)


def main() -> None:
    parser = argparse.ArgumentParser(
        prog="bench.py",
        description="Run Criterion benchmarks and optionally generate SVG charts.",
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
        ("Vec", "vec_capacity", PALETTE["vec"]),
        ("bytes", "bytes", PALETTE["bytes"]),
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
    plt.rcParams.update(rc_params())
    fig, ax = plt.subplots(figsize=(11.2, 6.2), constrained_layout=True)
    fig.patch.set_facecolor("#ffffff")
    ax.set_facecolor("#ffffff")

    x = [int(value) for value in chart.x_values]
    for series in chart.series:
        linewidth = 3.2 if series.name == "ZeroPool" else 2.2
        zorder = 5 if series.name == "ZeroPool" else 3
        ax.plot(
            x,
            series.values,
            marker="o",
            markersize=7,
            linewidth=linewidth,
            color=series.color,
            label=series.name,
            zorder=zorder,
        )
        for point_x, point_y in zip(x, series.values, strict=True):
            ax.annotate(
                f"{format_value(point_y)} {chart.unit}",
                (point_x, point_y),
                xytext=(0, 9),
                textcoords="offset points",
                ha="center",
                fontsize=8.5,
                color="#334155",
            )

    ax.set_yscale("log")
    ax.set_xticks(x)
    ax.set_xlabel(chart.x_label)
    ax.set_ylabel(f"{chart.y_label} ({chart.unit}, log scale)")
    ax.yaxis.set_major_formatter(FuncFormatter(lambda value, _: format_value(value)))
    ax.grid(axis="y", color="#E2E8F0", linewidth=1.0)
    ax.grid(axis="x", visible=False)
    ax.spines[["top", "right"]].set_visible(False)
    ax.spines[["left", "bottom"]].set_color("#CBD5E1")
    ax.tick_params(colors="#475569")
    ax.legend(
        loc="upper left",
        bbox_to_anchor=(0, 1.02),
        ncols=4,
        frameon=False,
        fontsize=10,
        handlelength=2.8,
    )
    add_titles(fig, chart.title, chart.subtitle)
    add_footer(fig, "Source: README benchmark snapshot. Detailed methodology in BENCHMARKS.md.")
    fig.savefig(path, format="svg", bbox_inches="tight", metadata={"Date": None})
    plt.close(fig)
    clean_file(path)


def write_bar_chart(chart: BarChart, path: Path) -> None:
    plt.rcParams.update(rc_params())
    fig, ax = plt.subplots(figsize=(11.2, 5.6), constrained_layout=True)
    fig.patch.set_facecolor("#ffffff")
    ax.set_facecolor("#ffffff")

    labels = [label for label, _, _ in chart.bars]
    values = [value for _, value, _ in chart.bars]
    colors = [color for _, _, color in chart.bars]
    y = range(len(labels))
    ax.barh(y, values, color=colors, height=0.56)
    ax.set_yticks(y, labels)
    ax.invert_yaxis()
    ax.set_xlabel(f"{chart.y_label} ({chart.unit})")
    ax.grid(axis="x", color="#E2E8F0", linewidth=1.0)
    ax.grid(axis="y", visible=False)
    ax.spines[["top", "right", "left"]].set_visible(False)
    ax.spines["bottom"].set_color("#CBD5E1")
    ax.tick_params(axis="x", colors="#475569")
    ax.tick_params(axis="y", colors="#0F172A", length=0)

    max_value = max(values)
    for index, value in enumerate(values):
        ax.text(
            value + max_value * 0.025,
            index,
            f"{format_value(value)} {chart.unit}",
            va="center",
            ha="left",
            fontsize=10,
            color="#334155",
            fontweight="bold",
        )

    ax.set_xlim(0, max_value * 1.18)
    add_titles(fig, chart.title, chart.subtitle)
    add_footer(fig, "Source: README benchmark snapshot. Detailed methodology in BENCHMARKS.md.")
    fig.savefig(path, format="svg", bbox_inches="tight", metadata={"Date": None})
    plt.close(fig)
    clean_file(path)


def rc_params() -> dict[str, object]:
    return {
        "font.family": "sans-serif",
        "font.sans-serif": ["Inter", "DejaVu Sans", "Arial", "sans-serif"],
        "axes.labelcolor": "#475569",
        "axes.labelsize": 11,
        "axes.titleweight": "bold",
        "xtick.labelsize": 10,
        "ytick.labelsize": 10,
        "svg.fonttype": "none",
    }


def add_titles(fig: plt.Figure, title: str, subtitle: str) -> None:
    fig.suptitle(title, x=0.01, y=1.04, ha="left", fontsize=22, fontweight="bold", color="#0F172A")
    fig.text(0.01, 0.985, subtitle, ha="left", va="top", fontsize=11, color="#64748B")


def add_footer(fig: plt.Figure, text: str) -> None:
    fig.text(0.01, -0.015, text, ha="left", va="bottom", fontsize=8.5, color="#94A3B8")


def format_value(value: float) -> str:
    if value >= 100:
        return f"{value:,.0f}"
    if value >= 10:
        return f"{value:.1f}".rstrip("0").rstrip(".")
    return f"{value:.2f}".rstrip("0").rstrip(".")


def clean_file(path: Path) -> None:
    svg = path.read_text(encoding="utf-8")
    path.write_text("\n".join(line.rstrip() for line in svg.splitlines()) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
