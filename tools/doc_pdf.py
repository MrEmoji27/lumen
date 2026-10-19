"""docs/lumen.md -> docs/lumen.pdf: styled HTML, printed by headless Edge.

Run: python tools/doc_pdf.py

The Markdown stays the source. This adds a cover with the headline numbers, prints the light
variant of every chart, and lays the rest out for A4. Needs Microsoft Edge (it is on every
Windows 11 machine) and the `markdown` package.
"""

import os
import re
import subprocess
import tempfile
from pathlib import Path

import markdown

ROOT = Path(__file__).resolve().parent.parent
DOCS = ROOT / "docs"
EDGE = Path(r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe")

# The cover's figures: each one is a measurement the doc itself states.
# The cover says what Lumen is, not how it compares: the comparison waits inside, with its
# method and its losses. The one number here is the whole process tree, in use.
FIGURES = [
    ("37 MB", "the whole browser, nothing open"),
    ("51,000", "lines of Rust"),
    ("3", "looks, every pixel drawn natively"),
    ("48", "tools for AI agents"),
]

CSS = """
@page { size: A4; margin: 18mm 17mm 20mm 17mm;
  @bottom-left { content: "Lumen VERSION"; font: 8pt 'Segoe UI', system-ui, sans-serif; color: #898781; }
  @bottom-right { content: counter(page) " / " counter(pages); font: 8pt 'Segoe UI', system-ui, sans-serif; color: #898781; } }
@page :first { @bottom-left { content: none; } @bottom-right { content: none; } }
:root { --ink: #1b1b1a; --ink2: #52514e; --muted: #898781; --line: #e1e0d9; --plane: #f6f5f1; --accent: #2a78d6; }
* { box-sizing: border-box; }
html { -webkit-print-color-adjust: exact; print-color-adjust: exact; }
body { margin: 0; font: 9.6pt/1.55 'Segoe UI', system-ui, sans-serif; color: var(--ink); }
.cover { height: 257mm; display: flex; flex-direction: column; justify-content: space-between;
  break-after: page; padding: 8mm 2mm 0; }
.cover .mark { font-size: 9pt; letter-spacing: .18em; text-transform: uppercase; color: var(--accent); font-weight: 600; }
.cover h1 { font-size: 46pt; line-height: 1; margin: 10mm 0 5mm; letter-spacing: -.02em; font-weight: 700; }
.cover .lede { font-size: 15pt; line-height: 1.35; color: var(--ink2); max-width: 140mm; margin: 0; font-weight: 400; }
.cover .meta { margin-top: 8mm; font-size: 9.5pt; color: var(--muted); }
.figs { display: grid; grid-template-columns: repeat(2, 1fr); gap: 5mm; margin-bottom: 4mm; }
.fig { background: var(--plane); border-radius: 10px; padding: 6mm 6mm 5mm; }
.fig b { display: block; font-size: 27pt; font-weight: 600; letter-spacing: -.01em; line-height: 1.1; }
.fig span { color: var(--ink2); font-size: 10pt; }
.note { font-size: 8.5pt; color: var(--muted); margin-top: 3mm; }
h1.doc { display: none; }
h2 { font-size: 17pt; margin: 9mm 0 3mm; padding-top: 2mm; border-top: 2px solid var(--ink);
  letter-spacing: -.01em; break-after: avoid; }
h3 { font-size: 11.5pt; margin: 7mm 0 2mm; break-after: avoid; }
p { margin: 0 0 2.6mm; orphans: 3; widows: 3; }
ul, ol { margin: 0 0 3mm; padding-left: 5.5mm; }
li { margin: 0 0 1.2mm; }
strong { font-weight: 600; }
em { color: var(--accent); font-style: normal; font-weight: 600; }
a { color: var(--accent); text-decoration: none; }
code { font: 8.6pt 'Cascadia Mono', Consolas, monospace; background: var(--plane); padding: .2mm 1.2mm; border-radius: 3px; }
table { width: 100%; border-collapse: collapse; margin: 1mm 0 5mm; font-size: 8.8pt; break-inside: avoid; }
th { text-align: left; font-weight: 600; color: var(--ink2); font-size: 8pt; text-transform: uppercase;
  letter-spacing: .04em; padding: 1.6mm 2.4mm; border-bottom: 1.5px solid var(--ink); }
td { padding: 1.7mm 2.4mm; border-bottom: 1px solid var(--line); vertical-align: top; font-variant-numeric: tabular-nums; }
td:first-child { font-weight: 500; }
img.chart { display: block; width: 100%; height: auto; margin: 2mm 0 4mm; break-inside: avoid; }
img.shot { display: block; width: 100%; height: auto; margin: 2mm 0 4mm; border-radius: 2mm; border: 1px solid var(--line); break-inside: avoid; }
.keep { break-inside: avoid; }
/* A sentence that introduces a table stays on the table's page. */
p:has(+ table) { break-after: avoid; }
"""


def version():
    """The version, from Cargo.toml: the one number, never typed twice."""
    toml = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    base = re.search(r'^version\s*=\s*"([^"]+)"', toml, flags=re.M).group(1)
    rel = re.search(r'^release\s*=\s*(\d+)', toml, flags=re.M)
    return f"{base}.{rel.group(1)}" if rel else base


def cover():
    figs = "".join(f'<div class="fig"><b>{v}</b><span>{k}</span></div>' for v, k in FIGURES)
    return (
        f'<section class="cover"><div><div class="mark">Version {version()} · 19 October 2026</div>'
        '<h1>Lumen</h1>'
        '<p class="lede">A native Windows shell around WebView2, written in Rust, built to keep memory low.</p>'
        '<p class="meta">What it is so far, what it measures, and where it goes next.</p>'
        '<p class="meta">Made by zemo · portfolio.mremoji47.workers.dev</p></div>'
        f'<div><div class="figs">{figs}</div>'
        '<p class="note">Measured on one laptop, a fresh profile, the content blocker on, the '
        'whole process tree, private working set. More numbers, and how they were made: the Performance section.</p></div></section>'
    )


def build():
    md = (DOCS / "lumen.md").read_text(encoding="utf-8")
    # Each chart in print: its light SVG, by absolute file URL.
    img = (DOCS / "img").as_uri()
    md = re.sub(
        r'<picture>.*?<img alt="([^"]*)" src="img/([a-z0-9]+)-light\.svg">\s*</picture>',
        lambda m: f'<img class="chart" alt="{m.group(1)}" src="{img}/{m.group(2)}-light.svg">',
        md,
        flags=re.S,
    )
    # Screenshots: by absolute file URL too, since the page is printed from a temp folder.
    md = md.replace('src="img/', f'src="{img}/')
    # The cover carries the title, tagline and version; drop the doc's own opening lines.
    md = re.sub(r"\A# Lumen\n\n.*?\n\n", "", md, flags=re.S)
    body = markdown.markdown(md, extensions=["tables", "sane_lists"])
    # Keep each h3 with the chart under it.
    body = re.sub(r'(<h3>.*?</h3>\s*<img class="chart"[^>]*>)', r'<div class="keep">\1</div>', body, flags=re.S)
    html = f'<!doctype html><html lang="en"><head><meta charset="utf-8"><title>Lumen</title><style>{CSS.replace('VERSION', version())}</style></head><body>{cover()}{body}</body></html>'
    tmp = Path(tempfile.gettempdir()) / "lumen-print.html"
    tmp.write_text(html, encoding="utf-8")
    out = DOCS / "lumen.pdf"
    profile = Path(tempfile.gettempdir()) / "lumen-print-edge"
    subprocess.run(
        [str(EDGE), "--headless=new", "--disable-gpu", "--no-pdf-header-footer", f"--user-data-dir={profile}",
         f"--print-to-pdf={out}", tmp.as_uri()],
        check=True, timeout=120, creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
    )
    print(f"wrote {out} ({out.stat().st_size // 1024} KB)")


if __name__ == "__main__":
    build()
