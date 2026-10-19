"""Lumen's idle memory, measured the way tools/vs_brave.py measures it.

    python tools/mem.py --tag baseline --scenarios idle,wiki

Every scenario starts a fresh Lumen: a throwaway profile under %TEMP%, the exe
copied next to it, off-screen (LUMEN_HEADLESS), muted, and the same blocking
lists the person has. The number is the private working set (Task Manager's
Memory column) summed over the whole process tree, median of five samples.
Only the tree this script started is ever stopped, and only after checking the
path.

Scenarios:
  idle    nothing open: the start page alone
  wiki    one Wikipedia article, left alone for 15 s (the page trims after that)
  tabs10  ten common sites opened one after another, left alone for 60 s
"""
import argparse, json, os, shutil, statistics, subprocess, sys, tempfile, time
from pathlib import Path

import psutil

ROOT = Path(__file__).resolve().parent.parent
WIKI = "https://en.wikipedia.org/wiki/Rust_(programming_language)"
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


def tree(p):
    try:
        return [p] + p.children(recursive=True)
    except psutil.Error:
        return []


def idle_seconds():
    """How long since the last mouse click or key press, anywhere in Windows.
    Lumen trims the page on screen 15 s after that, so the measurement waits
    for it: otherwise the number says more about the person at the keyboard
    than about Lumen."""
    import ctypes

    class LastInputInfo(ctypes.Structure):
        _fields_ = [("cbSize", ctypes.c_uint), ("dwTime", ctypes.c_ulong)]

    info = LastInputInfo()
    info.cbSize = ctypes.sizeof(info)
    ctypes.windll.user32.GetLastInputInfo(ctypes.byref(info))
    return (ctypes.windll.kernel32.GetTickCount() - info.dwTime) / 1000.0


def wait_idle(seconds, cap=90.0):
    """Wait until the machine has been left alone for `seconds`. Gives up after
    `cap` and says so: the person is at the keyboard, and the page on screen
    will not be trimmed."""
    waited = 0.0
    while waited < cap:
        if idle_seconds() >= seconds:
            return True
        time.sleep(1.0)
        waited += 1.0
    return False


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
        self.tmp = Path(tempfile.mkdtemp(prefix="lumen-mem-"))
        self.exe = str(self.tmp / "lumen.exe")
        shutil.copy2(exe, self.exe)
        self.data = self.tmp / "data"
        self.data.mkdir()
        own = Path(os.environ["APPDATA"]) / "Lumen" / "filters"
        if own.is_dir():
            # A warm start, as a person has: the lists already on disk.
            shutil.copytree(own, self.data / "filters")
        (self.data / "settings.json").write_text(
            json.dumps({"first_run_done": True, **(extra or {})}), encoding="utf-8")
        self.env = dict(os.environ, LUMEN_DATA=str(self.data), LUMEN_HEADLESS="1", LUMEN_ARGS="--mute-audio")
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


def scenario(name, make, settle):
    b = make()
    try:
        if name == "idle":
            b.start()
            time.sleep(settle)
            mb, procs = sample(b.root)
        elif name == "wiki":
            b.start(WIKI)
            time.sleep(settle)
            # The page has loaded; now wait for the machine to go quiet, so the
            # page is trimmed the way it is for a person who stepped away.
            trimmed = wait_idle(18.0)
            mb, procs = sample(b.root)
            return {"mb": mb, "procs": procs, "trimmed": trimmed}
        elif name == "tabs10":
            b.start(TABS[0])
            time.sleep(8)
            for u in TABS[1:]:
                b.open(u)
                time.sleep(6)
            time.sleep(settle)
            mb, procs = sample(b.root)
        else:
            raise SystemExit(f"unknown scenario {name}")
    finally:
        b.close()
    return {"mb": mb, "procs": procs}


# How long each scenario is left alone before it is measured. The article is
# given the same time as the empty window: long enough that the page has
# finished loading and the trim (15 s after the last input) has had its turn.
SETTLE = {"idle": 40.0, "wiki": 40.0, "tabs10": 60.0}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--tag", default="run")
    ap.add_argument("--scenarios", default="idle,wiki")
    ap.add_argument("--lumen", default=str(ROOT / "target" / "release" / "lumen.exe"))
    ap.add_argument("--settle", type=float, default=0.0, help="seconds to leave it alone before measuring")
    ap.add_argument("--settings", default="", help="JSON laid over the settings of the Lumen under test")
    ap.add_argument("--env", default="", help="extra environment: NAME=VALUE,NAME=VALUE")
    a = ap.parse_args()
    extra = json.loads(a.settings) if a.settings else None
    for pair in filter(None, (x.strip() for x in a.env.split(","))):
        k, _, v = pair.partition("=")
        os.environ[k] = v
    rows = []
    for s in [x for x in a.scenarios.split(",") if x]:
        settle = a.settle or SETTLE.get(s, 15.0)
        r = scenario(s, lambda: Lumen(a.lumen, extra), settle)
        r.update(tag=a.tag, scenario=s)
        print(json.dumps(r), flush=True)
        rows.append(r)
    out = ROOT / "docs" / "mem.json"
    rows_all = json.loads(out.read_text(encoding="utf-8")) if out.exists() else []
    rows_all += rows
    out.write_text(json.dumps(rows_all, indent=1), encoding="utf-8")
    print(f"wrote {out}")


if __name__ == "__main__":
    main()
