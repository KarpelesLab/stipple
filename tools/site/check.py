#!/usr/bin/env python3
"""Check the generated site, and take the gallery's thumbnails.

    python3 tools/site/check.py target/site            # links, then every example in Chrome
    python3 tools/site/check.py target/site --links    # links only
    python3 tools/site/check.py target/site --only todo,calculator

Links: every relative href/src in the site's pages must name a file in the
site, and a #fragment an id in it.

Examples: serves the site, opens every web example (examples/NAME.html?check)
in headless Chrome, four at a time, in frames of the example's size; each
page posts back whether the app started, how many frames it presented, how
many colors its canvas shows (a blank canvas has one), the errors it logged,
and the canvas as a PNG, which becomes thumbs/NAME.png. Exits non-zero if
any link is broken or any example failed. Standard library only.
"""

import argparse
import base64
import http.server
import json
import os
import shutil
import socketserver
import subprocess
import sys
import tempfile
import threading
import time
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import unquote, urlsplit


class Refs(HTMLParser):
    def __init__(self):
        super().__init__()
        self.refs = []
        self.ids = set()

    def handle_starttag(self, tag, attrs):
        a = dict(attrs)
        if "id" in a:
            self.ids.add(a["id"])
        if tag == "a" and "name" in a:
            self.ids.add(a["name"])
        for key in ("href", "src"):
            if a.get(key) is not None:
                self.refs.append((tag, a[key]))


def check_links(site):
    pages = {}
    for p in site.rglob("*.html"):
        parser = Refs()
        parser.feed(p.read_text(errors="replace"))
        pages[p.resolve()] = parser
    broken = []
    for page, parser in pages.items():
        for tag, ref in parser.refs:
            u = urlsplit(ref)
            if u.scheme or u.netloc or ref.startswith(("data:", "javascript:", "mailto:")):
                continue
            target = page if not u.path else (page.parent / unquote(u.path)).resolve()
            if target.is_dir():
                target = target / "index.html"
            rel = page.relative_to(site.resolve())
            if not target.exists():
                broken.append(f"{rel}: {ref} (no such file)")
                continue
            if u.fragment and target.suffix == ".html" and target in pages:
                if u.fragment not in pages[target].ids:
                    broken.append(f"{rel}: {ref} (no #{u.fragment} there)")
    return broken


CHECK_PAGE = """<!doctype html><meta charset="utf-8"><title>check</title>
<style>body{margin:0;background:#888} iframe{border:0;display:block;margin:4px;background:#fff}</style>
<script type="module">
const list = %s;
const queue = [...list];
async function one(ex) {
  const f = document.createElement("iframe");
  f.style.width = ex.size[0] + "px";
  f.style.height = ex.size[1] + "px";
  f.src = `examples/${ex.name}.html?embed&check`;
  document.body.append(f);
  // The page reports to the server; the server tells us when it has.
  const t0 = performance.now();
  while (performance.now() - t0 < 30000) {
    await new Promise((r) => setTimeout(r, 300));
    const r = await fetch(`/__done?${ex.name}`);
    if ((await r.text()) === "1") break;
  }
  f.remove();
}
async function worker() { while (queue.length) await one(queue.shift()); }
await Promise.all([worker(), worker(), worker(), worker()]);
await fetch("/__finished");
</script>
"""


def find_chrome(explicit):
    if explicit:
        return explicit
    for c in (
        os.environ.get("CHROME"),
        shutil.which("google-chrome"),
        shutil.which("google-chrome-stable"),
        shutil.which("chromium"),
        shutil.which("chromium-browser"),
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    ):
        if c and Path(c).exists():
            return c
    return None


def check_examples(site, chrome, only, thumbs, timeout):
    meta = json.loads((site / "site.json").read_text())
    examples = [e for e in meta["web"] if not only or e["name"] in only]
    reports = {}
    finished = threading.Event()
    page = CHECK_PAGE % json.dumps(examples)

    class Handler(http.server.SimpleHTTPRequestHandler):
        def __init__(self, *a, **k):
            super().__init__(*a, directory=str(site), **k)

        def log_message(self, *a):
            pass

        def do_GET(self):
            if self.path == "/__check.html":
                return self.reply(page.encode(), "text/html")
            if self.path.startswith("/__done?"):
                name = self.path.split("?", 1)[1]
                return self.reply(b"1" if name in reports else b"0", "text/plain")
            if self.path == "/__finished":
                finished.set()
                return self.reply(b"ok", "text/plain")
            return super().do_GET()

        def do_POST(self):
            body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
            if self.path == "/__report":
                r = json.loads(body)
                reports[r["name"]] = r
            self.reply(b"ok", "text/plain")

        def reply(self, data, ctype):
            self.send_response(200)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

    Handler.extensions_map = {**Handler.extensions_map, ".wasm": "application/wasm", ".js": "text/javascript"}
    server = socketserver.ThreadingTCPServer(("127.0.0.1", 0), Handler)
    server.daemon_threads = True
    port = server.server_address[1]
    threading.Thread(target=server.serve_forever, daemon=True).start()

    profile = tempfile.mkdtemp(prefix="stipple-check-")
    cmd = [
        chrome,
        "--headless=new",
        "--no-sandbox",
        "--disable-gpu",
        "--hide-scrollbars",
        "--no-first-run",
        "--mute-audio",
        "--force-device-scale-factor=1",
        "--window-size=2000,1400",
        f"--user-data-dir={profile}",
        "--remote-debugging-port=0",
        f"http://127.0.0.1:{port}/__check.html",
    ]
    proc = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    deadline = time.time() + timeout
    while not finished.is_set() and time.time() < deadline and proc.poll() is None:
        time.sleep(0.5)
    proc.kill()
    proc.wait()
    server.shutdown()
    shutil.rmtree(profile, ignore_errors=True)

    failed = []
    if thumbs:
        (site / "thumbs").mkdir(exist_ok=True)
    for ex in examples:
        name = ex["name"]
        r = reports.get(name)
        if r is None:
            failed.append(name)
            print(f"FAIL {name}: no report (didn't load in time)")
            continue
        errors = [e for e in r["errors"] if e]
        ok = r["started"] and r["presents"] > 0 and r["colors"] > 2 and not any("panicked" in e for e in errors)
        line = f"{'ok  ' if ok else 'FAIL'} {name}: {r['presents']} frames, {r['colors']} colors"
        if errors:
            line += " | " + " | ".join(e[:300] for e in errors[:3])
        print(line)
        if not ok:
            failed.append(name)
        if thumbs and r.get("png", "").startswith("data:image/png;base64,") and ok:
            (site / "thumbs" / f"{name}.png").write_bytes(base64.b64decode(r["png"].split(",", 1)[1]))
    return failed


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("site", type=Path)
    ap.add_argument("--links", action="store_true", help="check links only")
    ap.add_argument("--chrome", help="the Chrome binary (default: found on PATH)")
    ap.add_argument("--only", help="comma-separated examples to run")
    ap.add_argument("--no-thumbs", action="store_true", help="don't write thumbs/")
    ap.add_argument("--timeout", type=int, default=600)
    a = ap.parse_args()
    ok = True
    if not a.links:
        chrome = find_chrome(a.chrome)
        if not chrome:
            print("no Chrome found (--chrome, or $CHROME)")
            sys.exit(2)
        only = set(a.only.split(",")) if a.only else None
        failed = check_examples(a.site, chrome, only, not a.no_thumbs, a.timeout)
        print(f"examples: {len(failed)} failed" + (f" ({', '.join(failed)})" if failed else ""))
        ok = not failed
    # After the examples, which write the thumbnails the gallery shows. A
    # missing thumbnail isn't broken: the card shows a placeholder.
    broken = [b for b in check_links(a.site) if "thumbs/" not in b]
    for b in broken:
        print("broken link:", b)
    print(f"links: {len(broken)} broken")
    ok = ok and not broken
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
