"""Linux: the app installed into a staging root (a DESTDIR tree under the
prefix, `/usr` by default) and a tarball of it, for a distribution package
or a plain `cp -a` onto the system:

    <prefix>/bin/<exe>
    <prefix>/share/applications/<id>.desktop
    <prefix>/share/dbus-1/services/<id>.service     (linux.dbus-activatable)
    <prefix>/share/icons/hicolor/<n>x<n>/apps/<id>.png (and scalable/, for an SVG)

With `linux.dbus-activatable = true` the `.desktop` file says
`DBusActivatable=true` and the session bus `.service` file starts the app
for a desktop's `org.freedesktop.Application` call (the app turns on
`App::dbus_activatable`); see docs/os-integration.md.
"""

import os
import shutil
import tarfile
from pathlib import Path

from common import PackageError, cargo_build, log, output, warn

# The icon sizes installed, as the hicolor theme names them.
ICON_SIZES = (16, 24, 32, 48, 64, 128, 256, 512)

# GApplication's switch for a process the bus starts (`--gapplication-service`),
# which Stipple's single instance knows and Launch::current() drops.
SERVICE_SWITCH = "--gapplication-service"


def host_triple():
    for line in output(["rustc", "-vV"]).splitlines():
        if line.startswith("host: "):
            return line.split(None, 1)[1].strip()
    raise PackageError("rustc -vV gave no host")


def exec_quote(arg):
    """An `Exec` argument, quoted as the Desktop Entry spec asks when it has
    reserved characters (and `%` doubled)."""
    arg = arg.replace("%", "%%")
    if not any(c in arg for c in " \t\n\"'\\><~|&;$*?#()`"):
        return arg
    escaped = "".join("\\" + c if c in '"`$\\' else c for c in arg)
    return f'"{escaped}"'


def desktop_value(s):
    """A `.desktop` string value: newlines, tabs and backslashes escaped."""
    return s.replace("\\", "\\\\").replace("\n", "\\n").replace("\t", "\\t").replace("\r", "\\r")


def desktop_entry(m, exe_path):
    """The `.desktop` file's text."""
    exe = exec_quote(exe_path)
    lines = [
        "[Desktop Entry]",
        "Type=Application",
        "Version=1.5",
        f"Name={desktop_value(m.name)}",
    ]
    for lang in m.languages:
        name = m.name_in(lang)
        if name:
            lines.append(f"Name[{lang.replace('-', '_')}]={desktop_value(name)}")
    if m.linux_comment:
        lines.append(f"Comment={desktop_value(m.linux_comment)}")
    # %U: the files and URLs the desktop opens with it (a scheme handler's
    # URL, files dropped on it), each an argument.
    lines += [
        f"Exec={exe} %U",
        f"Icon={m.id}",
        "Terminal=false",
        f"Categories={''.join(c + ';' for c in m.linux_categories)}",
        "StartupNotify=true",
        f"StartupWMClass={m.id}",
    ]
    mime = [f"x-scheme-handler/{s}" for s in m.url_schemes] + m.linux_mime_types
    if mime:
        lines.append(f"MimeType={''.join(t + ';' for t in mime)}")
    if m.linux_dbus_activatable:
        lines.append("DBusActivatable=true")
    if m.linux_actions:
        lines.append(f"Actions={''.join(a['id'] + ';' for a in m.linux_actions)}")
    for a in m.linux_actions:
        # A desktop that activates over D-Bus calls ActivateAction(id); the
        # others run Exec, which just starts (or raises) the app.
        lines += ["", f"[Desktop Action {a['id']}]", f"Name={desktop_value(a['name'])}", f"Exec={exe}"]
    return "\n".join(lines) + "\n"


def dbus_service(m, exe_path):
    """The session bus `.service` file's text."""
    return (
        "[D-BUS Service]\n"
        f"Name={m.id}\n"
        f"Exec={exec_quote(exe_path)} {SERVICE_SWITCH}\n"
    )


def stage(m, root, exe, exe_name, helper=None):
    """Lay the app out under `root` (the DESTDIR) at the manifest's prefix.
    Returns the files written, relative to `root`."""
    prefix = root / m.linux_prefix.lstrip("/")
    written = []

    def put(rel, data=None, src=None, mode=0o644):
        p = prefix / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        if src is not None:
            shutil.copyfile(src, p)
        else:
            p.write_text(data)
        os.chmod(p, mode)
        written.append(p.relative_to(root).as_posix())

    exe_path = f"{m.linux_prefix.rstrip('/')}/bin/{exe_name}"
    put(f"bin/{exe_name}", src=exe, mode=0o755)
    put(f"share/applications/{m.id}.desktop", desktop_entry(m, exe_path))
    if m.linux_dbus_activatable:
        put(f"share/dbus-1/services/{m.id}.service", dbus_service(m, exe_path))
    if m.icon:
        icons = prefix / "share" / "icons" / "hicolor"
        if m.icon.suffix.lower() == ".svg":
            put(f"share/icons/hicolor/scalable/apps/{m.id}.svg", src=m.icon)
        if helper is not None:
            jobs = [(icons / f"{n}x{n}" / "apps" / f"{m.id}.png", n, 1.0, None, m.icon) for n in ICON_SIZES]
            helper.icons(jobs)
            for out, *_ in jobs:
                written.append(Path(out).relative_to(root).as_posix())
    else:
        warn("no icon in the manifest: the desktop shows a generic one")
    if m.assets:
        warn("linux: assets aren't installed (the app finds them beside its executable)")
    return written


def package(m, args, target, target_dir, work, helper):
    triple = getattr(args, "linux_target", None) or host_triple()
    if "linux" not in triple:
        raise PackageError(f"{triple} isn't a Linux target")
    log(f"building {target.name} for {triple}")
    cargo_build(args.manifest_path, target, triple, args.profile, args.features)
    exe = target.artifact(target_dir, triple, args.profile, cdylib=False)
    if not exe.exists():
        raise PackageError(f"no {exe}: is the {target.kind} a binary?")
    if getattr(args, "prefix", None):
        if not args.prefix.startswith("/"):
            raise PackageError("--prefix: an absolute path")
        m.linux_prefix = args.prefix
    out_dir = Path(args.out or Path(target_dir) / "stipple-package" / "linux")
    root = out_dir / "root"
    if root.exists():
        shutil.rmtree(root)
    root.mkdir(parents=True)
    files = stage(m, root, exe, target.name, helper)
    arch = triple.split("-")[0]
    tarball = out_dir / f"{m.id}-{m.version}-{arch}.tar.gz"
    with tarfile.open(tarball, "w:gz") as t:
        for rel in sorted(files):
            info = t.gettarinfo(root / rel, rel)
            info.uid = info.gid = 0
            info.uname = info.gname = "root"
            with open(root / rel, "rb") as f:
                t.addfile(info, f)
    for rel in files:
        log(f"  {rel}")
    log(f"wrote {root} and {tarball}")
