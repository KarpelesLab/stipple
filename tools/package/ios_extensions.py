"""iOS app extensions and app groups: the manifest's
`[[package.metadata.stipple.ios.extension]]` entries and `ios.app-groups`,
and what they become: `PlugIns/<Name>.appex` bundles (an executable, an
Info.plist with `CFBundlePackageType` `XPC!` and the `NSExtension`
dictionary, the privacy manifest, a provisioning profile) and the
`com.apple.security.application-groups` entitlement.

`ios.py` builds and signs them (each extension before the app, which is
signed last); the pure parts are here, tested by test_package.py.
"""

import re

from manifest import IOS_ID, ManifestError

APP_GROUP = re.compile(r"^group\.[A-Za-z0-9-]+(\.[A-Za-z0-9-]+)+$")
EXT_NAME = re.compile(r"^[A-Za-z][A-Za-z0-9_-]*$")
ENTRIES = ("main", "NSExtensionMain")

APP_GROUPS_KEY = "com.apple.security.application-groups"


def app_groups(value, where="ios.app-groups"):
    """`ios.app-groups`: a list of `group.<reverse-dns>` identifiers."""
    if not isinstance(value, list) or not all(isinstance(g, str) for g in value):
        raise ManifestError(f"{where}: a list of group identifiers (\"group.com.example.app\")")
    for g in value:
        if not APP_GROUP.match(g):
            raise ManifestError(f"{where}: {g!r} isn't an app group (group.com.example.app)")
    if len(set(value)) != len(value):
        raise ManifestError(f"{where}: a group is listed twice")
    return list(value)


def profile_env(name):
    """The variable holding an extension's provisioning profile:
    `STIPPLE_IOS_PROVISIONING_PROFILE_<NAME>`, the name in capitals with
    anything else made `_` (`Tunnel` -> `..._TUNNEL`, `share-ext` ->
    `..._SHARE_EXT`)."""
    return "STIPPLE_IOS_PROVISIONING_PROFILE_" + re.sub(r"[^A-Z0-9]", "_", name.upper())


class Extension:
    """One `[[ios.extension]]`, checked. `m` is the Manifest being read (for
    its paths, bundle id and app groups)."""

    def __init__(self, m, d, index):
        where = f"ios.extension[{index}]"
        d = dict(d)
        self.name = d.pop("name", None)
        if not isinstance(self.name, str) or not EXT_NAME.match(self.name):
            raise ManifestError(
                f"{where}.name: required, a bundle name (letters, digits, - and _: \"Tunnel\")"
            )
        where = f"ios.extension.{self.name}"
        bins = [k for k in ("bin", "example") if k in d]
        if len(bins) != 1:
            raise ManifestError(f"{where}: give the Cargo target as bin = \"...\" or example = \"...\"")
        self.kind = bins[0]
        self.target = str(d.pop(self.kind))
        suffix = d.pop("bundle-id-suffix", None)
        full = d.pop("bundle-id", None)
        if suffix is not None and full is not None:
            raise ManifestError(f"{where}: bundle-id or bundle-id-suffix, not both")
        if full is None:
            suffix = suffix or re.sub(r"[^A-Za-z0-9-]", "-", self.name.lower())
            full = f"{m.ios_bundle_id}.{suffix}"
        self.bundle_id = str(full)
        check_nesting(m.ios_bundle_id, self.bundle_id, where)
        self.point = d.pop("point", None)
        if not isinstance(self.point, str) or not self.point:
            raise ManifestError(
                f"{where}.point: required, the NSExtensionPointIdentifier "
                "(com.apple.networkextension.packet-tunnel, com.apple.share-services...)"
            )
        self.principal_class = d.pop("principal-class", None)
        self.attributes = dict(d.pop("attributes", {}))
        self.extension_info = dict(d.pop("extension", {}))
        self.info = dict(d.pop("info", {}))
        self.display_name = str(d.pop("display-name", self.name))
        self.minimum_os = str(d.pop("minimum-os", m.ios_minimum))
        self.entry = d.pop("entry", "main")
        if self.entry not in ENTRIES:
            raise ManifestError(f"{where}.entry: {self.entry!r} isn't one of {', '.join(ENTRIES)}")
        self.entitlements = m._path(d.pop("entitlements", None), f"{where}.entitlements")
        groups = d.pop("app-groups", False)
        if groups is True:
            self.app_groups = list(m.ios_app_groups)
        elif groups is False:
            self.app_groups = []
        else:
            self.app_groups = app_groups(groups, f"{where}.app-groups")
            missing = [g for g in self.app_groups if g not in m.ios_app_groups]
            if missing:
                raise ManifestError(
                    f"{where}.app-groups: {', '.join(missing)} not in ios.app-groups"
                )
        if groups is True and not self.app_groups:
            raise ManifestError(f"{where}.app-groups = true, but ios.app-groups is empty")
        if d:
            raise ManifestError(f"unknown keys in {where}: {', '.join(sorted(d))}")

    @property
    def profile_env(self):
        return profile_env(self.name)

    @property
    def bundle_dir(self):
        return f"{self.name}.appex"

    def __repr__(self):
        return (
            f"Extension({self.name!r}, {self.kind}={self.target!r}, id={self.bundle_id!r}, "
            f"point={self.point!r}, principal-class={self.principal_class!r}, "
            f"app-groups={self.app_groups!r}, entry={self.entry!r})"
        )


def check_nesting(app_id, ext_id, where):
    """An extension's bundle id is the app's followed by `.` and more."""
    if not IOS_ID.match(ext_id) or ext_id.endswith(".") or ".." in ext_id:
        raise ManifestError(f"{where}.bundle-id: {ext_id!r}")
    if not ext_id.startswith(app_id + "."):
        raise ManifestError(
            f"{where}.bundle-id: {ext_id!r} must start with the app's {app_id!r} and a dot "
            f"({app_id}.tunnel)"
        )


def extensions(m, entries):
    """The checked `[[ios.extension]]` list: unique names, bundle ids and
    profile variables."""
    if not isinstance(entries, list):
        raise ManifestError("ios.extension: use [[package.metadata.stipple.ios.extension]] tables")
    out = [Extension(m, d, i) for i, d in enumerate(entries)]
    for attr, what in (("name", "name"), ("bundle_id", "bundle id"), ("profile_env", "profile variable")):
        seen = {}
        for e in out:
            v = getattr(e, attr)
            if v in seen:
                raise ManifestError(
                    f"ios.extension: {seen[v]} and {e.name} have the same {what} ({v})"
                )
            seen[v] = e.name
    return out


# --- Info.plist and entitlements ---------------------------------------------


def info_plist(m, ext, exe_name, simulator, toolchain):
    """The extension's Info.plist. The version and build are the app's (App
    Store Connect rejects an extension whose differ)."""
    from ios import _short_version

    ns = {"NSExtensionPointIdentifier": ext.point}
    if ext.principal_class:
        ns["NSExtensionPrincipalClass"] = ext.principal_class
    if ext.attributes:
        ns["NSExtensionAttributes"] = ext.attributes
    ns.update(ext.extension_info)
    info = {
        "CFBundleDevelopmentRegion": m.languages[0] if m.languages else "en",
        "CFBundleDisplayName": ext.display_name,
        "CFBundleExecutable": exe_name,
        "CFBundleIdentifier": ext.bundle_id,
        "CFBundleInfoDictionaryVersion": "6.0",
        "CFBundleName": ext.name[:15],
        "CFBundlePackageType": "XPC!",
        "CFBundleShortVersionString": _short_version(m.version),
        "CFBundleVersion": str(m.build),
        "CFBundleSupportedPlatforms": ["iPhoneSimulator" if simulator else "iPhoneOS"],
        "MinimumOSVersion": ext.minimum_os,
        "UIDeviceFamily": sorted(set(m.ios_family)),
        "NSExtension": ns,
    }
    info.update(toolchain)
    info.update(ext.info)
    return info


def group_entitlements(groups):
    """The manifest's part of a bundle's entitlements: the app groups (and,
    for whoever adds more manifest-driven entitlements, the place to)."""
    return {APP_GROUPS_KEY: list(groups)} if groups else {}


def check_profile_groups(groups, profile_ent, who):
    """The groups the bundle asks for must be in its provisioning profile
    (which lists the ones its app id has, or a wildcard)."""
    granted = profile_ent.get(APP_GROUPS_KEY)
    if granted is None or not groups:
        return []
    granted = list(granted)
    if any(g.endswith("*") for g in granted):
        return []
    return [f"{who}: app group {g} isn't in its provisioning profile" for g in groups if g not in granted]


# --- Building ------------------------------------------------------------------

# The linker arguments for `entry = "NSExtensionMain"`: Foundation's
# NSExtensionMain as the entry point, as Xcode links extensions (Rust's own
# `main` then never runs).
NSEXTENSIONMAIN_LINK_ARGS = [
    "-C", "link-arg=-e", "-C", "link-arg=_NSExtensionMain",
    "-C", "link-arg=-framework", "-C", "link-arg=Foundation",
]


def cargo_command(args, target, triple, ext):
    """`cargo build` for an extension whose Rust `main` calls
    `platform::extension_main()`; `cargo rustc` with the linker arguments
    for one linked to start in NSExtensionMain."""
    sub = "build" if ext.entry == "main" else "rustc"
    cmd = ["cargo", sub, "--manifest-path", args.manifest_path, "--target", triple]
    cmd += ["--release"] if args.profile == "release" else ["--profile", args.profile]
    cmd += target.cargo_args()
    if args.features:
        cmd += ["--features", args.features]
    if ext.entry == "NSExtensionMain":
        cmd += ["--"] + NSEXTENSIONMAIN_LINK_ARGS
    return cmd


def package_all(m, args, package, triple, target_dir, app, out_dir, work, simulator, env):
    """Build each extension, write `PlugIns/<Name>.appex` into `app`, and
    sign it (before the app is)."""
    import os
    import plistlib
    import shutil

    import ios
    from common import PackageError, Target, log, run, warn

    if not m.ios_extensions:
        return []
    plugins = app / "PlugIns"
    plugins.mkdir(exist_ok=True)
    toolchain = ios.toolchain_keys(simulator)
    privacy = ios.privacy_manifest(m)
    done = []
    for ext in m.ios_extensions:
        target = Target(package, ext.kind, ext.target)
        log(f"building the {ext.name} extension ({target.kind} {target.name}) for {triple}")
        ext_env = {**os.environ, **env}
        if not os.environ.get("IPHONEOS_DEPLOYMENT_TARGET"):
            ext_env["IPHONEOS_DEPLOYMENT_TARGET"] = ext.minimum_os
        run(cargo_command(args, target, triple, ext), env=ext_env)
        exe = target.artifact(target_dir, triple, args.profile, cdylib=False)
        if not exe.exists():
            raise PackageError(f"no {exe}: is the {target.kind} a binary?")
        appex = plugins / ext.bundle_dir
        if appex.exists():
            shutil.rmtree(appex)
        appex.mkdir()
        shutil.copy2(exe, appex / target.name)
        os.chmod(appex / target.name, 0o755)
        with open(appex / "Info.plist", "wb") as f:
            plistlib.dump(info_plist(m, ext, target.name, simulator, toolchain), f)
        run(["plutil", "-lint", appex / "Info.plist"], quiet=True)
        with open(appex / "PrivacyInfo.xcprivacy", "wb") as f:
            plistlib.dump(privacy, f)
        if not simulator:
            dsym = out_dir / f"{ext.bundle_dir}.dSYM"
            if dsym.exists():
                shutil.rmtree(dsym)
            try:
                run(["xcrun", "dsymutil", exe, "-o", dsym], quiet=True)
            except PackageError:
                warn(f"dsymutil failed: no dSYM for {ext.name}")
        ios.sign_bundle(
            appex, ext.bundle_id, ext.entitlements, ext.app_groups, ext.profile_env,
            work / f"entitlements-{ext.name}.plist", simulator, f"the {ext.name} extension",
        )
        log(f"wrote {appex}")
        done.append(appex)
    return done
