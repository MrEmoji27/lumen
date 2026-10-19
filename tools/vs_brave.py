"""Lumen against Brave, same machine, same pages, measured the same way.

    python tools/vs_brave.py [--only idle,wiki,youtube,tabs] [--lumen PATH] [--out vs-brave]

Each browser starts fresh: a throwaway profile under %TEMP%, muted, off-screen, with the same
switches that keep an off-screen window from being treated as hidden (Lumen's headless mode
sets them; Brave gets them by hand). Both keep their own blocking on: Brave's Shields, Lumen's
blocker. The number is the private working set (Task Manager's Memory column) summed over the
browser's whole process tree, averaged over a few samples. Only processes this script started
are ever stopped, and only after checking their path.

Scenarios:
  idle     each browser's own start page, nothing else (40 s)
  wiki     one long Wikipedia article (30 s after load)
  youtube  one YouTube video playing (40 s after load)
  tabs     ten common sites opened one after another and left alone: measured 90 s after the
           last one, and again at 6.5 minutes (Lumen freezes hidden tabs at 5 minutes)

Writes docs/<out>.md and docs/<out>.json.
"""
import argparse, json, os, shutil, statistics, subprocess, sys, tempfile, time
from pathlib import Path

import psutil

ROOT = Path(__file__).resolve().parent.parent
BRAVE = r"C:\Program Files\BraveSoftware\Brave-Browser\Application\brave.exe"
WIKI = "https://en.wikipedia.org/wiki/Rust_(programming_language)"
YOUTUBE = "https://www.youtube.com/watch?v=k5IzbrzH6x8"
TABS = [
    "https://en.wikipedia.org/wiki/Rust_(programming_language)",
    "https://github.com/rust-lang/rust",
    "https://www.reddit.com/r/programming/",
    "https://www.youtube.com/",
    "https://www.bbc.com/news",
    "https://www.amazon.com/",
    "https://stackoverflow.com/questions",
    "https://www.google.com/search?q=rust+programming",
    "https://www.theverge.com/",
    "https://developer.mozilla.org/en-US/docs/Web/JavaScript",
]
# What Lumen's headless mode sets so an off-screen window still paints; Brave gets the same.
SAME = ["--disable-features=CalculateNativeWinOcclusion", "--disable-backgrounding-occluded-windows"]


def tree(p):
    try:
        return [p] + p.children(recursive=True)
    except psutil.Error:
        return []


def ws_mb(procs):
    t = 0
    for q in procs:
        try:
            t += q.memory_full_info().uss
        except psutil.Error:
            pass
    return t / 1048576


def sample(root, n=5, gap=2.0):
    vals, count = [], 0
    for _ in range(n):
        tr = tree(root)
        count = max(count, len(tr))
        vals.append(ws_mb(tr))
        time.sleep(gap)
    return round(statistics.median(vals)), count


def stop(root, exe):
    """The tree of a process this script started, and only if its path is the one it ran."""
    try:
        if Path(root.exe()).resolve() != Path(exe).resolve():
            return
        for c in root.children(recursive=True):
            try:
                c.kill()
            except psutil.Error:
                pass
        root.kill()
    except psutil.Error:
        pass


class Lumen:
    def __init__(self, exe, extra=None):
        self.tmp = Path(tempfile.mkdtemp(prefix="lumen-vs-"))
        self.exe = str(self.tmp / "lumen.exe")
        shutil.copy2(exe, self.exe)
        self.data = self.tmp / "data"
        self.data.mkdir()
        own = Path(os.environ["APPDATA"]) / "Lumen" / "filters"
        if own.is_dir():
            # A warm start, as a person has: the lists already on disk.
            shutil.copytree(own, self.data / "filters")
        (self.data / "settings.json").write_text(json.dumps({"first_run_done": True, **(extra or {})}), encoding="utf-8")
        self.env = dict(os.environ, LUMEN_DATA=str(self.data), LUMEN_HEADLESS="1")
        self.root = None

    def start(self, url=None):
        args = [self.exe, "--headless"] + ([url] if url else [])
        self.root = psutil.Process(subprocess.Popen(args, env=self.env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).pid)

    def open(self, url):
        # A second lumen.exe hands the address to the running one, as a click on a link would.
        subprocess.run([self.exe, "--headless", url], env=self.env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=30)

    def close(self):
        if self.root:
            stop(self.root, self.exe)
        time.sleep(2)
        shutil.rmtree(self.tmp, ignore_errors=True)


class Brave:
    def __init__(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="brave-vs-"))
        self.base = [BRAVE, f"--user-data-dir={self.tmp}", "--no-first-run", "--no-default-browser-check", "--mute-audio",
                     "--window-position=-32000,-32000", "--window-size=1360,860"] + SAME
        self.root = None

    def start(self, url=None):
        p = subprocess.Popen(self.base + ([url] if url else []), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.root = psutil.Process(p.pid)

    def open(self, url):
        subprocess.Popen(self.base + [url], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).wait(timeout=30)

    def close(self):
        if self.root:
            stop(self.root, BRAVE)
        time.sleep(3)
        shutil.rmtree(self.tmp, ignore_errors=True)


def scenario(name, make):
    out = {}
    b = make()
    try:
        if name == "idle":
            b.start()
            time.sleep(40)
            out["mb"], out["procs"] = sample(b.root)
        elif name == "wiki":
            b.start(WIKI)
            time.sleep(30)
            out["mb"], out["procs"] = sample(b.root)
        elif name == "youtube":
            b.start(YOUTUBE)
            time.sleep(40)
            out["mb"], out["procs"] = sample(b.root)
        elif name == "tabs":
            b.start(TABS[0])
            time.sleep(8)
            for u in TABS[1:]:
                b.open(u)
                time.sleep(8)
            time.sleep(90)
            out["mb"], out["procs"] = sample(b.root)
            time.sleep(max(0, 390 - 90 - 10))
            out["mb_later"], out["procs_later"] = sample(b.root)
    finally:
        b.close()
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--only", default="idle,wiki,youtube,tabs")
    ap.add_argument("--lumen", default=str(ROOT / "target" / "release" / "lumen.exe"))
    ap.add_argument("--out", default="vs-brave")
    ap.add_argument("--lumen-settings", default="", help='JSON laid over the settings of the Lumen under test, e.g. {"trim_watched": false}')
    a = ap.parse_args()
    which = [s for s in a.only.split(",") if s]
    rows = []
    for s in which:
        extra = json.loads(a.lumen_settings) if a.lumen_settings else None
        for name, make in (("Lumen", lambda: Lumen(a.lumen, extra)), ("Brave", Brave)):
            r = scenario(s, make)
            r.update(browser=name, scenario=s)
            print(json.dumps(r), flush=True)
            rows.append(r)
    (ROOT / "docs" / f"{a.out}.json").write_text(json.dumps(rows, indent=1), encoding="utf-8")
    lines = ["| Scenario | Lumen | Brave | Lumen saves |", "|---|---|---|---|"]
    for s in which:
        for key, label in (("mb", s), ("mb_later", f"{s}, 6.5 min")):
            l = next((r for r in rows if r["scenario"] == s and r["browser"] == "Lumen" and key in r), None)
            b = next((r for r in rows if r["scenario"] == s and r["browser"] == "Brave" and key in r), None)
            if l and b:
                pk = "procs" if key == "mb" else "procs_later"
                save = (1 - l[key] / b[key]) * 100 if b[key] else 0
                lines.append(f"| {label} | {l[key]} MB ({l[pk]} processes) | {b[key]} MB ({b[pk]} processes) | {save:.0f}% |")
    (ROOT / "docs" / f"{a.out}.md").write_text(
        "# Lumen against Brave\n\nMade by `tools/vs_brave.py`: same machine, fresh profiles, muted, off-screen, each browser's own "
        "blocking on. Private working set of the whole process tree, median of five samples.\n\n" + "\n".join(lines) + "\n",
        encoding="utf-8")
    print(f"wrote docs/{a.out}.md")


if __name__ == "__main__":
    main()
