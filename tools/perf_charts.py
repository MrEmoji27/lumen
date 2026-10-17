"""Lumen's perf charts: measured numbers in, SVG files out (light and dark).

Run: python tools/perf_charts.py   ->   docs/img/<name>-light.svg, docs/img/<name>-dark.svg

Every number here is a measurement from docs/perf.md (or the run named beside it).
Change a number there, change it here, run this again. No dependencies.
"""

from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "docs" / "img"

# Chart tokens: a validated two-slot categorical pair plus chrome, light and dark.
THEMES = {
    "light": {
        "surface": "#fcfcfb", "ink": "#0b0b0b", "ink2": "#52514e", "muted": "#898781",
        "grid": "#e1e0d9", "axis": "#c3c2b7", "s1": "#2a78d6", "s2": "#eb6834", "quiet": "#b9b8b0",
        "ring": "rgba(11,11,11,0.10)",
    },
    "dark": {
        "surface": "#1a1a19", "ink": "#f0efec", "ink2": "#c3c2b7", "muted": "#898781",
        "grid": "#2c2c2a", "axis": "#383835", "s1": "#3987e5", "s2": "#d95926", "quiet": "#5a5955",
        "ring": "rgba(255,255,255,0.10)",
    },
}

FONT = 'system-ui, -apple-system, "Segoe UI", sans-serif'


def esc(s):
    return str(s).replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def svg(w, h, t, title, subtitle, body, label):
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" width="{w}" height="{h}" '
        f'role="img" aria-label="{esc(label)}" font-family=\'{FONT}\'>\n'
        f'<rect x="0.5" y="0.5" width="{w - 1}" height="{h - 1}" rx="12" fill="{t["surface"]}" stroke="{t["ring"]}"/>\n'
        f'<text x="28" y="38" font-size="17" font-weight="600" fill="{t["ink"]}">{esc(title)}</text>\n'
        f'<text x="28" y="60" font-size="12.5" fill="{t["ink2"]}">{esc(subtitle)}</text>\n'
        f"{body}</svg>\n"
    )


def text(x, y, s, t, size=12, color="ink2", anchor="start", weight=400):
    return (
        f'<text x="{x:.1f}" y="{y:.1f}" font-size="{size}" font-weight="{weight}" '
        f'fill="{t[color]}" text-anchor="{anchor}">{esc(s)}</text>\n'
    )


def column(x, y_top, w, y_base, fill, tip):
    """A bar standing on y_base: 4 px round at its data end, square at the baseline."""
    h = y_base - y_top
    r = min(4.0, h / 2, w / 2)
    if h <= 0:
        return ""
    d = (
        f"M{x:.1f},{y_base:.1f} L{x:.1f},{y_top + r:.1f} Q{x:.1f},{y_top:.1f} {x + r:.1f},{y_top:.1f} "
        f"L{x + w - r:.1f},{y_top:.1f} Q{x + w:.1f},{y_top:.1f} {x + w:.1f},{y_top + r:.1f} L{x + w:.1f},{y_base:.1f} Z"
    )
    return f'<path d="{d}" fill="{fill}"><title>{esc(tip)}</title></path>\n'


def hbar(x0, y, length, h, fill, tip):
    """A bar lying from x0: round at its data end, square at the baseline."""
    r = min(4.0, length / 2, h / 2)
    if length <= 0:
        return ""
    x1 = x0 + length
    d = (
        f"M{x0:.1f},{y:.1f} L{x1 - r:.1f},{y:.1f} Q{x1:.1f},{y:.1f} {x1:.1f},{y + r:.1f} "
        f"L{x1:.1f},{y + h - r:.1f} Q{x1:.1f},{y + h:.1f} {x1 - r:.1f},{y + h:.1f} L{x0:.1f},{y + h:.1f} Z"
    )
    return f'<path d="{d}" fill="{fill}"><title>{esc(tip)}</title></path>\n'


def gridlines(x0, x1, y_of, ticks, t, unit="", fmt=lambda v: f"{v:g}"):
    out = ""
    for v in ticks:
        y = y_of(v)
        out += f'<line x1="{x0}" x2="{x1}" y1="{y:.1f}" y2="{y:.1f}" stroke="{t["axis"] if v == 0 else t["grid"]}" stroke-width="1"/>\n'
        out += text(x0 - 10, y + 4, fmt(v) + unit, t, 11.5, "muted", "end")
    return out


def legend(x, y, items, t):
    out = ""
    for name, color in items:
        out += f'<rect x="{x}" y="{y - 9}" width="12" height="12" rx="3" fill="{t[color]}"/>\n'
        out += text(x + 18, y + 1, name, t, 12, "ink2")
        x += 26 + 7.2 * len(name)
    return out


# ------------------------------------------------------------------ the charts

def history(t):
    """Build history: what the native interface and the perf pass did."""
    steps = ["HTML interface", "Native interface", "Features added", "Perf pass"]
    wiki = [199, 118, 117, 91]
    idle = [144, 18, 17, 18]
    w, h = 780, 380
    x0, x1, yb, yt = 84, 640, 320, 96
    X = lambda i: x0 + 40 + i * (x1 - x0 - 80) / 3
    Y = lambda v: yb - v / 220 * (yb - yt)
    b = legend(440, 84, [("One Wikipedia tab", "s1"), ("Empty new tab", "s2")], t)
    b += gridlines(x0, x1, Y, [0, 50, 100, 150, 200], t, "")
    b += text(x0 - 10, yt - 14, "MB", t, 11.5, "muted", "end")
    for i, s in enumerate(steps):
        b += text(X(i), yb + 24, s, t, 12, "ink2", "middle")
    for vals, color, name in ((idle, "s2", "Empty new tab"), (wiki, "s1", "One Wikipedia tab")):
        pts = " ".join(f"{X(i):.1f},{Y(v):.1f}" for i, v in enumerate(vals))
        b += f'<polyline points="{pts}" fill="none" stroke="{t[color]}" stroke-width="2" stroke-linejoin="round" stroke-linecap="round"/>\n'
        for i, v in enumerate(vals):
            b += f'<circle cx="{X(i):.1f}" cy="{Y(v):.1f}" r="5" fill="{t[color]}" stroke="{t["surface"]}" stroke-width="2"><title>{esc(name)}, {steps[i]}: {v} MB</title></circle>\n'
        # Label the start and the end only.
        b += text(X(0), Y(vals[0]) - 12, f"{vals[0]} MB", t, 12, "ink", "middle", 600)
        b += text(X(3) + 12, Y(vals[3]) + 4, f"{vals[3]} MB", t, 12, "ink", "start", 600)
    return svg(w, h, t, "The native interface took the empty window from 144 MB to 18 MB",
               "Private memory of Lumen and its WebView2 processes, release builds, at each step of the build",
               b, "Empty new tab 144 to 18 MB; one Wikipedia tab 199 to 91 MB")


def idle(t):
    """Idle memory across builds: the regression and the fix."""
    builds = [
        ("15 Oct", 18.2), ("UI rework, wake fix,", 21.3), ("Pins, icons", 21.5),
        ("Customize", 22.6), ("0.2.0", 24.6), ("0.2.1", 18.5),
    ]
    sub2 = ["", "agent security", "", "", "", ""]
    w, h = 780, 396
    x0, x1, yb, yt = 84, 752, 310, 96
    Y = lambda v: yb - v / 30 * (yb - yt)
    slot = (x1 - x0) / len(builds)
    bw = 24
    b = gridlines(x0, x1, Y, [0, 10, 20, 30], t)
    b += text(x0 - 10, yt - 14, "MB", t, 11.5, "muted", "end")
    for i, (name, v) in enumerate(builds):
        cx = x0 + slot * (i + 0.5)
        color = t["s1"] if name == "0.2.1" else (t["s2"] if name == "0.2.0" else t["quiet"])
        b += column(cx - bw / 2, Y(v), bw, Y(0), color, f"{name}: {v} MB")
        b += text(cx, yb + 22, name, t, 12, "ink2", "middle")
        if sub2[i]:
            b += text(cx, yb + 38, sub2[i], t, 12, "ink2", "middle")
        if name in ("15 Oct", "0.2.0", "0.2.1"):
            b += text(cx, Y(v) - 10, f"{v} MB", t, 12.5, "ink", "middle", 600)
    b += text(x0, h - 14, "The 6 MB was one block: the ad matcher built in debug mode for the privacy report. 0.2.1 builds it lean.", t, 12, "muted")
    return svg(w, h, t, "Idle memory crept up 6 MB over three days, then came back",
               "lumen.exe private working set at idle, start page, headless, release builds, in build order",
               b, "Idle memory: 18.2 MB on 15 Oct, 24.6 MB at 0.2.0, 18.5 MB at 0.2.1")


def tabs40(t):
    """Forty tabs: processes and memory, two panels, one scale each."""
    w, h = 780, 380
    b = ""
    panels = [
        (40, 360, "Renderer processes", [("One per tab", 54, "quiet"), ("One per site", 23, "s1")], 60, [0, 20, 40, 60], "", lambda v: f"{v:g}"),
        (420, 740, "Memory, forty heavy tabs", [("Peak", 2.7, "quiet"), ("Settled, tabs asleep", 1.1, "s1")], 3, [0, 1, 2, 3], " GB", lambda v: f"{v:g}"),
    ]
    for px0, px1, name, bars, top, ticks, unit, fmt in panels:
        x0, x1, yb, yt = px0 + 44, px1, 316, 112
        Y = lambda v, top=top, yb=yb, yt=yt: yb - v / top * (yb - yt)
        b += text(px0 + 4, 92, name, t, 13, "ink", "start", 600)
        b += gridlines(x0, x1, Y, ticks, t, unit, fmt)
        slot = (x1 - x0) / len(bars)
        for i, (label, v, color) in enumerate(bars):
            cx = x0 + slot * (i + 0.5)
            b += column(cx - 12, Y(v), 24, Y(0), t[color], f"{label}: {v:g}{unit}")
            b += text(cx, Y(v) - 10, f"{v:g}{unit}", t, 12.5, "ink", "middle", 600)
            b += text(cx, yb + 22, label, t, 12, "ink2", "middle")
    b += text(28, h - 14, "Same forty tabs. Hidden tabs trim at 30 s, freeze at 5 min, unload at 30 min. CPU after settling: 0.4%.", t, 12, "muted")
    return svg(w, h, t, "Forty tabs: 31 fewer processes, and 1.1 GB once they sleep",
               "Release build, the same forty tabs before and after each change", b,
               "Renderer processes 54 to 23; forty heavy tabs peak 2.7 GB, settle at 1.1 GB")


def gpu(t):
    """Video: GPU work in-process against a separate GPU process. A trade, three panels."""
    w, h = 780, 390
    b = legend(28, 84, [("GPU in WebView2's main process", "s1"), ("Separate GPU process", "s2")], t)
    panels = [
        (28, "Private memory, MB", 639, 726, 800, [0, 400, 800], ""),
        (284, "CPU, % of one core", 122, 75, 150, [0, 50, 100, 150], ""),
        (540, "Dropped frames, %", 6.8, 0, 10, [0, 5, 10], ""),
    ]
    for px, name, a, s, top, ticks, unit in panels:
        x0, x1, yb, yt = px + 44, px + 228, 318, 132
        Y = lambda v, top=top, yb=yb, yt=yt: yb - v / top * (yb - yt)
        b += text(px, 116, name, t, 13, "ink", "start", 600)
        b += gridlines(x0, x1, Y, ticks, t, unit)
        cx = (x0 + x1) / 2
        for dx, v, color, who in ((-16, a, "s1", "in-process"), (16, s, "s2", "separate")):
            b += column(cx + dx - 12, Y(v), 24, Y(0), t[color], f"{name}, {who}: {v:g}{unit}")
            b += text(cx + dx, Y(v) - 8, f"{v:g}{unit}", t, 12, "ink", "middle", 600)
    b += text(28, h - 16, "Same 1080p video, two runs each, mean shown. In-process is the default; the setting trades about 90 MB for smoother video.", t, 12, "muted")
    return svg(w, h, t, "One GPU setting trades 90 MB of memory for smoother video",
               "A 1080p video in one tab, release build, WebView2's GPU work in its main process against in a process of its own", b,
               "In-process: 639 MB, 122% CPU, 6.8% dropped; separate: 726 MB, 75% CPU, 0% dropped")


def wakes(t):
    """The wake storm: how often the event loop woke, per minute."""
    rows = [
        ("A page rewriting its title every 10 ms, before", 5119, "quiet"),
        ("The same page, after", 254, "s1"),
        ("A download running (aria2 polled every 700 ms)", 165, "s1"),
        ("An idle tab, headless", 94, "s1"),
    ]
    w, h = 780, 330
    x0, x1 = 360, 700
    X = lambda v: x0 + v / 5500 * (x1 - x0)
    b = ""
    for v in (0, 2500, 5000):
        b += f'<line x1="{X(v):.1f}" x2="{X(v):.1f}" y1="92" y2="282" stroke="{t["axis"] if v == 0 else t["grid"]}" stroke-width="1"/>\n'
        b += text(X(v), 300, f"{v:,}", t, 11.5, "muted", "middle")
    for i, (label, v, color) in enumerate(rows):
        y = 104 + i * 46
        b += text(x0 - 14, y + 16, label, t, 12, "ink2", "end")
        b += hbar(x0, y, X(v) - x0, 22, t[color], f"{label}: {v:,} wakes a minute")
        b += text(X(v) + 8, y + 16, f"{v:,}", t, 12.5, "ink", "start", 600)
    b += text(28, h - 10, "Wakes of Lumen's event loop per minute, release build. The fix: a 50 ms floor on every wait, and title events Lumen never reads ignored.", t, 12, "muted")
    return svg(w, h, t, "A busy page used to wake Lumen 5,119 times a minute; now 254",
               "Event-loop wakes per minute", b, "Wakes per minute: 5,119 before, 254 after, 165 downloading, 94 idle")


CHARTS = {"history": history, "idle": idle, "tabs40": tabs40, "gpu": gpu, "wakes": wakes}

if __name__ == "__main__":
    OUT.mkdir(parents=True, exist_ok=True)
    for name, fn in CHARTS.items():
        for mode, t in THEMES.items():
            (OUT / f"{name}-{mode}.svg").write_text(fn(t), encoding="utf-8")
    print(f"wrote {len(CHARTS) * 2} files to {OUT}")
