"""iOS: an `.app` bundle (device or simulator), signed, and for a device an
`.ipa` to upload to App Store Connect, with no Xcode project.

The bundle holds the binary, an Info.plist written from the manifest, the
app icon (an asset catalog compiled by Xcode's actool when Xcode is there,
else loose PNGs), the localized display name and permission prompts
(`<lang>.lproj/InfoPlist.strings`), the privacy manifest, the app's assets,
and, when signing for a device, the provisioning profile. codesign signs it
with the entitlements the profile grants.
"""

import os
import plistlib
import shlex
import shutil
import subprocess
import sys
import zipfile
from pathlib import Path

from common import PackageError, cargo_build, copy_tree, log, run, warn, zip_add
from manifest import ios_lproj

ORIENTATION_KEYS = {
    "portrait": "UIInterfaceOrientationPortrait",
    "portrait-upside-down": "UIInterfaceOrientationPortraitUpsideDown",
    "landscape-left": "UIInterfaceOrientationLandscapeLeft",
    "landscape-right": "UIInterfaceOrientationLandscapeRight",
}
ALL_ORIENTATIONS = list(ORIENTATION_KEYS)

# Loose icons, for a bundle built without actool: the home screen's sizes.
LOOSE_ICONS = [
    ("AppIcon60x60@2x.png", 120, 1),
    ("AppIcon60x60@3x.png", 180, 1),
    ("AppIcon76x76@2x~ipad.png", 152, 2),
    ("AppIcon83.5x83.5@2x~ipad.png", 167, 2),
]

# The required-reason APIs Stipple itself reaches (through Rust's standard
# library): file sizes and timestamps (stat, for the bundle's assets and the
# app's own files) and the monotonic clock (Instant, for animations and
# timers).
STIPPLE_PRIVACY_APIS = {
    "NSPrivacyAccessedAPICategoryFileTimestamp": ["C617.1"],
    "NSPrivacyAccessedAPICategorySystemBootTime": ["35F9.1"],
}


def have_xcode():
    try:
        subprocess.run(["xcrun", "--find", "actool"], check=True, capture_output=True)
        return True
    except (OSError, subprocess.CalledProcessError):
        return False


def info_plist(m, exe_name, simulator):
    family = sorted(set(m.ios_family))
    orientations = [ORIENTATION_KEYS[o] for o in m.orientations]
    info = {
        "CFBundleDevelopmentRegion": m.languages[0] if m.languages else "en",
        "CFBundleDisplayName": m.name,
        "CFBundleExecutable": exe_name,
        "CFBundleIdentifier": m.ios_bundle_id,
        "CFBundleInfoDictionaryVersion": "6.0",
        "CFBundleName": m.name[:15],
        "CFBundlePackageType": "APPL",
        "CFBundleShortVersionString": _short_version(m.version),
        "CFBundleVersion": str(m.build),
        "CFBundleSupportedPlatforms": ["iPhoneSimulator" if simulator else "iPhoneOS"],
        "LSRequiresIPhoneOS": True,
        "MinimumOSVersion": m.ios_minimum,
        "UIDeviceFamily": family,
        "UILaunchScreen": {},
        # An iPad's trackpad and mouse clicks as indirect pointer touches,
        # whose button mask tells a secondary click (a context menu).
        "UIApplicationSupportsIndirectInputEvents": True,
        "UIApplicationSceneManifest": {
            "UIApplicationSupportsMultipleScenes": m.ios_multiple_scenes,
        },
        "UISupportedInterfaceOrientations": orientations,
        "UIRequiredDeviceCapabilities": ["arm64"],
        "ITSAppUsesNonExemptEncryption": m.ios_encryption,
    }
    if 2 in family:
        ipad = m.ios_ipad_orientations or ALL_ORIENTATIONS
        info["UISupportedInterfaceOrientations~ipad"] = [ORIENTATION_KEYS[o] for o in ipad]
        if set(ipad) != set(ALL_ORIENTATIONS):
            # iPad multitasking needs every orientation; without, full screen.
            info["UIRequiresFullScreen"] = True
    info.update(toolchain_keys(simulator))
    if m.languages:
        info["CFBundleLocalizations"] = list(m.languages)
    info.update(url_types(m))
    info.update(m.ios_usage)
    info.update(m.ios_info)
    return info


def url_types(m):
    """CFBundleURLTypes for the manifest's url-schemes: the schemes iOS opens
    the app for (application:openURL:options:, scene:openURLContexts:)."""
    if not m.url_schemes:
        return {}
    return {
        "CFBundleURLTypes": [
            {
                "CFBundleURLName": m.ios_bundle_id,
                "CFBundleTypeRole": "Viewer",
                "CFBundleURLSchemes": list(m.url_schemes),
            }
        ]
    }


def link_entitlements(m):
    """com.apple.developer.associated-domains for the manifest's
    associated-domains: `applinks:host` (iOS takes the paths from the
    domain's apple-app-site-association, not the entitlement)."""
    if not m.link_domains:
        return {}
    return {
        "com.apple.developer.associated-domains": [
            f"applinks:{d['host']}" + (f"?mode={d['mode']}" if d["mode"] else "")
            for d in m.link_domains
        ]
    }


def toolchain_keys(simulator):
    """The DT* keys Xcode writes, which App Store Connect checks (the SDK and
    Xcode the app was built with); empty without Xcode."""
    sdk = "iphonesimulator" if simulator else "iphoneos"

    def out(*cmd):
        try:
            return subprocess.run(cmd, check=True, capture_output=True, text=True).stdout.strip()
        except (OSError, subprocess.CalledProcessError):
            return ""

    version = out("xcrun", "--sdk", sdk, "--show-sdk-version")
    if not version:
        return {}
    xcode = out("xcodebuild", "-version").split()
    keys = {
        "DTPlatformName": sdk,
        "DTPlatformVersion": version,
        "DTPlatformBuild": out("xcrun", "--sdk", sdk, "--show-sdk-build-version"),
        "DTSDKName": f"{sdk}{version}",
        "DTSDKBuild": out("xcrun", "--sdk", sdk, "--show-sdk-build-version"),
        "DTCompiler": "com.apple.compilers.llvm.clang.1_0",
        "BuildMachineOSBuild": out("sw_vers", "-buildVersion"),
    }
    # "Xcode 16.2 Build version 16C5032a": DTXcode "1620", DTXcodeBuild.
    if len(xcode) >= 5 and xcode[0] == "Xcode":
        nums = (xcode[1].split(".") + ["0", "0"])[:3]
        keys["DTXcode"] = f"{int(nums[0]):02d}{nums[1][:1]}{nums[2][:1]}"
        keys["DTXcodeBuild"] = xcode[-1]
    return {k: v for k, v in keys.items() if v}


def _short_version(v):
    """CFBundleShortVersionString: up to three dot-separated integers."""
    core = v.split("-")[0].split("+")[0]
    parts = core.split(".")[:3]
    if not all(p.isdigit() for p in parts):
        raise PackageError(f"version {v!r}: iOS wants up to three numbers (1.2.3)")
    return ".".join(parts)


def privacy_manifest(m):
    if m.ios_privacy_manifest:
        with open(m.ios_privacy_manifest, "rb") as f:
            return plistlib.load(f)
    apis = {k: list(v) for k, v in STIPPLE_PRIVACY_APIS.items()}
    for cat, reasons in m.ios_privacy.items():
        apis.setdefault(cat, [])
        apis[cat] += [r for r in reasons if r not in apis[cat]]
    return {
        "NSPrivacyTracking": False,
        "NSPrivacyTrackingDomains": [],
        "NSPrivacyCollectedDataTypes": [],
        "NSPrivacyAccessedAPITypes": [
            {"NSPrivacyAccessedAPIType": cat, "NSPrivacyAccessedAPITypeReasons": reasons}
            for cat, reasons in apis.items()
        ],
    }


def _strings_file(entries):
    def q(s):
        return '"' + s.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n") + '"'

    return "".join(f"{q(k)} = {q(v)};\n" for k, v in entries.items())


def localize(m, app):
    """`<lang>.lproj/InfoPlist.strings`: the display name and permission
    prompts in each language the manifest translates."""
    from manifest import usage_keys

    for lang in m.languages:
        loc = m.localized.get(lang, {})
        entries = {}
        if loc.get("name"):
            entries["CFBundleDisplayName"] = loc["name"]
            entries["CFBundleName"] = loc["name"][:15]
        entries.update(usage_keys(loc.get("usage", {}), f"localized.{lang}.usage"))
        # The directory says the app is in that language (the Settings app
        # offers it), with or without strings.
        d = app / f"{ios_lproj(lang)}.lproj"
        d.mkdir(exist_ok=True)
        if entries:
            (d / "InfoPlist.strings").write_text(_strings_file(entries), encoding="utf-8")


def icons(m, app, work, helper, simulator, info):
    """The app icon: an asset catalog through actool, else loose PNGs."""
    if not m.icon:
        warn("no icon in the manifest: App Store Connect rejects a build without one")
        return
    # iOS icons are opaque: the image over the background colour.
    bg = m.icon_background[:3] + (255,)
    icon_1024 = work / "AppIcon-1024.png"
    if have_xcode():
        catalog = work / "Assets.xcassets"
        iconset = catalog / "AppIcon.appiconset"
        iconset.mkdir(parents=True, exist_ok=True)
        helper.icons([(iconset / "icon-1024.png", 1024, 1.0, bg, m.icon)])
        (catalog / "Contents.json").write_text('{"info":{"author":"xcode","version":1}}\n')
        (iconset / "Contents.json").write_text(
            '{"images":[{"filename":"icon-1024.png","idiom":"universal","platform":"ios",'
            '"size":"1024x1024"}],"info":{"author":"xcode","version":1}}\n'
        )
        partial = work / "actool-info.plist"
        cmd = [
            "xcrun", "actool", catalog, "--compile", app,
            "--platform", "iphonesimulator" if simulator else "iphoneos",
            "--minimum-deployment-target", m.ios_minimum,
            "--app-icon", "AppIcon", "--output-partial-info-plist", partial,
            "--output-format", "human-readable-text", "--notices", "--warnings",
        ]
        for fam in sorted(set(m.ios_family)):
            cmd += ["--target-device", "iphone" if fam == 1 else "ipad"]
        run(cmd, quiet=True)
        with open(partial, "rb") as f:
            info.update(plistlib.load(f))
        shutil.copy(iconset / "icon-1024.png", icon_1024)
    else:
        warn(
            "Xcode's actool isn't here: loose icon files instead of an asset catalog "
            "(enough for the simulator and ad-hoc installs; App Store Connect wants Assets.car)"
        )
        jobs = [(app / name, size, 1.0, bg, m.icon) for name, size, fam in LOOSE_ICONS
                if fam in m.ios_family]
        jobs.append((icon_1024, 1024, 1.0, bg, m.icon))
        helper.icons(jobs)
        if 1 in m.ios_family:
            info["CFBundleIcons"] = {"CFBundlePrimaryIcon": {"CFBundleIconFiles": ["AppIcon60x60"]}}
        if 2 in m.ios_family:
            info["CFBundleIcons~ipad"] = {
                "CFBundlePrimaryIcon": {
                    "CFBundleIconFiles": ["AppIcon60x60", "AppIcon76x76", "AppIcon83.5x83.5"],
                }
            }


# --- Signing -----------------------------------------------------------------


def profile_plist(path):
    out = subprocess.run(
        ["security", "cms", "-D", "-i", str(path)], capture_output=True, check=True
    ).stdout
    return plistlib.loads(out)


def entitlements(m, profile):
    """The entitlements to sign with: the profile's (its wildcard app id made
    this app's), then the manifest's file on top."""
    ent = {}
    if profile:
        ent = dict(profile.get("Entitlements", {}))
        team = (profile.get("TeamIdentifier") or [""])[0]
        app_id = f"{team}.{m.ios_bundle_id}"
        granted = ent.get("application-identifier", "")
        if granted.endswith(".*") and app_id.startswith(granted[:-1]):
            ent["application-identifier"] = app_id
        elif granted and granted != app_id:
            raise PackageError(
                f"the provisioning profile is for {granted}, not {app_id}"
            )
        # Keychain groups and the like come as wildcards: keep the app's own.
        if "keychain-access-groups" in ent:
            ent["keychain-access-groups"] = [app_id]
    links = link_entitlements(m)
    if links and profile and "com.apple.developer.associated-domains" not in ent:
        warn(
            "associated-domains: the provisioning profile doesn't grant Associated Domains "
            "(turn it on for the app id); a device won't install the app"
        )
    ent.update(links)
    if m.ios_entitlements:
        with open(m.ios_entitlements, "rb") as f:
            ent.update(plistlib.load(f))
    return ent


def sign(m, app, work, simulator):
    identity = os.environ.get("STIPPLE_IOS_SIGN_IDENTITY")
    profile_path = os.environ.get("STIPPLE_IOS_PROVISIONING_PROFILE")
    if not identity:
        # Ad hoc: runs on the simulator; a device needs a real identity.
        run(["codesign", "--force", "--sign", "-", "--timestamp=none", app], quiet=True)
        if not simulator:
            warn(
                "signed ad hoc: set STIPPLE_IOS_SIGN_IDENTITY and "
                "STIPPLE_IOS_PROVISIONING_PROFILE to install on a device or upload"
            )
        return False
    profile = None
    if profile_path:
        profile_path = Path(profile_path).expanduser()
        profile = profile_plist(profile_path)
        shutil.copy(profile_path, app / "embedded.mobileprovision")
    elif not simulator:
        warn("no STIPPLE_IOS_PROVISIONING_PROFILE: a device won't install the app")
    ent = entitlements(m, profile)
    ent_file = work / "entitlements.plist"
    with open(ent_file, "wb") as f:
        plistlib.dump(ent, f)
    cmd = ["codesign", "--force", "--sign", identity, "--timestamp=none", "--generate-entitlement-der"]
    if ent:
        cmd += ["--entitlements", ent_file]
    keychain = os.environ.get("STIPPLE_IOS_KEYCHAIN")
    if keychain:
        cmd += ["--keychain", keychain]
    run(cmd + [app], quiet=True)
    return True


# --- Building ----------------------------------------------------------------


def package(m, args, target, target_dir, work, helper):
    if sys.platform != "darwin":
        raise PackageError("iOS packaging needs macOS (codesign, and Xcode for the icon catalog)")
    simulator = args.simulator
    triple = args.ios_target or ("aarch64-apple-ios-sim" if simulator else "aarch64-apple-ios")
    work = Path(work) / ("ios-sim" if simulator else "ios")
    if work.exists():
        shutil.rmtree(work)
    work.mkdir(parents=True)
    out_dir = Path(args.out or Path(target_dir) / "stipple-package" / ("ios-sim" if simulator else "ios"))
    out_dir.mkdir(parents=True, exist_ok=True)

    env = {}
    if not os.environ.get("IPHONEOS_DEPLOYMENT_TARGET"):
        env["IPHONEOS_DEPLOYMENT_TARGET"] = m.ios_minimum
    log(f"building {target.name} for {triple}")
    cargo_build(args.manifest_path, target, triple, args.profile, args.features, env)
    exe = target.artifact(target_dir, triple, args.profile, cdylib=False)
    if not exe.exists():
        raise PackageError(f"no {exe}: is the {target.kind} a binary?")

    app_name = _bundle_name(m.name)
    app = out_dir / f"{app_name}.app"
    if app.exists():
        shutil.rmtree(app)
    app.mkdir()
    exe_name = target.name
    shutil.copy2(exe, app / exe_name)
    os.chmod(app / exe_name, 0o755)

    info = info_plist(m, exe_name, simulator)
    icons(m, app, work, helper, simulator, info)
    localize(m, app)
    with open(app / "PrivacyInfo.xcprivacy", "wb") as f:
        plistlib.dump(privacy_manifest(m), f)
    if m.assets:
        if m.assets.is_dir():
            copy_tree(m.assets, app / "assets")
        else:
            warn(f"no assets directory {m.assets}")
    with open(app / "Info.plist", "wb") as f:
        plistlib.dump(info, f)
    run(["plutil", "-lint", app / "Info.plist"], quiet=True)
    for strings in app.glob("*.lproj/InfoPlist.strings"):
        run(["plutil", "-lint", strings], quiet=True)

    # Debug symbols for crash reports (they hold only what the profile keeps:
    # `debug = "line-tables-only"` in [profile.release] gives file and line).
    if not simulator:
        dsym = out_dir / f"{app_name}.app.dSYM"
        if dsym.exists():
            shutil.rmtree(dsym)
        try:
            run(["xcrun", "dsymutil", exe, "-o", dsym], quiet=True)
            log(f"wrote {dsym}")
        except PackageError:
            warn("dsymutil failed: no dSYM")

    signed = sign(m, app, work, simulator)
    run(["codesign", "--verify", "--deep", "--strict", app], quiet=True)
    log(f"wrote {app}")
    if simulator:
        log(f"run it: xcrun simctl install booted {shlex.quote(str(app))} && "
            f"xcrun simctl launch booted {m.ios_bundle_id}")
        return app, None

    ipa = out_dir / f"{app_name}-{m.version}.ipa"
    write_ipa(app, ipa)
    log(f"wrote {ipa}" + ("" if signed else " (ad hoc: not uploadable)"))
    return app, ipa


def write_ipa(app, ipa):
    """`Payload/<Name>.app`, zipped with its modes (the executable stays
    executable) and its symlinks."""
    with zipfile.ZipFile(ipa, "w") as z:
        root = app.parent
        _zip_dir(z, "Payload/")
        for dirpath, dirnames, filenames in os.walk(app):
            dirnames.sort()
            # Directories too: an empty `<lang>.lproj` still declares a language.
            _zip_dir(z, "Payload/" + Path(dirpath).relative_to(root).as_posix() + "/")
            for name in sorted(filenames) + [d for d in dirnames if (Path(dirpath) / d).is_symlink()]:
                p = Path(dirpath) / name
                arc = "Payload/" + p.relative_to(root).as_posix()
                if p.is_symlink():
                    info = zipfile.ZipInfo(arc, date_time=(2000, 1, 1, 0, 0, 0))
                    info.external_attr = (0o120755) << 16
                    info.create_system = 3
                    z.writestr(info, os.readlink(p))
                else:
                    zip_add(z, arc, path=p, mode=os.stat(p).st_mode & 0o777)


def _zip_dir(z, name):
    info = zipfile.ZipInfo(name, date_time=(2000, 1, 1, 0, 0, 0))
    info.external_attr = (0o040755 << 16) | 0x10
    info.create_system = 3
    z.writestr(info, b"")


def _bundle_name(name):
    keep = "".join(c for c in name if c.isalnum() or c in " -_.")
    return keep.strip() or "App"
