#!/usr/bin/env python3
"""Audit Stipple's public API against docs/api-guidelines.md.

Reads rustdoc's JSON for the crate (a nightly feature):

    RUSTDOCFLAGS="-Z unstable-options --output-format json" \\
      cargo +nightly doc --no-deps --lib --target-dir target/apijson
    python3 .github/scripts/api-audit.py target/apijson/doc/stipple.json

and lists, for every item reachable from outside the crate that isn't
#[doc(hidden)]:
  - structs with public fields that aren't #[non_exhaustive];
  - enums that aren't #[non_exhaustive];
  - items reachable at more than one public path (the prelude aside);
  - functions and methods with 5 or more parameters (not counting `self`,
    `cx` and `theme`), or a run of 3 or more parameters of the same
    number or bool type;
  - names (docs/api-guidelines.md, "Naming"): by-value builders
    (`self -> Self`) named `with_*`, where a setter takes the bare property
    name, and `&self` methods named `get_*`, where a getter does.
Items listed in api-audit-allow.txt (one path per line, `# reason` after it)
are deliberate exceptions. Exits 1 if anything else is found.
"""
import json
import os
import sys

PRIMITIVES = {"f32", "f64", "bool", "u8", "u16", "u32", "u64", "usize", "i32", "i64", "isize"}
CONTEXT_PARAMS = {"self", "cx", "theme"}


def main():
    path = sys.argv[1] if len(sys.argv) > 1 else "target/apijson/doc/stipple.json"
    d = json.load(open(path))
    idx = d["index"]
    allow_path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "api-audit-allow.txt")
    allow = set()
    if os.path.exists(allow_path):
        for line in open(allow_path):
            line = line.split("#", 1)[0].strip()
            if line:
                allow.add(line)

    def kind(it):
        return next(iter(it["inner"]))

    def attrs(it):
        return " ".join(str(a) for a in it.get("attrs", []))

    def hidden(it):
        return "doc(hidden)" in attrs(it)

    # Walk the module tree from the root, following public re-exports, to find
    # every item's (first) public path.
    seen = {}
    all_paths = {}
    mods = {}

    def walk(mid, prefix, visiting):
        if mid in visiting or str(mid) not in idx:
            return
        visiting = visiting | {mid}
        for cid in idx[str(mid)]["inner"]["module"]["items"]:
            c = idx.get(str(cid))
            if c is None or c["visibility"] != "public" or hidden(c):
                continue
            k = kind(c)
            if k == "use":
                u = c["inner"]["use"]
                tid = u.get("id")
                if tid is None or str(tid) not in idx:
                    continue
                t = idx[str(tid)]
                if hidden(t):
                    continue
                if u.get("is_glob"):
                    if kind(t) == "module":
                        walk(tid, prefix, visiting)
                    continue
                if kind(t) == "module":
                    mods.setdefault(str(tid), set()).add(f"{prefix}::{u['name']}")
                    walk(tid, f"{prefix}::{u['name']}", visiting)
                else:
                    seen.setdefault(str(tid), f"{prefix}::{u['name']}")
                    all_paths.setdefault(str(tid), set()).add(f"{prefix}::{u['name']}")
            elif k == "module":
                mods.setdefault(str(cid), set()).add(f"{prefix}::{c['name']}")
                walk(cid, f"{prefix}::{c['name']}", visiting)
            else:
                seen.setdefault(str(cid), f"{prefix}::{c['name']}")
                all_paths.setdefault(str(cid), set()).add(f"{prefix}::{c['name']}")

    walk(d["root"], "stipple", frozenset())

    problems = []

    def check_fn(p, fn):
        inputs = fn["sig"]["inputs"]
        named = [(n, t) for n, t in inputs if n not in CONTEXT_PARAMS]
        if len(named) >= 5:
            problems.append(("params", p, f"{len(named)} parameters"))
            return
        run, prev = 1, None
        for _, t in named:
            prim = t.get("primitive") if isinstance(t, dict) else None
            if prim in PRIMITIVES and prim == prev:
                run += 1
                if run >= 3:
                    problems.append(("params", p, f"{run}+ `{prim}` parameters in a row"))
                    return
            else:
                run = 1
            prev = prim

    def check_naming(p, type_id, fn, name):
        inputs = fn["sig"]["inputs"]
        if not inputs or inputs[0][0] != "self":
            return
        recv = inputs[0][1]
        borrowed = recv.get("borrowed_ref") if isinstance(recv, dict) else None
        if name.startswith("with_") and borrowed is None:
            out = fn["sig"].get("output")
            returns_self = isinstance(out, dict) and (
                out.get("generic") == "Self"
                or str(out.get("resolved_path", {}).get("id")) == type_id
            )
            if returns_self:
                problems.append(("naming", p, "a `with_*` builder: setters take the bare name"))
        elif name.startswith("get_") and borrowed is not None and not borrowed["is_mutable"]:
            problems.append(("naming", p, "a `get_*` method: getters take the bare name"))

    for iid, p in sorted(seen.items(), key=lambda x: x[1]):
        it = idx[iid]
        k = kind(it)
        ne = "non_exhaustive" in attrs(it)
        if k == "struct":
            s = it["inner"]["struct"]["kind"]
            if isinstance(s, dict) and "plain" in s and not ne:
                pubf = [idx[str(f)]["name"] for f in s["plain"]["fields"]
                        if str(f) in idx and idx[str(f)]["visibility"] == "public"]
                if pubf:
                    problems.append(("struct", p, "public fields: " + ", ".join(pubf)))
        elif k == "enum" and not ne:
            problems.append(("enum", p, f"{len(it['inner']['enum']['variants'])} variants"))
        elif k == "function":
            check_fn(p, it["inner"]["function"])
        if k in ("struct", "enum"):
            for impl_id in it["inner"][k]["impls"]:
                im = idx.get(str(impl_id))
                if not im or im["inner"]["impl"].get("trait"):
                    continue
                for mid in im["inner"]["impl"]["items"]:
                    m = idx.get(str(mid))
                    if m and m["visibility"] == "public" and kind(m) == "function" and not hidden(m):
                        check_fn(f"{p}::{m['name']}", m["inner"]["function"])
                        check_naming(f"{p}::{m['name']}", iid, m["inner"]["function"], m["name"])

    # One canonical path per item: the prelude may repeat any of them.
    for table in (all_paths, mods):
        for iid, ps in table.items():
            outside = sorted(p for p in ps if not p.startswith("stipple::prelude::"))
            if len(outside) > 1:
                problems.append(("paths", outside[0], "also at " + ", ".join(outside[1:])))

    found = [x for x in problems if x[1] not in allow]
    stale = sorted(allow - {x[1] for x in problems})
    titles = {
        "struct": "structs with public fields, not #[non_exhaustive]",
        "enum": "enums not #[non_exhaustive]",
        "params": "functions with many (or easily swapped) parameters",
        "paths": "items with more than one public path (outside the prelude)",
        "naming": "setters named `with_*`, getters named `get_*`",
    }
    for key, title in titles.items():
        items = [x for x in found if x[0] == key]
        if items:
            print(f"== {title}: {len(items)}")
            for _, p, why in items:
                print(f"  {p}  ({why})")
    if stale:
        print("== allowlisted but no longer flagged (remove from api-audit-allow.txt):")
        for p in stale:
            print(f"  {p}")
    print(f"{len(seen)} public items; {len(found)} to look at, {len(problems) - len(found)} allowed")
    sys.exit(1 if found else 0)


if __name__ == "__main__":
    main()
