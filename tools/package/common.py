"""What both platforms share: running commands, building the app with Cargo,
the host helper (icons, dex), and zip files with the right modes."""

import json
import os
import shlex
import shutil
import subprocess
import sys
import zipfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
VERBOSE = False


class PackageError(Exception):
    pass


def log(msg):
    print(f"cargo-stipple: {msg}", file=sys.stderr)


def warn(msg):
    print(f"cargo-stipple: warning: {msg}", file=sys.stderr)


def run(cmd, quiet=False, **kw):
    cmd = [str(c) for c in cmd]
    if VERBOSE:
        log("$ " + shlex.join(cmd))
    if quiet and not VERBOSE:
        r = subprocess.run(cmd, capture_output=True, text=True, **kw)
        if r.returncode != 0:
            sys.stderr.write(r.stdout + r.stderr)
            raise PackageError(f"{Path(cmd[0]).name} failed (exit {r.returncode})")
        return r
    r = subprocess.run(cmd, **kw)
    if r.returncode != 0:
        raise PackageError(f"{Path(cmd[0]).name} failed (exit {r.returncode})")
    return r


def output(cmd, **kw):
    return run(cmd, quiet=True, **kw).stdout


# --- Cargo -----------------------------------------------------------------


class Target:
    """What to build: a bin, the lib, or an example of the package."""

    def __init__(self, package, kind, name=None):
        self.package = package
        self.kind = kind  # "bin", "lib", "example"
        targets = package["targets"]
        if kind == "example":
            match = [t for t in targets if t["name"] == name and "example" in t["kind"]]
            if not match:
                raise PackageError(f"no example {name!r} in {package['name']}")
        elif kind == "lib":
            match = [t for t in targets if set(t["kind"]) & {"cdylib", "lib", "staticlib"}]
            if not match:
                raise PackageError(f"{package['name']} has no [lib]")
        else:
            match = [t for t in targets if "bin" in t["kind"] and (name is None or t["name"] == name)]
            if not match:
                raise PackageError(f"no bin {name or ''} in {package['name']}".replace("  ", " "))
            if len(match) > 1:
                names = ", ".join(t["name"] for t in match)
                raise PackageError(f"several bins ({names}): pick one with --bin")
        self.target = match[0]
        self.name = self.target["name"]

    def cargo_args(self):
        if self.kind == "example":
            return ["--example", self.name]
        if self.kind == "lib":
            return ["--lib"]
        return ["--bin", self.name]

    def artifact(self, target_dir, triple, profile, cdylib):
        """The built file: `lib<name>.so` for a cdylib, else the executable."""
        d = Path(target_dir) / triple / profile
        if self.kind == "example":
            d = d / "examples"
        if cdylib:
            return d / f"lib{self.name.replace('-', '_')}.so"
        return d / self.name


def cargo_build(manifest_path, target, triple, profile, features, env=None):
    cmd = ["cargo", "build", "--manifest-path", manifest_path, "--target", triple]
    cmd += ["--release"] if profile == "release" else ["--profile", profile]
    cmd += target.cargo_args()
    if features:
        cmd += ["--features", features]
    run(cmd, env={**os.environ, **(env or {})})


# --- The host helper -------------------------------------------------------


def stipple_package(manifest_path):
    """The app's Stipple from `cargo metadata` (its version, its source, its
    manifest_path in Cargo's download), or None when the app has none."""
    from manifest import cargo_metadata

    key = str(Path(manifest_path).resolve())
    if key not in _STIPPLE:
        meta = cargo_metadata(manifest_path, deps=True)
        _STIPPLE[key] = next((p for p in meta["packages"] if p["name"] == "stipple"), None)
    return _STIPPLE[key]


_STIPPLE = {}


def installed_helper(manifest_path):
    """The installed `cargo-stipple` (src/bin/cargo-stipple.rs) as the helper
    command, when it is what ran this script and it was built from the very
    release the app depends on (from crates.io): it carries helper.rs, so
    nothing needs building. None otherwise."""
    exe = os.environ.get("STIPPLE_PACKAGE_HELPER")
    version = os.environ.get("STIPPLE_PACKAGE_VERSION")
    if not exe or not version:
        return None
    p = stipple_package(manifest_path)
    if p and p["version"] == version and (p.get("source") or "").startswith("registry+"):
        return [exe, "__stipple-helper"]
    return None


class Helper:
    """The host helper: the installed `cargo-stipple` when it matches the
    app's Stipple (installed_helper), else the scratch crate around
    helper.rs, built once per run against the app's own Stipple (its source
    from `cargo metadata`, its versions from the app's Cargo.lock)."""

    def __init__(self, manifest_path, work, target_dir):
        self.manifest_path = Path(manifest_path)
        self.work = Path(work) / "helper"
        self.target_dir = Path(target_dir) / "stipple-package-helper"
        self.cmd = None

    def _stipple_dir(self):
        p = stipple_package(self.manifest_path)
        if p is None:
            raise PackageError("the app doesn't depend on stipple")
        return Path(p["manifest_path"]).parent

    def build(self):
        """The helper's command (a list), built the first time."""
        if self.cmd:
            return self.cmd
        installed = installed_helper(self.manifest_path)
        if installed:
            self.cmd = installed
            return self.cmd
        self.work.mkdir(parents=True, exist_ok=True)
        (self.work / "src").mkdir(exist_ok=True)
        stipple = self._stipple_dir()
        (self.work / "Cargo.toml").write_text(
            "[package]\n"
            'name = "stipple-package-helper"\n'
            'version = "0.0.0"\n'
            'edition = "2024"\n'
            "publish = false\n\n"
            "[dependencies]\n"
            f"stipple = {{ path = {json.dumps(str(stipple))} }}\n\n"
            "# Not part of any workspace around it.\n"
            "[workspace]\n"
        )
        shutil.copy(HERE / "helper.rs", self.work / "src" / "main.rs")
        # The app's resolved versions, so the helper builds what the app does.
        lock = _find_lock(self.manifest_path.parent)
        if lock and not (self.work / "Cargo.lock").exists():
            shutil.copy(lock, self.work / "Cargo.lock")
        log("building the host helper (icons, dex)")
        run(
            ["cargo", "build", "--release", "--quiet", "--manifest-path", self.work / "Cargo.toml"],
            env={**os.environ, "CARGO_TARGET_DIR": str(self.target_dir)},
        )
        exe = self.target_dir / "release" / "stipple-package-helper"
        if os.name == "nt":
            exe = exe.with_suffix(".exe")
        self.cmd = [exe]
        return self.cmd

    def dex(self, lib_name, out):
        run(self.build() + ["dex", lib_name, out])

    def icons(self, jobs):
        """`jobs`: (out, size, scale, background (r, g, b, a) or None, source)."""
        if not jobs:
            return
        cmd = self.build()
        lines = []
        for out, size, scale, bg, src in jobs:
            Path(out).parent.mkdir(parents=True, exist_ok=True)
            bgs = "-" if bg is None else "".join(f"{c:02x}" for c in bg)
            lines.append(f"{out}\t{size}\t{scale:.6f}\t{bgs}\t{src}")
        self.work.mkdir(parents=True, exist_ok=True)
        f = self.work / "icons.tsv"
        f.write_text("\n".join(lines) + "\n")
        run(cmd + ["icons", f])


def _find_lock(start):
    d = Path(start).resolve()
    while True:
        if (d / "Cargo.lock").exists():
            return d / "Cargo.lock"
        if d.parent == d:
            return None
        d = d.parent


# --- Files -----------------------------------------------------------------

# Already-compressed media, stored as they are: reading them needs no
# inflating, and Assets::open_fd hands them out as a range of the APK.
STORED = {
    ".png", ".jpg", ".jpeg", ".webp", ".gif", ".ogg", ".mp3", ".opus", ".flac", ".aac", ".wav",
    ".mp4", ".m4a", ".m4v", ".webm", ".mkv", ".3gp",
}


def walk(root):
    """The files under `root`, as (path, `/`-separated relative name), sorted,
    skipping dotfiles."""
    root = Path(root)
    out = []
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = sorted(d for d in dirnames if not d.startswith("."))
        for f in sorted(filenames):
            if f.startswith("."):
                continue
            p = Path(dirpath) / f
            out.append((p, p.relative_to(root).as_posix()))
    return out


def zip_add(z, name, data=None, path=None, stored=False, mode=0o644):
    info = zipfile.ZipInfo(name, date_time=(2000, 1, 1, 0, 0, 0))
    info.compress_type = zipfile.ZIP_STORED if stored else zipfile.ZIP_DEFLATED
    info.external_attr = (0o100000 | mode) << 16
    info.create_system = 3
    if path is not None:
        data = Path(path).read_bytes()
    z.writestr(info, data)


def copy_tree(src, dst):
    if dst.exists():
        shutil.rmtree(dst)
    shutil.copytree(src, dst, ignore=shutil.ignore_patterns(".*"))
