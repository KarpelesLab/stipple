#!/usr/bin/env python3
"""Fetch Google Play Billing Library and package it for a Gradle-less APK.

Stipple's Android backend drives the Play Billing Library
(com.android.billingclient:billing) over JNI, with no Java of its own: it
looks the library's classes up in the app's class loader at run time. A
Gradle build gets them from `implementation("com.android.billingclient:billing:X")`;
an APK built without Gradle (examples/androiddemo/build-apk.sh) gets them from
this script, which does what Gradle would:

  1. resolves the library's dependency closure from Google's Maven repository
     and Maven Central (compile and runtime scopes; the highest version asked
     for wins, as Gradle's resolution does),
  2. downloads each AAR or JAR into a cache and takes its classes,
  3. runs the SDK's d8 over them (release mode) into classes2.dex,
     classes3.dex, ... in OUT_DIR (classes.dex stays the app's own),
  4. writes OUT_DIR/manifest-fragment.xml: the <uses-permission>, <queries>
     and <application> children of every AAR's manifest, with
     ${applicationId} filled in. Entries naming resources (`@integer/...`)
     are left out, since an aapt build doesn't merge the AARs' resources; none
     of them is on the billing path.

  tools/android/billing.py OUT_DIR [--version 8.3.0] [--package com.example.app]
      [--min-api 24] [--cache DIR] [--manifest AndroidManifest.xml]

With --manifest, OUT_DIR/AndroidManifest.xml is that manifest with the
entries added (and --package defaults to its package).

An aapt build doesn't compile the AARs' resources either (no R classes),
which the 8.x closure doesn't need at run time but 9.x's does (androidx.startup
reads its R.string): use 8.x here (8.0.0 and 8.3.0 were run), or Gradle for 9.

Needs ANDROID_HOME (for d8 and android.jar) and network access to
dl.google.com and repo1.maven.org (the cache makes the next run offline).
The library is Google's, under the Android Software Development Kit License;
the AARs' third_party_licenses files list what's inside.
"""

import argparse
import glob
import os
import re
import subprocess
import sys
import urllib.request
import xml.etree.ElementTree as ET
import zipfile

REPOS = [
    "https://dl.google.com/android/maven2",
    "https://repo1.maven.org/maven2",
]
POM = "{http://maven.apache.org/POM/4.0.0}"
ANDROID = "{http://schemas.android.com/apk/res/android}"


def version_key(v):
    return [int(p) if p.isdigit() else p for p in re.split(r"[.\-]", v)]


def fetch(cache, path):
    """The bytes of repository file `path` (group/artifact/version/file), from
    the cache or the first repository that has it; None if none does."""
    local = os.path.join(cache, path)
    if os.path.exists(local):
        with open(local, "rb") as f:
            return f.read()
    for repo in REPOS:
        try:
            with urllib.request.urlopen(f"{repo}/{path}", timeout=60) as r:
                data = r.read()
        except Exception:
            continue
        os.makedirs(os.path.dirname(local), exist_ok=True)
        with open(local, "wb") as f:
            f.write(data)
        return data
    return None


def coords_path(group, artifact, version, ext):
    return f"{group.replace('.', '/')}/{artifact}/{version}/{artifact}-{version}.{ext}"


def clean_version(v):
    """A plain version from a Maven range or a fixed requirement: `[1.2.3]`,
    `[1.0,2.0)` (the lower bound)."""
    v = v.strip()
    if v.startswith("[") or v.startswith("("):
        v = v.strip("[]()").split(",")[0].strip()
    return v


def pom_deps(cache, group, artifact, version):
    """(group, artifact, version, type) of a POM's compile and runtime
    dependencies, properties and the parent's dependencyManagement applied
    as far as these artifacts need."""
    data = fetch(cache, coords_path(group, artifact, version, "pom"))
    if data is None:
        return [], "jar"
    root = ET.fromstring(data)
    props = {"project.version": version}
    for p in root.findall(f"{POM}properties/*"):
        props[p.tag.replace(POM, "")] = (p.text or "").strip()
    managed = {}
    parent = root.find(f"{POM}parent")
    if parent is not None:
        pg = parent.findtext(f"{POM}groupId")
        pa = parent.findtext(f"{POM}artifactId")
        pv = parent.findtext(f"{POM}version")
        pdata = fetch(cache, coords_path(pg, pa, pv, "pom"))
        if pdata is not None:
            proot = ET.fromstring(pdata)
            for p in proot.findall(f"{POM}properties/*"):
                props.setdefault(p.tag.replace(POM, ""), (p.text or "").strip())
            for d in proot.findall(f"{POM}dependencyManagement/{POM}dependencies/{POM}dependency"):
                managed[(d.findtext(f"{POM}groupId"), d.findtext(f"{POM}artifactId"))] = d.findtext(
                    f"{POM}version"
                )

    def sub(s):
        return re.sub(r"\$\{([^}]+)\}", lambda m: props.get(m.group(1), m.group(0)), s or "")

    packaging = (root.findtext(f"{POM}packaging") or "jar").strip()
    out = []
    for d in root.findall(f"{POM}dependencies/{POM}dependency"):
        scope = (d.findtext(f"{POM}scope") or "compile").strip()
        if scope not in ("compile", "runtime") or (d.findtext(f"{POM}optional") or "") == "true":
            continue
        g, a = sub(d.findtext(f"{POM}groupId")), sub(d.findtext(f"{POM}artifactId"))
        v = sub(d.findtext(f"{POM}version")) or sub(managed.get((g, a)))
        if not v:
            continue
        out.append((g, a, clean_version(v), (d.findtext(f"{POM}type") or "").strip()))
    return out, packaging


def resolve(cache, root):
    """The dependency closure of `root` (group, artifact, version): artifact ->
    (version, packaging), the highest version asked for winning."""
    chosen = {}
    todo = [root]
    while todo:
        g, a, v = todo.pop()
        key = (g, a)
        if key in chosen and version_key(chosen[key][0]) >= version_key(v):
            continue
        deps, packaging = pom_deps(cache, g, a, v)
        chosen[key] = (v, packaging)
        for dg, da, dv, _ in deps:
            todo.append((dg, da, dv))
    return chosen


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("out")
    ap.add_argument("--version", default="8.3.0")
    ap.add_argument("--package", default=None)
    ap.add_argument("--min-api", default="24")
    ap.add_argument("--manifest", help="an app manifest to write OUT/AndroidManifest.xml from, with the entries added")
    ap.add_argument("--cache", default=os.path.expanduser("~/.cache/stipple/maven"))
    args = ap.parse_args()

    if args.package is None:
        args.package = "${applicationId}"
        if args.manifest:
            m = re.search(r'package="([^"]+)"', open(args.manifest).read())
            if m:
                args.package = m.group(1)

    sdk = os.environ.get("ANDROID_HOME") or os.environ.get("ANDROID_SDK_ROOT")
    if not sdk:
        sys.exit("set ANDROID_HOME to the Android SDK")
    tools = sorted(glob.glob(f"{sdk}/build-tools/*"), key=lambda p: version_key(os.path.basename(p)))
    jars = sorted(glob.glob(f"{sdk}/platforms/android-*/android.jar"))
    if not tools or not jars:
        sys.exit("the SDK needs build-tools and a platform")
    d8, android_jar = f"{tools[-1]}/d8", jars[-1]

    os.makedirs(args.out, exist_ok=True)
    work = os.path.join(args.out, "jars")
    os.makedirs(work, exist_ok=True)
    closure = resolve(args.cache, ("com.android.billingclient", "billing", args.version))
    # kotlin-stdlib 1.8 took in the classes of kotlin-stdlib-jdk7 and -jdk8
    # (Gradle's module metadata upgrades those to empty 1.8 artifacts); an
    # older -jdk7/-jdk8 next to it would define them twice.
    stdlib = closure.get(("org.jetbrains.kotlin", "kotlin-stdlib"))
    if stdlib and version_key(stdlib[0]) >= version_key("1.8"):
        for extra in ("kotlin-stdlib-jdk7", "kotlin-stdlib-jdk8"):
            closure.pop(("org.jetbrains.kotlin", extra), None)
    classes, manifests = [], []
    for (g, a), (v, packaging) in sorted(closure.items()):
        ext = "aar" if packaging == "aar" else "jar"
        data = fetch(args.cache, coords_path(g, a, v, ext))
        if data is None and ext == "jar":
            ext, data = "aar", fetch(args.cache, coords_path(g, a, v, "aar"))
        if data is None:
            print(f"  skipped {g}:{a}:{v} (no artifact)", file=sys.stderr)
            continue
        local = os.path.join(args.cache, coords_path(g, a, v, ext))
        if ext == "jar":
            classes.append(local)
        else:
            with zipfile.ZipFile(local) as z:
                names = z.namelist()
                if "classes.jar" in names:
                    jar = os.path.join(work, f"{g}-{a}-{v}.jar")
                    with open(jar, "wb") as f:
                        f.write(z.read("classes.jar"))
                    classes.append(jar)
                for n in names:
                    if n.startswith("libs/") and n.endswith(".jar"):
                        jar = os.path.join(work, f"{g}-{a}-{v}-{os.path.basename(n)}")
                        with open(jar, "wb") as f:
                            f.write(z.read(n))
                        classes.append(jar)
                if "AndroidManifest.xml" in names:
                    manifests.append((f"{g}:{a}:{v}", z.read("AndroidManifest.xml")))
        print(f"  {g}:{a}:{v}", file=sys.stderr)

    dex_dir = os.path.join(args.out, "dex")
    os.makedirs(dex_dir, exist_ok=True)
    for old in glob.glob(f"{dex_dir}/*.dex"):
        os.remove(old)
    # d8's notes about the library's bytecode (missing EnclosingMethod
    # attributes) are noise; its output is shown only if it fails.
    r = subprocess.run(
        [d8, "--release", "--min-api", args.min_api, "--lib", android_jar, "--output", dex_dir]
        + classes,
        capture_output=True,
        text=True,
    )
    if r.returncode != 0:
        sys.exit(f"d8 failed:\n{r.stdout}{r.stderr}")
    # d8 names its output classes.dex, classes2.dex, ...; the APK's own
    # classes.dex comes first, so these move up one.
    produced = sorted(glob.glob(f"{dex_dir}/classes*.dex"), key=lambda p: version_key(re.sub(r"\D", "", os.path.basename(p)) or "1"))
    for old in glob.glob(f"{args.out}/classes*.dex"):
        os.remove(old)
    for i, p in enumerate(produced):
        os.replace(p, os.path.join(args.out, f"classes{i + 2}.dex"))

    write_manifests(args.out, manifests, args.package, args.manifest)
    print(f"wrote {len(produced)} dex file(s) and manifest-fragment.xml to {args.out}", file=sys.stderr)


def merge_manifests(manifests, package):
    """The <manifest>-level and <application>-level entries of the AARs'
    manifests, as XML text lines: duplicates dropped, components of the same
    name merged (their children joined), entries naming a resource left out."""
    ET.register_namespace("android", ANDROID.strip("{}"))
    top, app, notes = [], {}, []
    order = []

    def names_resource(el):
        return any(
            v.startswith("@") and not v.startswith("@android:")
            for e in el.iter()
            for v in e.attrib.values()
        )

    for name, data in manifests:
        root = ET.fromstring(data.decode("utf-8").replace("${applicationId}", package))
        for el in list(root):
            if el.tag != "application" and names_resource(el):
                notes.append(f"<!-- left out (names a resource): {el.tag} from {name} -->")
            elif el.tag in ("uses-permission", "queries", "permission", "uses-feature"):
                top.append(el)
            elif el.tag == "application":
                for child in el:
                    if names_resource(child):
                        notes.append(f"<!-- left out (names a resource): {child.tag} from {name} -->")
                        continue
                    key = (child.tag, child.get(f"{ANDROID}name"))
                    if key in app:
                        have = {ET.tostring(c) for c in app[key]}
                        app[key].extend(c for c in child if ET.tostring(c) not in have)
                        app[key].attrib.update(child.attrib)
                    else:
                        app[key] = child
                        order.append(key)

    def text(el):
        for e in el.iter():
            e.tail = None
            if e.text is not None and not e.text.strip():
                e.text = None
        s = ET.tostring(el, encoding="unicode")
        return re.sub(r'\s+xmlns:android="[^"]+"', "", s)

    top_lines, seen = [], set()
    for el in top:
        s = text(el)
        if s not in seen:
            seen.add(s)
            top_lines.append(s)
    app_lines = [text(app[k]) for k in order] + notes
    return top_lines, app_lines


def write_manifests(out, manifests, package, app_manifest):
    top, app = merge_manifests(manifests, package)
    with open(os.path.join(out, "manifest-fragment.xml"), "w") as f:
        f.write("<!-- Generated by tools/android/billing.py: the entries the Play Billing\n")
        f.write("     Library and its dependencies add to an app's manifest. The first part\n")
        f.write("     goes in <manifest>, the part in <application> in <application>. -->\n")
        f.write('<fragment xmlns:android="http://schemas.android.com/apk/res/android">\n')
        for line in top:
            f.write(f"  {line}\n")
        f.write("  <application>\n")
        for line in app:
            f.write(f"    {line}\n")
        f.write("  </application>\n</fragment>\n")
    if app_manifest:
        with open(app_manifest) as f:
            s = f.read()
        at = s.index("<application")
        s = s[:at] + "".join(f"{l}\n  " for l in top) + s[at:]
        at = s.rindex("</application>")
        s = s[:at] + "".join(f"  {l}\n  " for l in app) + s[at:]
        with open(os.path.join(out, "AndroidManifest.xml"), "w") as f:
            f.write(s)


if __name__ == "__main__":
    main()
