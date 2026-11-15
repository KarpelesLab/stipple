#!/usr/bin/env python3
"""Compare a benchmark run with its baseline, and check the latency budgets.

    python3 tools/bench/compare.py target/bench/linux-software.json \\
        --baseline perf/baseline/linux-software.json \\
        --rerun "target/release/examples/bench --renderer software --only {only} --json {json}" \\
        --summary "$GITHUB_STEP_SUMMARY" --title "Linux · software"

Reads the JSON `examples/bench` (or `tools/bench/run_web.py`) writes. For
each scenario in the baseline it compares the medians, after dividing out
how fast the machine was in each run: both runs time the same CPU loop
(`calibration_ms`), and the baseline's median is scaled by the ratio of the
two (clamped to 1/3..3) before comparing, for the scenarios that do CPU work
(`"normalize": false` in perf/budgets.json leaves one alone: the browser's
latencies, which wait for the display's next frame).

A scenario regresses when its median is more than its threshold times the
(scaled) baseline's, *and* more than `min_delta_ms` slower in absolute terms
(sub-millisecond scenarios jitter by more than their own size on a shared
runner). A budget is a hard limit on a metric (input to frame ready under
16 ms), whatever the baseline. A budget with `"hardware": true` applies only
to a run on a hardware GPU (the report's `gpu_hardware`), one with
`"hardware": false` only to a run without one (llvmpipe, WARP, SwiftShader,
a virtual machine's device): CI's runners have no GPU, so the hardware
targets are checked where the benchmark runs on one.

A scenario the baseline doesn't have yet is reported without comparing it
(its budgets still apply).

Timing on CI is noisy, so a failure has to happen twice: with `--rerun`, the
command is run again for just the failing scenarios (`{only}`: their names,
comma-separated; `{json}`: where it writes; `{dir}`: a scratch directory),
and only a scenario that fails in both runs fails the check.

Nothing fails when the baseline is missing or `"provisional": true` (the
budgets still apply), or when the run asked for a GPU renderer the machine
doesn't have (it measured the software renderer instead). Exit status 1
when something failed (unless --no-fail), 0 otherwise. The markdown table
goes to stdout and, with --summary, is appended to that file
($GITHUB_STEP_SUMMARY on GitHub Actions).
"""

import argparse
import fnmatch
import json
import shlex
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent


def load(path):
    try:
        return json.loads(Path(path).read_text(encoding="utf-8"))
    except FileNotFoundError:
        return None


def settings(config, name):
    """The perf/budgets.json settings for scenario `name` (globs allowed)."""
    out = {
        "threshold": config.get("threshold", 1.5),
        "min_delta_ms": config.get("min_delta_ms", 1.0),
        "normalize": True,
        "metric": "median_ms",
    }
    for pattern, s in config.get("scenarios", {}).items():
        if fnmatch.fnmatch(name, pattern):
            out.update(s)
    return out


def budgets_for(config, run, name):
    """The budgets that apply to scenario `name` in `run`."""
    platform = run.get("platform", "")
    family = "web" if platform.startswith("web") else platform.split("-")[0]
    out = []
    for b in config.get("budgets", []):
        if name not in b["scenarios"]:
            continue
        if b.get("platforms") and family not in b["platforms"]:
            continue
        if b.get("renderers") and run.get("renderer") not in b["renderers"]:
            continue
        if "hardware" in b and bool(run.get("gpu_hardware")) != b["hardware"]:
            continue
        out.append(b)
    return out


def budget_limit(budget):
    """A budget's limit and its unit: `ms` for a time, `max` (in the
    metric's own unit) for another metric, such as `texture_mb`."""
    if "ms" in budget:
        return budget["ms"], " ms"
    unit = budget.get("metric", "").rsplit("_", 1)[-1]
    unit = {"mb": "MB", "kb": "kB"}.get(unit, unit)
    return budget["max"], f" {unit}" if unit else ""


def calibration_factor(now, base):
    a, b = now.get("calibration_ms"), base.get("calibration_ms")
    if not a or not b:
        return 1.0
    return min(3.0, max(1 / 3, a / b))


def by_name(run):
    return {s["name"]: s for s in run.get("scenarios", []) if s.get("median_ms") is not None}


def evaluate(now, base, config):
    """Per scenario: (row, failures). A failure is ("regression" | "budget",
    blocking, text)."""
    factor = calibration_factor(now, base) if base else 1.0
    cur = by_name(now)
    old = by_name(base) if base else {}
    gpu_missing = bool(now.get("gpu_unavailable"))
    provisional = not base or base.get("provisional", False) or gpu_missing
    rows = []
    for name, s in cur.items():
        if name == "calibration":
            continue
        cfg = settings(config, name)
        metric = cfg["metric"]
        value = s.get(metric)
        fails = []
        b = old.get(name)
        expected = None
        ratio = None
        if b is not None and b.get(metric) is not None and not gpu_missing:
            f = factor if cfg["normalize"] else 1.0
            expected = b[metric] * f
            ratio = value / expected if expected > 0 else None
            if (
                ratio is not None
                and ratio > cfg["threshold"]
                and value - expected > cfg["min_delta_ms"]
            ):
                fails.append(
                    (
                        "regression",
                        not provisional,
                        f"{ratio:.2f}x the baseline (threshold {cfg['threshold']}x)",
                    )
                )
        for budget in budgets_for(config, now, name):
            what = budget.get("metric", "median_ms")
            got = s.get(what)
            limit, unit = budget_limit(budget)
            if got is not None and got > limit:
                fails.append(
                    (
                        "budget",
                        budget.get("blocking", True),
                        f"{what} {got:.2f}{unit} over the {limit}{unit} budget",
                    )
                )
        rows.append(
            {
                "name": name,
                "renderer": s.get("renderer", ""),
                "metric": metric,
                "base": b.get(metric) if b else None,
                "expected": expected,
                "now": value,
                "p95": s.get("p95_ms"),
                "ratio": ratio,
                "budgets": [
                    "{}{}".format(*budget_limit(bb))
                    + ("" if bb.get("metric", "median_ms") == "median_ms" else f" ({bb['metric']})")
                    + ("" if "hardware" not in bb else " (hardware GPU)" if bb["hardware"] else " (software GPU)")
                    for bb in budgets_for(config, now, name)
                ],
                "work": s.get("work_ms"),
                "fails": fails,
            }
        )
    missing = [n for n in old if n not in cur and n != "calibration"] if not gpu_missing else []
    return rows, missing, factor, provisional


def rerun(command, names):
    """Run `command` again for `names`; the JSON it wrote, or None."""
    with tempfile.TemporaryDirectory() as tmp:
        out = Path(tmp) / "rerun.json"
        # Split first, then fill in: no shell (it runs on Windows too).
        args = [
            part.format(only=",".join(names), json=str(out), dir=tmp)
            for part in shlex.split(command)
        ]
        print(f"re-running: {shlex.join(args)}", flush=True)
        try:
            r = subprocess.run(args, cwd=ROOT)
        except OSError as e:
            print(f"the re-run didn't start: {e}")
            return None
        if r.returncode != 0:
            print(f"the re-run failed ({r.returncode})")
        if out.exists():
            return json.loads(out.read_text(encoding="utf-8"))
        found = sorted(Path(tmp).glob("*.json"))
        return json.loads(found[0].read_text(encoding="utf-8")) if found else None


def fmt(v, digits=2):
    return "–" if v is None else f"{v:.{digits}f}"


def markdown(title, now, base, rows, missing, factor, provisional, reran):
    lines = [f"### Performance: {title}", ""]
    meta = [f"renderer **{now.get('renderer', '?')}**"]
    if now.get("gpu_unavailable"):
        meta.append(f"⚠️ asked for {now.get('renderer_requested')}: {now['gpu_unavailable']}")
    if now.get("gpu_device"):
        kind = "" if "gpu_hardware" not in now else " (hardware)" if now["gpu_hardware"] else " (software)"
        meta.append(f"device {now['gpu_device']}{kind}")
    if base:
        meta.append(f"calibration {fmt(now.get('calibration_ms'))} ms (baseline {fmt(base.get('calibration_ms'))} ms, scale ×{factor:.2f})")
    else:
        meta.append("no baseline yet: recording only")
    if base and base.get("provisional"):
        meta.append("provisional baseline: regressions are reported, not failed")
    lines.append(" · ".join(meta))
    lines.append("")
    work = any(r["work"] is not None for r in rows)
    lines.append(
        "| scenario | renderer | baseline ms | expected ms | now ms | p95 ms | delta |"
        + (" work ms |" if work else "")
        + " budget | |"
    )
    lines.append("|---|---|--:|--:|--:|--:|--:|" + ("--:|" if work else "") + "--:|---|")
    for r in rows:
        delta = "–" if r["ratio"] is None else f"{(r['ratio'] - 1) * 100:+.0f}%"
        status = "✅"
        notes = []
        for kind, blocking, text in r["fails"]:
            status = "❌" if blocking else "⚠️"
            notes.append(text)
        if r["name"] in reran:
            notes.append(reran[r["name"]])
        metric = "" if r["metric"] == "median_ms" else f" ({r['metric']})"
        lines.append(
            f"| {r['name']}{metric} | {r['renderer']} | {fmt(r['base'])} | {fmt(r['expected'])} | "
            f"{fmt(r['now'])} | {fmt(r['p95'])} | {delta} | "
            + (f"{fmt(r['work'])} | " if work else "")
            + f"{', '.join(r['budgets']) or '–'} | "
            f"{status} {'; '.join(notes)} |"
        )
    if missing:
        lines.append("")
        lines.append(f"Not measured this time (in the baseline): {', '.join(missing)}")
    lines.append("")
    return "\n".join(lines)


def main():
    # Windows consoles default to cp1252, which can't print the table's marks.
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("run", type=Path, help="this run's JSON")
    ap.add_argument("--baseline", type=Path, help="the baseline's JSON (missing: recording only)")
    ap.add_argument("--budgets", type=Path, default=ROOT / "perf" / "budgets.json")
    ap.add_argument("--rerun", help="command re-running the failing scenarios: {only}, {json}, {dir}")
    ap.add_argument("--summary", type=Path, help="append the markdown table to this file")
    ap.add_argument("--title", default=None)
    ap.add_argument("--no-fail", action="store_true", help="report, but exit 0")
    a = ap.parse_args()

    now = load(a.run)
    if now is None:
        sys.exit(f"no {a.run}")
    base = load(a.baseline) if a.baseline else None
    config = load(a.budgets) or {}
    title = a.title or a.run.stem

    rows, missing, factor, provisional = evaluate(now, base, config)
    reran = {}
    failing = [r["name"] for r in rows if any(blocking for _, blocking, _ in r["fails"])]
    if failing and a.rerun:
        again = rerun(a.rerun, failing)
        if again is None:
            print("the re-run wrote no JSON: keeping the first run's verdict")
        else:
            rows2, _, _, _ = evaluate(again, base, config)
            second = {r["name"]: r for r in rows2}
            for r in rows:
                if r["name"] not in failing:
                    continue
                r2 = second.get(r["name"])
                if r2 is None:
                    continue
                # Fails only what failed both times.
                kinds2 = {k for k, blocking, _ in r2["fails"] if blocking}
                kept = []
                for kind, blocking, text in r["fails"]:
                    if blocking and kind not in kinds2:
                        kept.append((kind, False, f"{text}, not on a re-run ({fmt(r2['now'])} ms)"))
                    else:
                        kept.append((kind, blocking, text))
                r["fails"] = kept
                reran[r["name"]] = f"re-run: {fmt(r2['now'])} ms"

    md = markdown(title, now, base, rows, missing, factor, provisional, reran)
    print(md)
    if a.summary:
        with open(a.summary, "a", encoding="utf-8") as f:
            f.write(md + "\n")
    failed = [r["name"] for r in rows if any(blocking for _, blocking, _ in r["fails"])]
    if failed:
        print(f"FAIL: {', '.join(failed)}")
        if not a.no_fail:
            sys.exit(1)
    else:
        print("ok")


if __name__ == "__main__":
    main()
