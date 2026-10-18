"""Per-site check: does each site work in Lumen, does it lag, how much memory does it take.

Run: python tools/site_check.py [--only youtube,google] [--exe path\\to\\lumen.exe]

For every site, twice (ad blocking on, then off), a fresh headless Lumen opens the site in a
person's tab (`lumen.exe --headless <url>`), a copy of the release exe in %TEMP% so the
person's own Lumen is never touched, its own LUMEN_DATA, one tab, muted. Through WebView2's
DevTools port it reads:

- load: the page's own load event, and its largest paint
- lag: every long task the page ran (count, total, worst), buffered from the start
- work: a probe that says the site did its job (a video plays, results show, the feed renders)
- errors: console errors, and requests Lumen answered 403 (blocked), by host
- memory: Lumen plus every WebView2 process under it, private bytes, 15 s after load
- idle CPU: the same processes over 10 s once the page has settled

Writes docs/site-check.md (a table) and docs/site-check.json (everything).
"""

import argparse, json, os, shutil, subprocess, sys, tempfile, time, urllib.request
from collections import Counter
from pathlib import Path

import psutil
import websocket

ROOT = Path(__file__).resolve().parent.parent
PORT = 9399

# name, url, probe: JS returning {ok: bool, note: str}
SITES = [
    ("youtube", "https://www.youtube.com/watch?v=k5IzbrzH6x8",
     "(()=>{const v=document.querySelector('video'); const e=document.querySelector('.ytp-error'); return {ok: !!v && v.currentTime>2 && !e, note: e? e.innerText.slice(0,60) : (v? 'video at '+v.currentTime.toFixed(1)+'s' : 'no video')}})()"),
    ("youtube-search", "https://www.youtube.com/results?search_query=rust+programming",
     "(()=>{const n=document.querySelectorAll('ytd-video-renderer').length; return {ok: n>3, note: n+' results'}})()"),
    ("ytmusic", "https://music.youtube.com/",
     "(()=>{const a=!!document.querySelector('ytmusic-app'); const n=document.querySelectorAll('ytmusic-two-row-item-renderer, ytmusic-responsive-list-item-renderer').length; return {ok: a && n>0, note: n+' items'}})()"),
    ("google", "https://www.google.com/search?q=lumen+browser",
     "(()=>{const n=document.querySelectorAll('#search a h3').length; return {ok: n>3, note: n+' results'}})()"),
    ("gmail", "https://mail.google.com/",
     "(()=>{const s=!!document.querySelector('input[type=email], #identifierId'); return {ok: s || document.body.innerText.length>200, note: location.host}})()"),
    ("instagram", "https://www.instagram.com/",
     "(()=>{const s=!!document.querySelector('input[name=username], input[name=email]'); return {ok: s || document.querySelectorAll('article').length>0, note: s? 'login form' : 'feed'}})()"),
    ("discord", "https://discord.com/login",
     "(()=>{const s=!!document.querySelector('input[name=email]'); return {ok: s, note: s? 'login form' : document.title}})()"),
    ("reddit", "https://www.reddit.com/r/rust/",
     "(()=>{const n=new Set([...document.querySelectorAll('a[href*=\"/comments/\"]')].map(a=>a.getAttribute('href').split('/').slice(0,6).join('/'))).size; const ad=document.querySelectorAll('shreddit-ad-post, [data-promoted=true], a[href*=\"alb.reddit.com\"]').length; return {ok: n>2, note: n+' posts, '+ad+' ad bits'}})()"),
    ("x", "https://x.com/",
     "(()=>{const t=document.body? document.body.innerText.length : 0; return {ok: t>100, note: t+' chars'}})()"),
    ("github", "https://github.com/rust-lang/rust",
     "(()=>{const r=!!document.querySelector('article.markdown-body, #readme'); return {ok: r, note: r? 'readme' : document.title}})()"),
    ("wikipedia", "https://en.wikipedia.org/wiki/Rust_(programming_language)",
     "(()=>{const n=document.querySelectorAll('#mw-content-text p').length; return {ok: n>10, note: n+' paragraphs'}})()"),
    ("twitch", "https://www.twitch.tv/directory",
     "(()=>{const n=document.querySelectorAll('[data-a-target=card-0], article, [data-test-selector*=card]').length; return {ok: n>0 || document.body.innerText.length>500, note: n+' cards'}})()"),
    ("spotify", "https://open.spotify.com/",
     "(()=>{const t=document.body? document.body.innerText.length : 0; return {ok: t>200, note: t+' chars'}})()"),
]

PAGE_METRICS = """(async()=>{
 const nav=performance.getEntriesByType('navigation')[0]||{};
 const lt=await new Promise(res=>{try{const o=new PerformanceObserver(l=>{const e=l.getEntries().map(x=>x.duration);res(e)});o.observe({type:'longtask',buffered:true});setTimeout(()=>res([]),800)}catch(e){res([])}});
 const lcp=await new Promise(res=>{try{const o=new PerformanceObserver(l=>{const e=l.getEntries();res(e.length?e[e.length-1].startTime:0)});o.observe({type:'largest-contentful-paint',buffered:true});setTimeout(()=>res(0),800)}catch(e){res(0)}});
 lt.sort((a,b)=>b-a);
 return {load: Math.round(nav.loadEventEnd||0), lcp: Math.round(lcp), longtasks: lt.length, blocked_ms: Math.round(lt.reduce((a,b)=>a+b,0)), worst: Math.round(lt[0]||0), heap_mb: performance.memory? Math.round(performance.memory.usedJSHeapSize/1048576) : null};
})()"""


class Page:
    def __init__(self, port):
        t = None
        for _ in range(100):
            try:
                ts = json.load(urllib.request.urlopen(f"http://127.0.0.1:{port}/json", timeout=2))
                t = next((x for x in ts if x.get("type") == "page" and x.get("url", "").startswith("http")), None)
                if t:
                    break
            except Exception:
                pass
            time.sleep(0.2)
        if not t:
            raise RuntimeError("no page target")
        self.ws = websocket.create_connection(t["webSocketDebuggerUrl"], timeout=30, suppress_origin=True)
        self.n = 0
        self.logs = []

    def _pump(self, m):
        if m.get("method") == "Log.entryAdded":
            self.logs.append(m["params"]["entry"])
        elif m.get("method") == "Runtime.exceptionThrown":
            self.logs.append({"level": "error", "source": "exception", "text": m["params"]["exceptionDetails"].get("text", "")})

    def send(self, method, params=None):
        self.n += 1
        me = self.n
        self.ws.send(json.dumps({"id": me, "method": method, "params": params or {}}))
        while True:
            m = json.loads(self.ws.recv())
            if m.get("id") == me:
                return m.get("result", m)
            self._pump(m)

    def drain(self, seconds):
        end = time.time() + seconds
        self.ws.settimeout(0.3)
        while time.time() < end:
            try:
                self._pump(json.loads(self.ws.recv()))
            except Exception:
                pass
        self.ws.settimeout(30)

    def eval(self, expr):
        r = self.send("Runtime.evaluate", {"expression": expr, "returnByValue": True, "awaitPromise": True})
        return r.get("result", {}).get("value")


def tree(pid):
    try:
        p = psutil.Process(pid)
        return [p] + p.children(recursive=True)
    except psutil.Error:
        return []


def mem_mb(procs):
    total = 0
    for p in procs:
        try:
            total += p.memory_info().private
        except psutil.Error:
            pass
    return round(total / 1048576)


def cpu_s(procs):
    return sum(cpu_by_kind(procs).values())


def kind(p):
    """lumen, browser, renderer, gpu, utility: what a process of the tree is."""
    try:
        if p.name().lower().startswith("lumen"):
            return "lumen"
        t = next((a.split("=", 1)[1] for a in p.cmdline() if a.startswith("--type=")), "browser")
        return {"gpu-process": "gpu"}.get(t, t)
    except psutil.Error:
        return "gone"


def cpu_by_kind(procs):
    out = Counter()
    for p in procs:
        try:
            c = p.cpu_times()
            out[kind(p)] += c.user + c.system
        except psutil.Error:
            pass
    return out


def run(exe, name, url, probe, adblock):
    data = Path(tempfile.mkdtemp(prefix=f"lumen-site-{name}-"))
    (data / "settings.json").write_text(json.dumps({"adblock": adblock, "cosmetic": adblock}), encoding="utf-8")
    # A warm start, as the person has: the lists and the built matcher already on disk.
    # A copy of the person's, read only; nothing is written back.
    own = Path(os.environ["APPDATA"]) / "Lumen" / "filters"
    if own.is_dir():
        shutil.copytree(own, data / "filters")
    env = dict(os.environ, LUMEN_DATA=str(data), LUMEN_HEADLESS="1", WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=f"--remote-debugging-port={PORT}")
    proc = subprocess.Popen([exe, "--headless", url], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    out = {"site": name, "adblock": adblock}
    try:
        page = Page(PORT)
        page.send("Log.enable")
        page.send("Runtime.enable")
        page.drain(14)
        out.update(page.eval(PAGE_METRICS) or {})
        out["probe"] = page.eval(probe) or {}
        procs = tree(proc.pid)
        page.drain(1)
        out["mem_mb"] = mem_mb(procs)
        c0 = cpu_by_kind(procs)
        page.drain(10)
        c1 = cpu_by_kind(tree(proc.pid))
        pct = lambda s: round(s / 10 * 100 / psutil.cpu_count(), 2)
        out["idle_cpu_pct"] = pct(sum(c1.values()) - sum(c0.values()))
        out["idle_cpu_by"] = {k: pct(c1[k] - c0.get(k, 0)) for k in c1 if c1[k] - c0.get(k, 0) > 0.01}
        errs = [e for e in page.logs if e.get("level") == "error"]
        blocked = [e for e in errs if "403 (Blocked)" in e.get("text", "")]
        out["errors"] = len(errs) - len(blocked)
        out["blocked"] = len(blocked)
        out["blocked_hosts"] = Counter(urllib.parse.urlparse(e.get("url", "")).netloc for e in blocked).most_common(6)
        out["blocked_urls"] = sorted({e.get("url", "")[:160] for e in blocked})[:30]
        out["error_samples"] = [e.get("text", "")[:140] for e in errs if "403 (Blocked)" not in e.get("text", "")][:5]
    except Exception as e:
        out["fail"] = str(e)[:200]
    finally:
        # Only the Lumen this run started, then the folder.
        try:
            p = psutil.Process(proc.pid)
            if Path(p.exe()).resolve() == Path(exe).resolve():
                for c in p.children(recursive=True):
                    c.kill()
                p.kill()
        except psutil.Error:
            pass
        time.sleep(2)
        shutil.rmtree(data, ignore_errors=True)
    return out


def main():
    import urllib.parse  # noqa: F401  (used above through urllib)
    ap = argparse.ArgumentParser()
    ap.add_argument("--only", default="")
    ap.add_argument("--exe", default=str(ROOT / "target" / "release" / "lumen.exe"))
    ap.add_argument("--runs", type=int, default=1, help="runs per site and blocker setting")
    ap.add_argument("--out", default="site-check")
    a = ap.parse_args()
    # A copy, so the person's running Lumen is never the one stopped.
    copy_dir = Path(tempfile.mkdtemp(prefix="lumen-site-exe-"))
    exe = str(copy_dir / "lumen.exe")
    shutil.copy2(a.exe, exe)
    only = set(filter(None, a.only.split(",")))
    results = []
    for name, url, probe in SITES:
        if only and name not in only:
            continue
        for adblock in (True, False):
            for k in range(a.runs):
                r = run(exe, name, url, probe, adblock)
                r["run"] = k + 1
                results.append(r)
                print(json.dumps(r), flush=True)
    shutil.rmtree(copy_dir, ignore_errors=True)
    (ROOT / "docs" / f"{a.out}.json").write_text(json.dumps(results, indent=1), encoding="utf-8")
    rows = ["| Site | Blocker | Works | Load | Lag (long tasks) | Worst | Memory | Idle CPU | Errors | Blocked |", "|---|---|---|---|---|---|---|---|---|---|"]
    for r in results:
        p = r.get("probe", {})
        works = ("yes" if p.get("ok") else "**no**") + (f" ({p.get('note')})" if p.get("note") else "")
        if "fail" in r:
            works = f"**run failed**: {r['fail'][:60]}"
        rows.append(f"| {r['site']} | {'on' if r['adblock'] else 'off'} | {works} | {r.get('load', '')} ms | {r.get('longtasks', '')} = {r.get('blocked_ms', '')} ms | {r.get('worst', '')} ms | {r.get('mem_mb', '')} MB | {r.get('idle_cpu_pct', '')}% | {r.get('errors', '')} | {r.get('blocked', '')} |")
    (ROOT / "docs" / f"{a.out}.md").write_text("# Site check\n\nMade by `tools/site_check.py`. One site per fresh headless Lumen, release build, signed out.\n\n" + "\n".join(rows) + "\n", encoding="utf-8")
    print(f"wrote docs/{a.out}.md")


if __name__ == "__main__":
    import urllib.parse
    main()
