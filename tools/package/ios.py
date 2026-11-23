"""iOS: an `.app` bundle (device or simulator), signed, and for a device an
`.ipa` to upload to App Store Connect, with no Xcode project.

The bundle holds the binary, an Info.plist written from the manifest, the
app icon (an asset catalog compiled by Xcode's actool when Xcode is there,
else loose PNGs), the localized display name and permission prompts
(`<lang>.lproj/InfoPlist.strings`), the privacy manifest, the app's assets,
and, when signing for a device, the provisioning profile. codesign signs it
with the entitlements the profile grants. App extensions go in `PlugIns/`
and are signed first (ios_extensions.py).
"""

import hashlib
import os
import plistlib
import shlex
import shutil
import subprocess
import sys
import zipfile
from pathlib import Path

import ios_extensions
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
    """The app's entitlements to sign with: the profile's (its wildcard app
    id made this app's), the manifest's associated domains and app groups,
    then the manifest's file on top."""
    return bundle_entitlements(m.ios_bundle_id, profile, m.ios_entitlements, m.ios_app_groups,
                               link_entitlements(m))


def bundle_entitlements(bundle_id, profile, file, groups, extra=None):
    """A bundle's entitlements (the app's or an extension's): `profile`'s
    for `bundle_id`, `extra` (the app's associated domains), the app groups
    (ios_extensions.group_entitlements), then `file`'s on top."""
    ent = {}
    if profile:
        ent = dict(profile.get("Entitlements", {}))
        team = (profile.get("TeamIdentifier") or [""])[0]
        app_id = f"{team}.{bundle_id}"
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
    if extra and profile and "com.apple.developer.associated-domains" in extra \
            and "com.apple.developer.associated-domains" not in ent:
        warn(
            "associated-domains: the provisioning profile doesn't grant Associated Domains "
            "(turn it on for the app id); a device won't install the app"
        )
    ent.update(extra or {})
    ent.update(ios_extensions.group_entitlements(groups))
    if file:
        with open(file, "rb") as f:
            ent.update(plistlib.load(f))
    return ent


# --- The simulator's entitlements ---------------------------------------------

# The team part of a simulator build's application identifier when
# STIPPLE_IOS_TEAM_ID doesn't give one (the simulator takes any).
SIMULATOR_TEAM = "SIMULATOR"


def simulator_entitlements(m, env=os.environ):
    """What a simulator build is entitled to: its application identifier and
    keychain access group (the Keychain refuses an app with neither:
    errSecMissingEntitlement), then the app's own entitlements."""
    team = env.get("STIPPLE_IOS_TEAM_ID") or SIMULATOR_TEAM
    app_id = f"{team}.{m.ios_bundle_id}"
    ent = {"application-identifier": app_id, "keychain-access-groups": [app_id]}
    ent.update(bundle_entitlements(m.ios_bundle_id, None, m.ios_entitlements, m.ios_app_groups,
                                   link_entitlements(m)))
    return ent


def simulator_link_args(ent, work, derq="derq"):
    """The rustc arguments embedding `ent` in the executable, as Xcode links a
    simulator build: the XML plist as `__TEXT,__entitlements`, and its DER
    form (when macOS's derq is there) as `__TEXT,__ents_der`. The simulator
    reads them there, not from the signature (an ad hoc signature carrying
    application-identifier doesn't launch). The files are named by their
    content, so that a change relinks."""
    xml = plistlib.dumps(ent)
    path = Path(work) / f"simulator-{hashlib.sha256(xml).hexdigest()[:16]}.xcent"
    path.write_bytes(xml)
    sections = [("__entitlements", path)]
    der = Path(f"{path}.der")
    try:
        subprocess.run([derq, "query", "-f", "xml", "-i", str(path), "-o", str(der), "--raw"],
                       check=True, capture_output=True)
        sections.append(("__ents_der", der))
    except (OSError, subprocess.CalledProcessError):
        warn("no derq: the simulator build carries its entitlements as XML only")
    args = []
    for section, file in sections:
        for a in ("-sectcreate", "__TEXT", section, str(file)):
            args += ["-C", "link-arg=-Xlinker", "-C", f"link-arg={a}"]
    return args


def sign(m, app, work, simulator):
    """Sign the app (after its extensions: the app's seal covers them)."""
    return sign_bundle(
        app, m.ios_bundle_id, m.ios_entitlements, m.ios_app_groups,
        "STIPPLE_IOS_PROVISIONING_PROFILE", work / "entitlements.plist", simulator, "the app",
        extra=link_entitlements(m),
    )


def sign_bundle(bundle, bundle_id, ent_path, groups, profile_var, ent_file, simulator, who,
                extra=None):
    identity = os.environ.get("STIPPLE_IOS_SIGN_IDENTITY")
    profile_path = os.environ.get(profile_var)
    if not identity:
        # Ad hoc: runs on the simulator; a device needs a real identity. The
        # entitlements still go in, for the simulator (app groups).
        cmd = ["codesign", "--force", "--sign", "-", "--timestamp=none"]
        ent = bundle_entitlements(bundle_id, None, ent_path, groups, extra)
        if ent:
            with open(ent_file, "wb") as f:
                plistlib.dump(ent, f)
            cmd += ["--entitlements", ent_file]
        run(cmd + [bundle], quiet=True)
        if not simulator and who == "the app" and not xcode_signing():
            warn(
                "signed ad hoc: set STIPPLE_IOS_SIGN_IDENTITY and "
                "STIPPLE_IOS_PROVISIONING_PROFILE to install on a device or upload"
            )
        return False
    profile = None
    if profile_path:
        profile_path = Path(profile_path).expanduser()
        profile = profile_plist(profile_path)
        shutil.copy(profile_path, bundle / "embedded.mobileprovision")
        for w in ios_extensions.check_profile_groups(groups, profile.get("Entitlements", {}), who):
            warn(w)
    elif not simulator:
        warn(f"no {profile_var}: a device won't install {who}")
    ent = bundle_entitlements(bundle_id, profile, ent_path, groups, extra)
    with open(ent_file, "wb") as f:
        plistlib.dump(ent, f)
    cmd = ["codesign", "--force", "--sign", identity, "--timestamp=none", "--generate-entitlement-der"]
    if ent:
        cmd += ["--entitlements", ent_file]
    keychain = os.environ.get("STIPPLE_IOS_KEYCHAIN")
    if keychain:
        cmd += ["--keychain", keychain]
    run(cmd + [bundle], quiet=True)
    return True


# --- Signing through Xcode (cloud-managed certificates) -----------------------

EXPORT_METHODS = ("app-store-connect", "release-testing", "debugging", "enterprise",
                  "app-store", "ad-hoc", "development")


def xcode_signing():
    """Whether Xcode signs the device build: a team is set and no identity
    of our own (STIPPLE_IOS_SIGN_IDENTITY) is."""
    return bool(os.environ.get("STIPPLE_IOS_TEAM_ID")) and not os.environ.get("STIPPLE_IOS_SIGN_IDENTITY")


def asc_key_args(env=os.environ):
    """xcodebuild's App Store Connect API key arguments, from STIPPLE_ASC_KEY_ID,
    STIPPLE_ASC_ISSUER_ID and STIPPLE_ASC_KEY_PATH (default: AuthKey_<ID>.p8
    where altool looks: ./private_keys, ~/private_keys, ~/.private_keys,
    ~/.appstoreconnect/private_keys). None without a key id: Xcode then uses
    the account signed in under Settings > Accounts."""
    key_id = env.get("STIPPLE_ASC_KEY_ID")
    if not key_id:
        return []
    issuer = env.get("STIPPLE_ASC_ISSUER_ID")
    if not issuer:
        raise PackageError("STIPPLE_ASC_KEY_ID is set but not STIPPLE_ASC_ISSUER_ID (the key's issuer id)")
    path = env.get("STIPPLE_ASC_KEY_PATH")
    if path:
        path = Path(path).expanduser()
        if not path.is_file():
            raise PackageError(f"STIPPLE_ASC_KEY_PATH: no file {path}")
    else:
        name = f"AuthKey_{key_id}.p8"
        dirs = [Path("private_keys"), Path("~/private_keys"), Path("~/.private_keys"),
                Path("~/.appstoreconnect/private_keys")]
        found = [d.expanduser() / name for d in dirs if (d.expanduser() / name).is_file()]
        if not found:
            raise PackageError(
                f"no {name} in ./private_keys, ~/private_keys, ~/.private_keys or "
                "~/.appstoreconnect/private_keys: set STIPPLE_ASC_KEY_PATH to the key file"
            )
        path = found[0]
    return ["-authenticationKeyPath", path.resolve(), "-authenticationKeyID", key_id,
            "-authenticationKeyIssuerID", issuer]


def export_options(env=os.environ):
    """The -exportOptionsPlist for xcodebuild: automatic signing for the team,
    the method, and whether to upload to App Store Connect."""
    method = env.get("STIPPLE_IOS_EXPORT_METHOD") or "app-store-connect"
    if method not in EXPORT_METHODS:
        raise PackageError(f"STIPPLE_IOS_EXPORT_METHOD: {method!r} isn't one of {', '.join(EXPORT_METHODS[:4])}")
    upload = env.get("STIPPLE_IOS_UPLOAD", "") not in ("", "0", "false", "no")
    if upload and method not in ("app-store-connect", "app-store"):
        raise PackageError(f"STIPPLE_IOS_UPLOAD: only the app-store-connect method uploads, not {method}")
    return {
        "method": method,
        "teamID": env["STIPPLE_IOS_TEAM_ID"],
        "signingStyle": "automatic",
        # The build number is the manifest's (or --build's), not Xcode's.
        "manageAppVersionAndBuildNumber": False,
        "uploadSymbols": True,
        "destination": "upload" if upload else "export",
    }


def write_archive(app, dsym, archive, team):
    """An `.xcarchive` around the (ad hoc signed) bundle and its dSYM, as
    `xcodebuild archive` would leave it: what `-exportArchive` re-signs."""
    if archive.exists():
        shutil.rmtree(archive)
    apps = archive / "Products" / "Applications"
    apps.mkdir(parents=True)
    shutil.copytree(app, apps / app.name, symlinks=True)
    if dsym and dsym.exists():
        shutil.copytree(dsym, archive / "dSYMs" / dsym.name, symlinks=True)
    with open(app / "Info.plist", "rb") as f:
        info = plistlib.load(f)
    import datetime

    with open(archive / "Info.plist", "wb") as f:
        plistlib.dump({
            "ApplicationProperties": {
                "ApplicationPath": f"Applications/{app.name}",
                "Architectures": ["arm64"],
                "CFBundleIdentifier": info["CFBundleIdentifier"],
                "CFBundleShortVersionString": info["CFBundleShortVersionString"],
                "CFBundleVersion": info["CFBundleVersion"],
                "Team": team,
            },
            "ArchiveVersion": 2,
            "CreationDate": datetime.datetime.now(datetime.UTC).replace(tzinfo=None),
            "Name": app.stem,
            "SchemeName": app.stem,
        }, f)


def xcode_export(app, dsym, work, ipa):
    """Have Xcode sign the bundle with the team's cloud-managed distribution
    certificate, making the provisioning profiles it needs
    (`-allowProvisioningUpdates`), signed in through an App Store Connect API
    key or Xcode's account; write the `.ipa` (and put the signed bundle in
    place of `app`), or upload it with STIPPLE_IOS_UPLOAD. Returns whether
    there is an `.ipa`."""
    opts = export_options()
    archive = work / f"{app.stem}.xcarchive"
    write_archive(app, dsym, archive, opts["teamID"])
    opts_path = work / "ExportOptions.plist"
    with open(opts_path, "wb") as f:
        plistlib.dump(opts, f)
    export = work / "export"
    if export.exists():
        shutil.rmtree(export)
    log(f"signing through Xcode for team {opts['teamID']} ({opts['method']})")
    key = asc_key_args()
    try:
        run(["xcodebuild", "-exportArchive", "-archivePath", archive, "-exportOptionsPlist", opts_path,
             "-exportPath", export, "-allowProvisioningUpdates", *key], quiet=True)
    except PackageError as e:
        how = "the App Store Connect key (an Admin or App Manager key)" if key else \
            "the account signed in under Xcode > Settings > Accounts (or set STIPPLE_ASC_KEY_ID)"
        raise PackageError(
            f"{e}: Xcode couldn't sign for team {opts['teamID']} through {how}; the app's "
            "bundle id (and each extension's) must be registered in the account, with its capabilities"
        ) from None
    if opts["destination"] == "upload":
        log("uploaded to App Store Connect")
        return False
    exported = sorted(export.glob("*.ipa"))
    if not exported:
        raise PackageError(f"xcodebuild wrote no .ipa in {export}")
    shutil.move(exported[0], ipa)
    # The signed bundle, from the .ipa, in place of the ad hoc one.
    unpacked = work / "signed"
    if unpacked.exists():
        shutil.rmtree(unpacked)
    run(["ditto", "-x", "-k", ipa, unpacked], quiet=True)
    shutil.rmtree(app)
    shutil.move(unpacked / "Payload" / app.name, app)
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
    if simulator:
        # `cargo rustc`: the entitlements' sections go in the app's
        # executable only, without rebuilding its dependencies.
        cmd = ["cargo", "rustc", "--manifest-path", args.manifest_path, "--target", triple]
        cmd += ["--release"] if args.profile == "release" else ["--profile", args.profile]
        cmd += target.cargo_args()
        if args.features:
            cmd += ["--features", args.features]
        cmd += ["--"] + simulator_link_args(simulator_entitlements(m), work)
        run(cmd, env={**os.environ, **env})
    else:
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

    # The extensions in PlugIns/, each built and signed before the app.
    ios_extensions.package_all(m, args, target.package, triple, target_dir, app, out_dir, work,
                               simulator, env)
    signed = sign(m, app, work, simulator)
    run(["codesign", "--verify", "--deep", "--strict", app], quiet=True)
    log(f"wrote {app}")
    if simulator:
        log(f"run it: xcrun simctl install booted {shlex.quote(str(app))} && "
            f"xcrun simctl launch booted {m.ios_bundle_id}")
        return app, None

    ipa = out_dir / f"{app_name}-{m.version}.ipa"
    if xcode_signing():
        dsym = out_dir / f"{app_name}.app.dSYM"
        if not xcode_export(app, dsym, work, ipa):
            return app, None
        run(["codesign", "--verify", "--deep", "--strict", app], quiet=True)
        log(f"wrote {ipa} (signed by Xcode)")
        return app, ipa
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
