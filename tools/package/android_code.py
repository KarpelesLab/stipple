"""The app's own Android code and manifest entries: Java sources compiled with
the SDK's tools (`android.java`), prebuilt dex files (`android.dex`), and a
manifest fragment (`android.manifest-fragment`) merged into the manifest
cargo-stipple writes.

The tools are the developer's (a JDK's `javac`, the build-tools' `d8`, the
platform's `android.jar`), found at packaging time; nothing here is linked
into the app.
"""

import os
import re
import shutil
import xml.etree.ElementTree as ET
import zipfile
from pathlib import Path

from common import PackageError, log, run, warn

ANDROID_NS = "http://schemas.android.com/apk/res/android"
A = "{" + ANDROID_NS + "}"

# What a fragment may add at the top of the manifest. <uses-sdk> comes from
# android.min-sdk / target-sdk, and the <application> is merged child by child.
TOP_LEVEL = {
    "uses-permission",
    "uses-permission-sdk-23",
    "uses-feature",
    "queries",
    "permission",
    "permission-group",
    "permission-tree",
    "supports-gl-texture",
    "supports-screens",
}

# The <application> children that name a class: no two may name the same one.
COMPONENTS = {"activity", "activity-alias", "service", "receiver", "provider"}

# The Java language level the sources compile as: what every API level
# Stipple supports runs (d8 desugars lambdas and default methods).
JAVA_RELEASE = "8"


# --- Java sources and dex files ------------------------------------------------


def find_javac():
    """A JDK's javac: $JAVA_HOME/bin/javac, else the one on PATH."""
    home = os.environ.get("JAVA_HOME")
    if home:
        for name in ("javac", "javac.exe"):
            p = Path(home) / "bin" / name
            if p.exists():
                return p
    p = shutil.which("javac")
    if p:
        return Path(p)
    raise PackageError(
        "android.java needs a JDK's javac (17 or later): set JAVA_HOME or put javac on PATH"
    )


def find_d8(sdk):
    for name in ("d8", "d8.bat"):
        p = sdk.build_tools / name
        if p.exists():
            return p
    raise PackageError(
        f"android.java needs d8, and {sdk.build_tools} has none "
        "(sdkmanager 'build-tools;35.0.0')"
    )


def java_sources(dirs):
    """Every .java file under the source directories, sorted."""
    files = []
    for d in dirs:
        files += sorted(p for p in Path(d).rglob("*.java") if p.is_file())
    if not files:
        raise PackageError(f"android.java: no .java files in {', '.join(map(str, dirs))}")
    return files


def compile_java(sdk, dirs, out, min_sdk, release=True):
    """The app's Java sources, compiled against the platform's android.jar
    and dexed: a list of dex files (one, unless there are more than 64K
    methods)."""
    javac, d8 = find_javac(), find_d8(sdk)
    sources = java_sources(dirs)
    out = Path(out)
    if out.exists():
        shutil.rmtree(out)
    classes, dex_dir = out / "classes", out / "dex"
    classes.mkdir(parents=True)
    dex_dir.mkdir()
    args = out / "sources.txt"
    args.write_text("".join(f'"{_arg(p)}"\n' for p in sources), encoding="utf-8")
    run(
        [javac, "-Xlint:-options", "--release", JAVA_RELEASE, "-encoding", "UTF-8",
         "-classpath", sdk.android_jar, "-sourcepath", os.pathsep.join(map(str, dirs)),
         "-d", classes, f"@{args}"],
        quiet=True,
    )
    jar = out / "classes.jar"
    with zipfile.ZipFile(jar, "w") as z:
        for p in sorted(classes.rglob("*.class")):
            z.write(p, p.relative_to(classes).as_posix())
    run(
        [d8, "--release" if release else "--debug", "--min-api", max(min_sdk, 21),
         "--lib", sdk.android_jar, "--output", dex_dir, jar],
        quiet=True,
    )
    dexes = sorted(dex_dir.glob("classes*.dex"), key=_dex_index)
    log(f"compiled {len(sources)} Java source(s) into {len(dexes)} dex file(s)")
    return dexes


def _arg(p):
    # javac's @file: quoted, with backslashes doubled.
    return str(p).replace("\\", "\\\\")


def _dex_index(p):
    digits = p.stem.removeprefix("classes")
    return int(digits) if digits else 1


def check_dex(path):
    """A prebuilt dex file, checked to be one (its `dex\\n` magic)."""
    data = Path(path).read_bytes()[:8]
    if len(data) < 8 or data[:4] != b"dex\n":
        raise PackageError(f"android.dex: {path} isn't a dex file")
    return Path(path)


def dex_names(paths):
    """(name in the APK, path) for each dex file in order: classes.dex,
    classes2.dex, ..."""
    return [("classes.dex" if i == 0 else f"classes{i + 1}.dex", p) for i, p in enumerate(paths)]


# --- The manifest fragment -----------------------------------------------------


def _qualify(name, package):
    """A component's class as Android resolves it: `.Foo` and `Foo` are in
    the app's package."""
    if name.startswith("."):
        return package + name
    if "." not in name:
        return f"{package}.{name}"
    return name


def _describe(el):
    name = el.get(A + "name") or el.get(A + "glEsVersion")
    return f"<{el.tag} android:name=\"{name}\">" if name else f"<{el.tag}>"


def _check_namespaces(root, path):
    for el in root.iter():
        if el.tag.startswith("{"):
            raise PackageError(f"{path}: <{el.tag}>: elements are in no namespace")
        for k in el.attrib:
            if k.startswith("{") and not k.startswith(A):
                ns = k[1 : k.index("}")]
                hint = (
                    " (Gradle's manifest merger instructions aren't supported)"
                    if ns == "http://schemas.android.com/tools"
                    else ""
                )
                raise PackageError(f"{path}: attribute {k} in namespace {ns}{hint}")


def _key(el):
    """What makes a top-level or non-component <application> entry unique."""
    return (el.tag, el.get(A + "name") or el.get(A + "glEsVersion"))


def _serialize(el, indent):
    el = _copy(el)
    el.tail = None
    ET.indent(el, space="  ")
    text = ET.tostring(el, encoding="unicode")
    text = re.sub(r'\s+xmlns:android="[^"]+"', "", text, count=1)
    return "\n".join(indent + line for line in text.splitlines())


def _copy(el):
    new = ET.Element(el.tag, dict(el.attrib))
    new.text = el.text if el.text and el.text.strip() else None
    for c in el:
        new.append(_copy(c))
    return new


def merge_fragment(manifest, fragment, package, path="manifest fragment"):
    """`manifest` (the text of an AndroidManifest.xml) with the entries of
    `fragment` (the text of one whose root is a bare <manifest>) added: its
    top-level entries before <application>, its <application>'s children at
    the end of the manifest's <application>, and that element's attributes on
    it. `${applicationId}` in the fragment is the package.

    An entry the manifest already has (the same permission, component class,
    provider authority, <meta-data> name) or an <application> attribute with
    another value is an error, not a duplicate. Returns (text, components
    added)."""
    ET.register_namespace("android", ANDROID_NS)
    try:
        frag = ET.fromstring(fragment.replace("${applicationId}", package))
    except ET.ParseError as e:
        raise PackageError(f"{path}: {e}")
    try:
        root = ET.fromstring(manifest)
    except ET.ParseError as e:
        raise PackageError(f"the manifest the fragment goes into: {e}")
    if frag.tag != "manifest":
        raise PackageError(f"{path}: the root element must be <manifest>, not <{frag.tag}>")
    if frag.attrib:
        raise PackageError(
            f"{path}: <manifest> takes no attributes here ({', '.join(frag.attrib)}): "
            "the package, versions and SDK levels come from the packaging manifest"
        )
    _check_namespaces(frag, path)
    app = root.find("application")
    if app is None:
        raise PackageError("the manifest has no <application> to merge the fragment into")

    have_top = {_key(el) for el in root if el.tag not in ("application", "uses-sdk", "queries")}
    have_classes = {}
    have_authorities = set()
    have_app = set()
    for el in app:
        if el.tag in COMPONENTS:
            if el.get(A + "name"):
                have_classes[_qualify(el.get(A + "name"), package)] = el.tag
            for a in (el.get(A + "authorities") or "").split(";"):
                if a.strip():
                    have_authorities.add(a.strip())
        else:
            have_app.add(_key(el))

    top, children, attrs = [], [], {}
    seen_app = False
    for el in frag:
        if el.tag == "application":
            if seen_app:
                raise PackageError(f"{path}: more than one <application>")
            seen_app = True
            for k, v in el.attrib.items():
                short = k.replace(A, "android:")
                if k in app.attrib:
                    if app.attrib[k] != v:
                        raise PackageError(
                            f"{path}: <application {short}=\"{v}\">: the manifest already "
                            f"says {short}=\"{app.attrib[k]}\""
                        )
                    continue
                attrs[k] = v
            for child in el:
                if child.tag in COMPONENTS:
                    name = child.get(A + "name")
                    if not name:
                        raise PackageError(f"{path}: <{child.tag}> without android:name")
                    cls = _qualify(name, package)
                    if cls in have_classes:
                        raise PackageError(
                            f"{path}: {_describe(child)}: the manifest already declares "
                            f"{cls} (a <{have_classes[cls]}>"
                            + (", one of Stipple's own" if cls.startswith("org.stipple.") or cls.startswith("android.app.") else "")
                            + ")"
                        )
                    have_classes[cls] = child.tag
                    # Written out in full, as Gradle's merger does: tools
                    # reading the manifest needn't resolve ".Name".
                    child.set(A + "name", cls)
                    target = child.get(A + "targetActivity")
                    if target:
                        child.set(A + "targetActivity", _qualify(target, package))
                    for a in (child.get(A + "authorities") or "").split(";"):
                        a = a.strip()
                        if not a:
                            continue
                        if a in have_authorities:
                            raise PackageError(
                                f"{path}: {_describe(child)}: the authority {a} is already taken"
                            )
                        have_authorities.add(a)
                else:
                    key = _key(child)
                    if key in have_app:
                        raise PackageError(
                            f"{path}: <application> {_describe(child)} is already in the manifest"
                        )
                    have_app.add(key)
                children.append(child)
        elif el.tag in TOP_LEVEL:
            if el.tag != "queries":
                key = _key(el)
                if key in have_top:
                    raise PackageError(
                        f"{path}: {_describe(el)} is already in the manifest "
                        "(from android.permissions or a Stipple feature): drop it from the fragment"
                    )
                have_top.add(key)
            top.append(el)
        elif el.tag == "uses-sdk":
            raise PackageError(f"{path}: <uses-sdk> comes from android.min-sdk and target-sdk")
        else:
            raise PackageError(
                f"{path}: <{el.tag}> can't be merged (top level: {', '.join(sorted(TOP_LEVEL))}, "
                "and <application>)"
            )

    text = manifest
    if attrs:
        at = _open_tag_end(text, "<application")
        extra = "".join(
            f"\n      {k.replace(A, 'android:')}=\"{_attr(v)}\"" for k, v in attrs.items()
        )
        text = text[:at] + extra + text[at:]
    if top:
        at = text.index("<application")
        line = text.rfind("\n", 0, at) + 1
        text = text[:line] + "".join(_serialize(el, "  ") + "\n" for el in top) + text[line:]
    if children:
        at = text.rindex("</application>")
        line = text.rfind("\n", 0, at) + 1
        if not text[line:at].strip():
            text = text[:line] + "".join(_serialize(el, "    ") + "\n" for el in children) + text[line:]
        else:
            text = text[:at] + "".join(_serialize(el, "    ") + "\n" for el in children) + text[at:]
    components = [c for c in children if c.tag in COMPONENTS]
    return text, components


def _open_tag_end(text, tag):
    """Where the attributes of the first `tag` end (before its `>` or `/>`)."""
    at = text.index(tag) + len(tag)
    quote = None
    while at < len(text):
        c = text[at]
        if quote:
            if c == quote:
                quote = None
        elif c in "\"'":
            quote = c
        elif c == ">":
            return at - 1 if text[at - 1] == "/" else at
        at += 1
    raise PackageError(f"the manifest's {tag}> isn't closed")


def _attr(v):
    return v.replace("&", "&amp;").replace('"', "&quot;").replace("<", "&lt;")


def merge_fragment_file(manifest_path, fragment_path, package, has_code):
    """Merge the fragment file into the manifest file in place."""
    text = Path(manifest_path).read_text(encoding="utf-8")
    fragment = Path(fragment_path).read_text(encoding="utf-8")
    merged, components = merge_fragment(text, fragment, package, path=str(fragment_path))
    Path(manifest_path).write_text(merged, encoding="utf-8")
    if components and not has_code:
        warn(
            "the manifest fragment declares components, but the app has no dex of its own "
            "(android.java or android.dex): Android won't find their classes"
        )
    log(f"merged {fragment_path} into the manifest")
