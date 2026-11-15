#!/usr/bin/env python3
"""Run Stipple's benchmarks in headless Chrome.

    cargo build --release --target wasm32-unknown-unknown --features gpu --example bench_web
    python3 tools/bench/run_web.py --out target/bench                      # software, webgl2, webgpu
    python3 tools/bench/run_web.py --renderer software --swiftshader --out target/bench

It serves tools/bench/index.html next to web/stipple.js, web/gpu.js, the
module and a font, opens the page once per renderer, and writes each report
to OUT/web-<renderer>.json (the format tools/bench/compare.py reads). The
scenarios that run inside the module (the native benchmark's, on the
software renderer) run on the `software` page only; every page times events
to presented frames on its renderer.

--swiftshader draws WebGL2 and WebGPU on SwiftShader, Chrome's CPU
implementation (what a CI runner without a GPU has); the report records the
device either way (`gpu_device`).
"""

import argparse
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
import urllib.parse
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent

FONTS = [
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/System/Library/Fonts/Supplemental/Arial.ttf",
    "/Library/Fonts/Arial.ttf",
    "C:\\Windows\\Fonts\\arial.ttf",
]


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


def run_page(chrome, files, query, timeout, swiftshader):
    """Serve `files` ({url path: file}), open index.html?query, and return the
    report the page POSTs (or None)."""
    report = {}
    done = threading.Event()

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def do_GET(self):
            path = urllib.parse.urlparse(self.path).path.lstrip("/") or "index.html"
            src = files.get(path)
            if src is None:
                self.send_error(404)
                return
            data = Path(src).read_bytes()
            ctype = {
                ".html": "text/html",
                ".js": "text/javascript",
                ".wasm": "application/wasm",
                ".ttf": "font/ttf",
            }.get(Path(path).suffix, "application/octet-stream")
            self.send_response(200)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(data)))
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            self.wfile.write(data)

        def do_POST(self):
            body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
            if self.path == "/__report":
                report["data"] = json.loads(body)
                done.set()
            self.send_response(200)
            self.send_header("Content-Length", "0")
            self.end_headers()

    server = socketserver.ThreadingTCPServer(("127.0.0.1", 0), Handler)
    server.daemon_threads = True
    port = server.server_address[1]
    threading.Thread(target=server.serve_forever, daemon=True).start()
    profile = tempfile.mkdtemp(prefix="stipple-bench-")
    cmd = [
        chrome,
        "--headless=new",
        "--no-sandbox",
        "--no-first-run",
        "--mute-audio",
        "--hide-scrollbars",
        "--force-device-scale-factor=1",
        "--window-size=1100,900",
        "--enable-unsafe-webgpu",
        # Keep timers and frames running at full rate in the background.
        "--disable-background-timer-throttling",
        "--disable-renderer-backgrounding",
        f"--user-data-dir={profile}",
        "--remote-debugging-port=0",
    ]
    if swiftshader:
        cmd += [
            "--enable-features=Vulkan",
            "--use-angle=swiftshader",
            "--use-vulkan=swiftshader",
            "--enable-unsafe-swiftshader",
        ]
    cmd.append(f"http://127.0.0.1:{port}/index.html?{query}")
    proc = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    deadline = time.time() + timeout
    while not done.is_set() and time.time() < deadline and proc.poll() is None:
        time.sleep(0.2)
    proc.kill()
    proc.wait()
    server.shutdown()
    shutil.rmtree(profile, ignore_errors=True)
    return report.get("data")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--renderer", default="software,webgl2,webgpu", help="comma-separated (default: %(default)s)")
    ap.add_argument("--out", type=Path, default=ROOT / "target" / "bench", help="where the reports go")
    ap.add_argument("--wasm", type=Path, default=ROOT / "target/wasm32-unknown-unknown/release/examples/bench_web.wasm")
    ap.add_argument("--font", type=Path, help="a .ttf for the app (default: DejaVu Sans or Arial, found)")
    ap.add_argument("--chrome", help="the Chrome binary (default: found on PATH, or $CHROME)")
    ap.add_argument("--swiftshader", action="store_true", help="WebGL2 and WebGPU on SwiftShader (as on CI)")
    ap.add_argument("--only", default="", help="scenarios to run in the module (default: all)")
    ap.add_argument("--percent", type=int, default=100, help="iterations, %% of the native counts")
    ap.add_argument("--events", type=int, default=40, help="events per interaction")
    ap.add_argument("--json", type=Path, help="with one renderer: write its report here instead")
    ap.add_argument("--runner", default="", help="a label stored in the reports")
    ap.add_argument("--timeout", type=float, default=600)
    a = ap.parse_args()

    chrome = find_chrome(a.chrome)
    if not chrome:
        sys.exit("no Chrome found (--chrome, or $CHROME)")
    if not a.wasm.exists():
        sys.exit(f"no {a.wasm}: cargo build --release --target wasm32-unknown-unknown --features gpu --example bench_web")
    font = a.font or next((Path(f) for f in FONTS if Path(f).exists()), None)
    if not font:
        sys.exit("no font found (--font)")
    files = {
        "index.html": HERE / "index.html",
        "stipple.js": ROOT / "web" / "stipple.js",
        "gpu.js": ROOT / "web" / "gpu.js",
        "bench_web.wasm": a.wasm,
        "font.ttf": font,
    }
    a.out.mkdir(parents=True, exist_ok=True)
    failed = False
    for renderer in [r for r in a.renderer.split(",") if r]:
        q = {"renderer": renderer, "events": a.events, "percent": a.percent}
        if renderer != "software":
            q["scenarios"] = 0
        elif a.only:
            q["only"] = a.only
        print(f"web bench: {renderer}…", flush=True)
        report = run_page(chrome, files, urllib.parse.urlencode(q), a.timeout, a.swiftshader)
        if report is None:
            print(f"FAIL {renderer}: no report (the page didn't finish in time)")
            failed = True
            continue
        report["runner"] = a.runner
        report["commit"] = os.environ.get("GITHUB_SHA", "")
        report["swiftshader"] = a.swiftshader
        path = a.json or a.out / f"web-{renderer}.json"
        path.write_text(json.dumps(report, indent=2) + "\n")
        for s in report["scenarios"]:
            work = f", work {s['work_ms']:.3f}" if s.get("work_ms") is not None else ""
            print(f"  {s['name']:<18} {s['renderer']:<9} median {s['median_ms']:.3f} ms, p95 {s['p95_ms']:.3f}{work}")
        if report.get("gpu_unavailable"):
            print(f"  note: {report['gpu_unavailable']} (device: {report.get('gpu_device')})")
        for e in report.get("errors", []):
            print(f"  error: {e[:400]}")
            failed = True
        print(f"  wrote {path}")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
