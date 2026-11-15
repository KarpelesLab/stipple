#!/usr/bin/env python3
"""Make benchmark runs the new baselines (perf/baseline/<runner>-<renderer>.json).

    python3 tools/bench/update_baseline.py --run 123456789     # a Perf workflow run's artifacts (gh)
    python3 tools/bench/update_baseline.py --from artifacts/   # JSON files already downloaded
    python3 tools/bench/update_baseline.py --from target/bench --provisional --note "M5 Max, local"

The Perf workflow uploads each job's report as an artifact `perf-<runner>-<renderer>`
holding `<runner>-<renderer>.json`; every such file found (in the run's
artifacts, or under --from) replaces the baseline of the same name. Review
the diff and commit it. Running the workflow by hand with `update_baseline`
ticked does the same and commits the result.

--provisional marks the files as such: tools/bench/compare.py then reports
regressions against them without failing (for numbers from another machine
than the CI runner's).
"""

import argparse
import json
import re
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
BASELINE = ROOT / "perf" / "baseline"
NAME = re.compile(r"^(linux|macos|windows|web)-[a-z0-9]+\.json$")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    src = ap.add_mutually_exclusive_group(required=True)
    src.add_argument("--run", help="a Perf workflow run id (downloads its perf-* artifacts with gh)")
    src.add_argument("--from", dest="source", type=Path, help="a directory holding the JSON reports")
    ap.add_argument("--provisional", action="store_true", help="report against them, never fail")
    ap.add_argument("--note", default="", help="where the numbers come from")
    a = ap.parse_args()

    with tempfile.TemporaryDirectory() as tmp:
        source = a.source
        if a.run:
            source = Path(tmp)
            subprocess.run(
                ["gh", "run", "download", a.run, "--pattern", "perf-*", "--dir", str(source)],
                check=True,
                cwd=ROOT,
            )
        found = sorted(p for p in source.rglob("*.json") if NAME.match(p.name))
        if not found:
            sys.exit(f"no <runner>-<renderer>.json reports under {source}")
        BASELINE.mkdir(parents=True, exist_ok=True)
        for p in found:
            report = json.loads(p.read_text())
            if report.get("gpu_unavailable"):
                print(f"skip {p.name}: it measured the software renderer ({report['gpu_unavailable']})")
                continue
            report.pop("provisional", None)
            if a.provisional:
                report["provisional"] = True
            report["baseline_from"] = a.note or (f"Perf workflow run {a.run}" if a.run else str(p))
            (BASELINE / p.name).write_text(json.dumps(report, indent=2) + "\n")
            print(f"perf/baseline/{p.name}")


if __name__ == "__main__":
    main()
