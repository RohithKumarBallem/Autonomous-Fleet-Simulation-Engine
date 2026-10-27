#!/usr/bin/env python3
"""Draws the two README figures from the raw JSON in bench/results into docs/img/. Nothing here is hand-typed.

Each SVG carries its own surface, so it reads the same on GitHub's light and dark themes.
"""
import glob, json, os, statistics
from xml.sax.saxutils import escape

HERE = os.path.dirname(__file__)
R = os.path.join(HERE, "results")
OUT = os.path.join(HERE, "..", "docs", "img")

SURFACE, BORDER, GRID, AXIS = "#fcfcfb", "#e1e0d9", "#e1e0d9", "#c3c2b7"
INK, INK2, MUTED = "#0b0b0b", "#52514e", "#898781"
BLUE, ORANGE = "#2a78d6", "#eb6834"  # categorical slots 1 and 2, validated for CVD and contrast
FONT = 'system-ui, -apple-system, "Segoe UI", sans-serif'
W, LEFT, RIGHT, TOP, BOTTOM = 760, 64, 28, 104, 72


def bar(x, y, w, h, r=4):
    """Bar anchored to the baseline with only its data end rounded."""
    r = min(r, w / 2, h)
    return f"M{x:.1f},{y + h:.1f} L{x:.1f},{y + r:.1f} Q{x:.1f},{y:.1f} {x + r:.1f},{y:.1f} L{x + w - r:.1f},{y:.1f} Q{x + w:.1f},{y:.1f} {x + w:.1f},{y + r:.1f} L{x + w:.1f},{y + h:.1f} Z"


def text(x, y, s, size=12, fill=INK2, anchor="start", weight=400):
    return f'<text x="{x:.1f}" y="{y:.1f}" font-size="{size}" fill="{fill}" text-anchor="{anchor}" font-weight="{weight}">{escape(s)}</text>'


def frame(title, subtitle, height, desc):
    return [
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {height}" width="{W}" height="{height}" font-family=\'{FONT}\' role="img" aria-labelledby="t d">',
        f"<title id=\"t\">{escape(title)}</title><desc id=\"d\">{escape(desc)}</desc>",
        f'<rect x="0.5" y="0.5" width="{W - 1}" height="{height - 1}" rx="10" fill="{SURFACE}" stroke="{BORDER}"/>',
        text(24, 36, title, 18, INK, weight=600),
        text(24, 58, subtitle, 12.5, INK2),
    ]


def y_axis(top, bottom, ymax, step, unit=""):
    out = []
    for v in range(0, int(ymax) + 1, step):
        y = bottom - (bottom - top) * v / ymax
        out.append(f'<line x1="{LEFT}" x2="{W - RIGHT}" y1="{y:.1f}" y2="{y:.1f}" stroke="{AXIS if v == 0 else GRID}" stroke-width="1"/>')
        out.append(text(LEFT - 10, y + 4, f"{v}{unit}", 11, MUTED, "end"))
    return out


def throughput():
    runs = [json.load(open(f)) for f in sorted(glob.glob(os.path.join(R, "scaling-run*.json")))]
    n = runs[0]["scenarios"]
    threads = [row["threads"] for row in runs[0]["rows"]]
    secs = [statistics.median(r["rows"][k]["seconds"] for r in runs) for k in range(len(threads))]
    sps = [n / s for s in secs]
    speed = [secs[0] / s for s in secs]

    height, ymax = 372, 450
    top, bottom = TOP - 14, height - BOTTOM
    svg = frame("Simulation throughput scales with threads",
                f"{n} scenarios of 500 vehicles; median of {len(runs)} runs; 6 cores / 12 threads; identical results throughout",
                height, f"Bar chart of scenarios per second at {', '.join(map(str, threads))} threads: "
                + ", ".join(f"{v:.0f}" for v in sps) + ".")
    svg += y_axis(top, bottom, ymax, 100)
    svg.append(text(24, top - 12, "scenarios / second", 11, MUTED))
    svg.append(text(LEFT - 10, bottom + 40, "speed-up", 11, MUTED, "end"))
    slot = (W - LEFT - RIGHT) / len(threads)
    bw = 72
    for k, t in enumerate(threads):
        cx = LEFT + slot * (k + 0.5)
        h = (bottom - top) * sps[k] / ymax
        svg.append(f'<path d="{bar(cx - bw / 2, bottom - h, bw, h)}" fill="{BLUE}"/>')
        svg.append(text(cx, bottom - h - 8, f"{sps[k]:.0f}", 13, INK, "middle", 600))
        svg.append(text(cx, bottom + 22, f"{t} thread{'s' if t > 1 else ''}", 12.5, INK2, "middle"))
        svg.append(text(cx, bottom + 40, f"{speed[k]:.2f}x", 12, MUTED, "middle"))
    svg.append("</svg>")
    return "\n".join(svg)


def cycle():
    cyc = sorted((json.load(open(f)) for f in glob.glob(os.path.join(R, "cycle-seeds*.json"))), key=lambda c: c["workload"]["scenarios"])
    height, ymax = 410, 80
    top, bottom = TOP, height - BOTTOM - 8
    svg = frame("Validation cycle time: sequential baseline vs Airflow pipeline",
                "Telemetry batch (1.1 M records) to published report; median seconds; identical results in both arms",
                height, "Grouped bar chart of cycle time in seconds per suite size. "
                + "; ".join(f"{c['workload']['scenarios']} scenarios: baseline {c['baseline_median_s']:.1f} s, pipeline {c['pipeline_median_s']:.1f} s"
                            for c in cyc) + ".")
    # legend
    lx = 24
    for label, color in (("Sequential baseline (1 worker)", ORANGE), ("Airflow pipeline (4 shards, all cores)", BLUE)):
        svg.append(f'<rect x="{lx}" y="72" width="12" height="12" rx="2" fill="{color}"/>')
        svg.append(text(lx + 18, 82, label, 12, INK2))
        lx += 18 + 7.0 * len(label) + 22
    svg += y_axis(top, bottom, ymax, 20, " s")
    svg.append(text(LEFT - 10, bottom + 42, "change", 11, MUTED, "end"))
    slot = (W - LEFT - RIGHT) / len(cyc)
    bw, gap = 46, 2
    for k, c in enumerate(cyc):
        cx = LEFT + slot * (k + 0.5)
        for j, (val, color) in enumerate(((c["baseline_median_s"], ORANGE), (c["pipeline_median_s"], BLUE))):
            x = cx - bw - gap / 2 if j == 0 else cx + gap / 2
            h = (bottom - top) * val / ymax
            svg.append(f'<path d="{bar(x, bottom - h, bw, h)}" fill="{color}"/>')
            svg.append(text(x + bw / 2, bottom - h - 6, f"{val:.1f}", 11.5, INK2, "middle"))
        pct = c["cycle_time_reduction_pct"]
        change = f"−{pct:.1f}%" if pct > 0 else f"+{-pct:.1f}%"
        svg.append(text(cx, bottom + 22, f"{c['workload']['scenarios']:,} scenarios", 12.5, INK2, "middle"))
        svg.append(text(cx, bottom + 42, change, 13, INK, "middle", 600))
    svg.append("</svg>")
    return "\n".join(svg)


if __name__ == "__main__":
    os.makedirs(OUT, exist_ok=True)
    for name, fn in (("throughput.svg", throughput), ("cycle-time.svg", cycle)):
        with open(os.path.join(OUT, name), "w", encoding="utf-8") as f:
            f.write(fn() + "\n")
        print("wrote", os.path.normpath(os.path.join(OUT, name)))
