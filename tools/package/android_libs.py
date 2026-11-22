"""Android libraries for the app's own Java (`android.libraries`), and the
app's own resources (`android.res`), with no Gradle.

A library is a plain `.jar` (classes) or an `.aar` (an Android library: a
zip of `classes.jar`, more jars under `libs/`, an `AndroidManifest.xml`, a
`res/` tree, perhaps `assets/` and native libraries under `jni/<abi>/`).
What Gradle does with them, done here with the SDK's own tools:

- every jar goes on `javac`'s class path for the app's sources, and is
  dexed with them (`d8`);
- each AAR's manifest is merged into the app's, after the app's own
  fragment: an entry the manifest already has is left out (the app's wins,
  as with Gradle's merger), relative class names are in the library's
  package, and `tools:` instructions are dropped;
- each AAR's `res/` is compiled with `aapt2 compile` and linked as an
  overlay under the app's (the app's resources win), and `aapt2 link
  --java --extra-packages` writes an `R` class for the app's package and
  each library's, compiled with the app's sources, so a library's
  `R.string.x` resolves at run time;
- an AAR's `assets/` and `jni/<abi>/*.so` go into the package too (the
  app's own assets win).

Libraries are files the developer has (downloaded from Maven, or built);
nothing is fetched here, and a library's own dependencies are listed like
any other library.
"""

import shutil
import xml.etree.ElementTree as ET
import zipfile
from dataclasses import dataclass, field
from pathlib import Path

from android_code import ANDROID_NS, A
from common import PackageError, log, warn



@dataclass
class Library:
    """One library, unpacked."""

    name: str
    jars: list = field(default_factory=list)
    package: str = None
    manifest: Path = None
    res: Path = None
    assets: Path = None
    jni: Path = None


def expand(paths):
    """The library files: each `.aar` or `.jar` given, and those directly in
    a directory given (sorted)."""
    out = []
    for p in paths:
        p = Path(p)
        if p.is_dir():
            found = sorted(x for x in p.iterdir() if x.suffix in (".aar", ".jar") and x.is_file())
            if not found:
                warn(f"android.libraries: no .aar or .jar in {p}")
            out += found
        elif p.suffix in (".aar", ".jar"):
            out.append(p)
        else:
            raise PackageError(f"android.libraries: {p} isn't an .aar, a .jar or a directory")
    seen = set()
    for p in out:
        if p.name in seen:
            raise PackageError(f"android.libraries: two libraries named {p.name}")
        seen.add(p.name)
    return out


def unpack(paths, out):
    """Each library unpacked under `out`: a list of Library."""
    out = Path(out)
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)
    libs = []
    for path in expand(paths):
        if path.suffix == ".jar":
            _check_zip(path)
            libs.append(Library(name=path.stem, jars=[path]))
            continue
        lib = Library(name=path.stem)
        dest = out / path.stem
        try:
            with zipfile.ZipFile(path) as z:
                for info in z.infolist():
                    name = info.filename
                    if info.is_dir() or name.startswith("/") or ".." in name.split("/"):
                        continue
                    if name == "classes.jar" or (name.startswith("libs/") and name.endswith(".jar")) \
                            or name == "AndroidManifest.xml" or name.startswith(("res/", "assets/", "jni/")):
                        target = dest / name
                        target.parent.mkdir(parents=True, exist_ok=True)
                        target.write_bytes(z.read(info))
        except zipfile.BadZipFile:
            raise PackageError(f"android.libraries: {path} isn't an AAR (not a zip)")
        if (dest / "classes.jar").is_file():
            lib.jars.append(dest / "classes.jar")
        lib.jars += sorted((dest / "libs").glob("*.jar")) if (dest / "libs").is_dir() else []
        manifest = dest / "AndroidManifest.xml"
        if not manifest.is_file():
            raise PackageError(f"android.libraries: {path} has no AndroidManifest.xml (not an AAR)")
        lib.manifest = manifest
        lib.package = library_package(manifest.read_text(encoding="utf-8"), path)
        if (dest / "res").is_dir() and any(p.is_file() for p in (dest / "res").rglob("*")):
            lib.res = dest / "res"
        if (dest / "assets").is_dir():
            lib.assets = dest / "assets"
        if (dest / "jni").is_dir():
            lib.jni = dest / "jni"
        libs.append(lib)
    if libs:
        log(
            f"{len(libs)} librar{'y' if len(libs) == 1 else 'ies'}: "
            + ", ".join(lib.name + (f" ({lib.package})" if lib.package else "") for lib in libs)
        )
    return libs


def _check_zip(path):
    if not zipfile.is_zipfile(path):
        raise PackageError(f"android.libraries: {path} isn't a jar (not a zip)")


def library_package(text, path="the library"):
    """The package an AAR's manifest names (where its R class goes)."""
    try:
        root = ET.fromstring(text)
    except ET.ParseError as e:
        raise PackageError(f"{path}: its AndroidManifest.xml: {e}")
    package = root.get("package")
    if not package:
        raise PackageError(f"{path}: its AndroidManifest.xml names no package")
    return package


def _qualify(name, package):
    if name.startswith("."):
        return package + name
    if "." not in name:
        return f"{package}.{name}"
    return name


def _key(el):
    return (el.tag, el.get(A + "name") or el.get(A + "glEsVersion"))


def _authorities(el):
    return [a.strip() for a in (el.get(A + "authorities") or "").split(";") if a.strip()]


def merge_library(root, text, lib_package, app_package, name="the library"):
    """Merge an AAR's manifest (`text`) into the manifest tree `root`, as
    Gradle's merger does with a library's: what the manifest already has
    wins. A component class the manifest declares already gets the
    library's `<meta-data>` it lacks (several libraries add theirs to
    androidx.startup's InitializationProvider); other entries already there
    (a permission, a `<meta-data>`, a provider authority) are left out.
    Relative class names are in the library's package, `${applicationId}` is
    the app's, `tools:` attributes are dropped, as are the library's
    <uses-sdk> and <application> attributes. Returns what was left out that
    the app may miss."""
    from android_code import COMPONENTS, TOP_LEVEL

    try:
        lib = ET.fromstring(text.replace("${applicationId}", app_package))
    except ET.ParseError as e:
        raise PackageError(f"{name}: its AndroidManifest.xml: {e}")
    for e in lib.iter():
        for k in [k for k in e.attrib if k.startswith("{") and not k.startswith(A)]:
            del e.attrib[k]
    app = root.find("application")
    have_top = {_key(el) for el in root if el.tag not in ("application", "uses-sdk", "queries")}
    classes, authorities, have_app = {}, set(), set()
    for el in app:
        if el.tag in COMPONENTS:
            if el.get(A + "name"):
                classes[_qualify(el.get(A + "name"), app_package)] = el
            authorities.update(_authorities(el))
        else:
            have_app.add(_key(el))
    dropped = []
    at = list(root).index(app)
    for el in lib:
        if not isinstance(el.tag, str) or el.tag == "uses-sdk":
            continue
        if el.tag == "application":
            dropped += [f"<application {k.replace(A, 'android:')}>" for k in el.attrib]
            for child in el:
                if not isinstance(child.tag, str):
                    continue
                if child.tag not in COMPONENTS:
                    if _key(child) not in have_app:
                        have_app.add(_key(child))
                        app.append(child)
                    continue
                cls = _qualify(child.get(A + "name") or "", lib_package)
                child.set(A + "name", cls)
                target = child.get(A + "targetActivity")
                if target:
                    child.set(A + "targetActivity", _qualify(target, lib_package))
                if cls in classes:
                    mine = classes[cls]
                    have = {_key(m) for m in mine if m.tag == "meta-data"}
                    for m in child:
                        if m.tag == "meta-data" and _key(m) not in have:
                            mine.append(m)
                    continue
                auth = _authorities(child)
                if any(a in authorities for a in auth):
                    dropped.append(f"<{child.tag} {cls}> (its authority is taken)")
                    continue
                classes[cls] = child
                authorities.update(auth)
                app.append(child)
        elif el.tag in TOP_LEVEL:
            if el.tag != "queries":
                if _key(el) in have_top:
                    continue
                have_top.add(_key(el))
            root.insert(at, el)
            at += 1
        else:
            dropped.append(f"<{el.tag}>")
    return dropped


def merge_manifests(manifest_path, libs, app_package):
    """Merge each AAR's manifest into the manifest file, after the app's own
    entries (its fragment's too), which win over a library's."""
    libs = [lib for lib in libs if lib.manifest]
    if not libs:
        return
    ET.register_namespace("android", ANDROID_NS)
    root = ET.fromstring(Path(manifest_path).read_text(encoding="utf-8"))
    if root.find("application") is None:
        raise PackageError("the manifest has no <application> to merge the libraries into")
    for lib in libs:
        dropped = merge_library(
            root, lib.manifest.read_text(encoding="utf-8"), lib.package, app_package, lib.name
        )
        if dropped:
            warn(f"library {lib.name}: left out of the manifest: {', '.join(dropped)}")
    ET.indent(root, space="  ")
    Path(manifest_path).write_text(
        '<?xml version="1.0" encoding="utf-8"?>\n' + ET.tostring(root, encoding="unicode") + "\n",
        encoding="utf-8",
    )
    log(f"merged the manifests of {len(libs)} librar{'y' if len(libs) == 1 else 'ies'}")


def jars(libs):
    """Every library jar, in order."""
    return [j for lib in libs for j in lib.jars]


def res_packages(libs):
    """The packages of the AARs with resources: each gets an R class."""
    out = []
    for lib in libs:
        if lib.package and lib.package not in out:
            out.append(lib.package)
    return out


def assets(libs):
    """(path, name) of every AAR's assets."""
    from common import walk

    out = []
    for lib in libs:
        if lib.assets:
            out += walk(lib.assets)
    return out


def native_libs(libs, abis):
    """(abi, path) of the AARs' native libraries for the ABIs built."""
    out = []
    for lib in libs:
        if not lib.jni:
            continue
        for abi_dir in sorted(lib.jni.iterdir()):
            if abi_dir.name in abis:
                out += [(abi_dir.name, so) for so in sorted(abi_dir.glob("*.so"))]
    return out
