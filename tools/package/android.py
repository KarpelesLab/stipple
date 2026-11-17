"""Android: an Android App Bundle (`.aab`) for Google Play, and a universal
APK to install for testing, with no Gradle.

Cargo builds the app's cdylib for each ABI with the NDK's clang (16 KB
pages); aapt2 compiles the resources (launcher icons, the label in every
language, the theme) and links the manifest twice, in proto format for the
bundle and in binary format for the APK; the bundle's zip is written here
(base/manifest, base/res, base/resources.pb, base/lib, base/assets,
base/dex, BundleConfig.pb and the native targeting), then signed with
jarsigner, and the APK with apksigner.
"""

import os
import platform
import shutil
import struct
import subprocess
import sys
import zipfile
from pathlib import Path
from xml.sax.saxutils import escape, quoteattr

import android_code
from common import STORED, PackageError, cargo_build, log, run, walk, warn, zip_add
from manifest import ANDROID_ABIS, android_qualifier

# Launcher icon sizes per density: the legacy icon (48 dp) and the adaptive
# icon's layers (108 dp).
DENSITIES = {"mdpi": 1.0, "hdpi": 1.5, "xhdpi": 2.0, "xxhdpi": 3.0, "xxxhdpi": 4.0}

# What the NDK names each target's clang.
CLANG_TRIPLE = {
    "aarch64-linux-android": "aarch64-linux-android",
    "armv7-linux-androideabi": "armv7a-linux-androideabi",
    "x86_64-linux-android": "x86_64-linux-android",
    "i686-linux-android": "i686-linux-android",
}

ABI_ALIAS = {"armeabi-v7a": 2, "arm64-v8a": 3, "x86": 4, "x86_64": 5}

BUNDLETOOL_VERSION = "1.15.6"

# The 16 KB page size Android 15 devices may use: Play requires 64-bit
# libraries whose LOAD segments are aligned to it.
PAGE_16K = 16384

CONFIG_CHANGES = (
    "orientation|screenSize|screenLayout|smallestScreenSize|keyboard|keyboardHidden|"
    "navigation|uiMode|density|locale|layoutDirection"
)


# --- The SDK -----------------------------------------------------------------


class Sdk:
    def __init__(self, compile_sdk):
        home = os.environ.get("ANDROID_HOME") or os.environ.get("ANDROID_SDK_ROOT")
        if not home:
            for guess in ("~/Library/Android/sdk", "~/Android/Sdk"):
                if Path(guess).expanduser().is_dir():
                    home = str(Path(guess).expanduser())
                    break
        if not home or not Path(home).is_dir():
            raise PackageError("no Android SDK: set ANDROID_HOME")
        self.home = Path(home)
        tools = sorted(
            (p for p in (self.home / "build-tools").glob("*") if (p / "aapt2").exists()),
            key=lambda p: _version_key(p.name),
        )
        if not tools:
            raise PackageError(f"no build-tools with aapt2 in {self.home}/build-tools")
        self.build_tools = tools[-1]
        self.aapt2 = self.build_tools / "aapt2"
        self.zipalign = self.build_tools / "zipalign"
        self.apksigner = self.build_tools / "apksigner"
        platforms = sorted(
            (p for p in (self.home / "platforms").glob("android-*") if (p / "android.jar").exists()),
            key=lambda p: _version_key(p.name.removeprefix("android-")),
        )
        if not platforms:
            raise PackageError(f"no platform in {self.home}/platforms")
        exact = [p for p in platforms if p.name == f"android-{compile_sdk}"]
        self.platform = exact[0] if exact else platforms[-1]
        self.api = _api(self.platform.name)
        if self.api < compile_sdk:
            warn(
                f"compiling against {self.platform.name}, older than the target SDK "
                f"{compile_sdk} (sdkmanager 'platforms;android-{compile_sdk}')"
            )
        self.android_jar = self.platform / "android.jar"
        ndk = os.environ.get("ANDROID_NDK_HOME") or os.environ.get("ANDROID_NDK_LATEST_HOME")
        if not ndk:
            ndks = sorted((self.home / "ndk").glob("*"), key=lambda p: _version_key(p.name))
            ndk = str(ndks[-1]) if ndks else None
        self.ndk = Path(ndk) if ndk else None

    def ndk_bin(self):
        if not self.ndk:
            raise PackageError("no NDK: set ANDROID_NDK_HOME (or sdkmanager 'ndk;28.2.13676358')")
        host = {"Darwin": "darwin-x86_64", "Windows": "windows-x86_64"}.get(
            platform.system(), "linux-x86_64"
        )
        return self.ndk / "toolchains" / "llvm" / "prebuilt" / host / "bin"

    def ndk_major(self):
        try:
            for line in (self.ndk / "source.properties").read_text().splitlines():
                if line.startswith("Pkg.Revision"):
                    return int(line.split("=")[1].strip().split(".")[0])
        except (OSError, ValueError):
            pass
        return 0


def _version_key(s):
    return [int(x) if x.isdigit() else -1 for x in s.replace("-", ".").split(".")]


def _api(name):
    try:
        return int(name.removeprefix("android-").split("-")[0])
    except ValueError:
        return 0


# --- Protocol buffers (the bundle's two small config messages) ---------------


def _varint(n):
    out = bytearray()
    while True:
        b = n & 0x7F
        n >>= 7
        if n:
            out.append(b | 0x80)
        else:
            out.append(b)
            return bytes(out)


def _field(num, value):
    """A length-delimited field (bytes, string or message) or a varint."""
    if isinstance(value, int):
        return _varint(num << 3) + _varint(value)
    if isinstance(value, str):
        value = value.encode()
    return _varint(num << 3 | 2) + _varint(len(value)) + value


def bundle_config(uncompressed_globs):
    """BundleConfig.pb: the bundletool version it follows, no language
    splits (the label in every language stays in the base APK, so the
    per-app language Android 13 offers works without Play's on-demand
    downloads), and the files to keep stored in the APKs Play generates."""
    bundletool = _field(2, BUNDLETOOL_VERSION)
    no_language_split = _field(1, _field(1, 3) + _field(2, 1))  # LANGUAGE, negate
    optimizations = _field(1, no_language_split)  # splits_config
    compression = b"".join(_field(1, g) for g in uncompressed_globs)
    return _field(1, bundletool) + _field(2, optimizations) + _field(3, compression)


def native_config(abis):
    """native.pb: which ABI each lib/ directory is for, so Play delivers a
    device only its own."""
    out = b""
    for abi in abis:
        targeting = _field(1, _field(1, ABI_ALIAS[abi]))
        out += _field(1, _field(1, f"lib/{abi}") + _field(2, targeting))
    return out


# --- ELF ---------------------------------------------------------------------


def elf_load_alignment(path):
    """The smallest p_align of the library's LOAD segments, and its class (32
    or 64)."""
    data = Path(path).read_bytes()
    if data[:4] != b"\x7fELF":
        raise PackageError(f"{path} isn't an ELF file")
    is64 = data[4] == 2
    endian = "<" if data[5] == 1 else ">"
    if is64:
        phoff, = struct.unpack_from(endian + "Q", data, 0x20)
        phentsize, phnum = struct.unpack_from(endian + "HH", data, 0x36)
    else:
        phoff, = struct.unpack_from(endian + "I", data, 0x1C)
        phentsize, phnum = struct.unpack_from(endian + "HH", data, 0x2A)
    aligns = []
    for i in range(phnum):
        off = phoff + i * phentsize
        p_type, = struct.unpack_from(endian + "I", data, off)
        if p_type != 1:  # PT_LOAD
            continue
        if is64:
            align, = struct.unpack_from(endian + "Q", data, off + 48)
        else:
            align, = struct.unpack_from(endian + "I", data, off + 28)
        aligns.append(align)
    return (min(aligns) if aligns else 0), (64 if is64 else 32)


# --- Resources and the manifest ------------------------------------------------


def _string(s):
    """A string resource's text: XML-escaped, with Android's own escapes."""
    s = s.replace("\\", "\\\\").replace("'", "\\'").replace('"', '\\"')
    if s[:1] in ("@", "?"):
        s = "\\" + s
    return escape(s)


def write_resources(m, res, sdk, helper):
    """The res/ tree: strings, theme, launcher icons, locale config. Returns
    what the manifest refers to."""
    res.mkdir(parents=True, exist_ok=True)
    refs = {"label": "@string/app_name", "theme": "@style/AppTheme"}

    def values(qualifier, name, body):
        d = res / (f"values-{qualifier}" if qualifier else "values")
        d.mkdir(exist_ok=True)
        (d / name).write_text(
            '<?xml version="1.0" encoding="utf-8"?>\n<resources>\n' + body + "</resources>\n",
            encoding="utf-8",
        )

    values(None, "strings.xml", f'  <string name="app_name">{_string(m.name)}</string>\n')
    for lang in m.languages:
        name = m.name_in(lang)
        if name:
            values(
                android_qualifier(lang),
                "strings.xml",
                f'  <string name="app_name">{_string(name)}</string>\n',
            )
    values(None, "themes.xml", f'  <style name="AppTheme" parent={quoteattr(m.android_theme)}/>\n')
    if m.android_splash:
        if sdk.api < 31:
            warn("android.splash-background needs a platform of API 31 or later; left out")
        else:
            values(
                "v31",
                "themes.xml",
                f'  <style name="AppTheme" parent={quoteattr(m.android_theme)}>\n'
                f'    <item name="android:windowSplashScreenBackground">'
                f"{_color(m.android_splash)}</item>\n"
                "  </style>\n",
            )

    if len(m.languages) > 1:
        if sdk.api < 33:
            warn("the per-app language list needs a platform of API 33 or later; left out")
        else:
            (res / "xml").mkdir(exist_ok=True)
            locales = "".join(f'  <locale android:name="{lang}"/>\n' for lang in m.languages)
            (res / "xml" / "locales_config.xml").write_text(
                '<?xml version="1.0" encoding="utf-8"?>\n'
                '<locale-config xmlns:android="http://schemas.android.com/apk/res/android">\n'
                + locales
                + "</locale-config>\n",
                encoding="utf-8",
            )
            refs["locale_config"] = "@xml/locales_config"

    if m.icon:
        jobs = []
        bg = m.icon_background
        for d, k in DENSITIES.items():
            mip = res / f"mipmap-{d}"
            # The legacy icon: the whole image over the background.
            jobs.append((mip / "ic_launcher.png", round(48 * k), 1.0, bg, m.icon))
            # The adaptive icon's foreground: the image over the visible part
            # of a 108 dp square (the launcher masks and moves it).
            jobs.append(
                (mip / "ic_launcher_foreground.png", round(108 * k), m.android_icon_scale, None,
                 m.android_icon_foreground)
            )
            if m.android_icon_monochrome:
                jobs.append(
                    (mip / "ic_launcher_monochrome.png", round(108 * k), m.android_icon_scale,
                     None, m.android_icon_monochrome)
                )
        helper.icons(jobs)
        values(None, "colors.xml", f'  <color name="ic_launcher_background">{_color(bg)}</color>\n')
        any_dpi = res / "mipmap-anydpi-v26"
        any_dpi.mkdir(exist_ok=True)
        mono = (
            '  <monochrome android:drawable="@mipmap/ic_launcher_monochrome"/>\n'
            if m.android_icon_monochrome and sdk.api >= 33
            else ""
        )
        (any_dpi / "ic_launcher.xml").write_text(
            '<?xml version="1.0" encoding="utf-8"?>\n'
            '<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">\n'
            '  <background android:drawable="@color/ic_launcher_background"/>\n'
            '  <foreground android:drawable="@mipmap/ic_launcher_foreground"/>\n'
            + mono
            + "</adaptive-icon>\n",
            encoding="utf-8",
        )
        refs["icon"] = "@mipmap/ic_launcher"
    else:
        warn("no icon in the manifest: the app gets Android's default icon")
    return refs


def _color(c):
    r, g, b, a = c
    return f"#{a:02x}{r:02x}{g:02x}{b:02x}" if a != 255 else f"#{r:02x}{g:02x}{b:02x}"


def screen_orientation(orientations):
    """android:screenOrientation for the allowed orientations, or None for
    any."""
    o = set(orientations)
    portrait = {"portrait", "portrait-upside-down"}
    landscape = {"landscape-left", "landscape-right"}
    if o <= portrait:
        return "sensorPortrait" if o == portrait else "portrait"
    if o <= landscape:
        return "sensorLandscape" if o == landscape else (
            "landscape" if "landscape-left" in o else "reverseLandscape"
        )
    return None


def has_code(m):
    """Whether the package carries dex files: Stipple's own, the Play
    Billing Library's, or the app's."""
    return bool(needs_dex(m) or m.android_billing or m.android_java or m.android_dex)


def manifest_xml(m, lib_name, refs):
    perms = list(m.android_permissions)
    if m.android_alarms:
        for p in ("android.permission.RECEIVE_BOOT_COMPLETED", "android.permission.USE_EXACT_ALARM"):
            if p not in perms:
                perms.append(p)
    lines = [
        '<?xml version="1.0" encoding="utf-8"?>',
        "<!-- Written by cargo-stipple from the app's packaging manifest. -->",
        '<manifest xmlns:android="http://schemas.android.com/apk/res/android"',
        f"    package={quoteattr(m.android_package)}"
        f' android:versionCode="{m.build}" android:versionName={quoteattr(m.version)}>',
        f'  <uses-sdk android:minSdkVersion="{m.android_min_sdk}"'
        f' android:targetSdkVersion="{m.android_target_sdk}"/>',
    ]
    if m.android_alarms:
        lines.append(
            '  <uses-permission android:name="android.permission.SCHEDULE_EXACT_ALARM"'
            ' android:maxSdkVersion="32"/>'
        )
    lines += [f"  <uses-permission android:name={quoteattr(p)}/>" for p in perms]
    app = [
        f'android:label="{refs["label"]}"',
        f'android:theme="{refs["theme"]}"',
        f'android:hasCode="{"true" if has_code(m) else "false"}"',
        'android:enableOnBackInvokedCallback="true"',
    ]
    if "icon" in refs:
        app.append(f'android:icon="{refs["icon"]}"')
    if "locale_config" in refs:
        app.append(f'android:localeConfig="{refs["locale_config"]}"')
    if m.android_debuggable:
        app.append('android:debuggable="true"')
    lines.append("  <application " + "\n      ".join(app) + ">")
    activity = [
        'android:name="android.app.NativeActivity"',
        'android:exported="true"',
        f'android:configChanges="{CONFIG_CHANGES}"',
    ]
    so = screen_orientation(m.orientations)
    if so:
        activity.append(f'android:screenOrientation="{so}"')
    lines += [
        "    <activity " + "\n        ".join(activity) + ">",
        f'      <meta-data android:name="android.app.lib_name" android:value={quoteattr(lib_name)}/>',
        "      <intent-filter>",
        '        <action android:name="android.intent.action.MAIN"/>',
        '        <category android:name="android.intent.category.LAUNCHER"/>',
        "      </intent-filter>",
        "    </activity>",
    ]
    if m.android_alarms:
        lines += [
            '    <receiver android:name="org.stipple.android.Alarm" android:exported="false">',
            "      <intent-filter>",
            '        <action android:name="android.intent.action.BOOT_COMPLETED"/>',
            '        <action android:name="android.intent.action.MY_PACKAGE_REPLACED"/>',
            "      </intent-filter>",
            "    </receiver>",
        ]
    lines += link_activity(m)
    if m.android_drag_files:
        # Files dragged out of the app: read-only, granted per URI.
        lines.append(
            '    <provider android:name="org.stipple.android.Files"'
            f' android:authorities={quoteattr(m.android_package + ".stipple.files")}'
            ' android:exported="false" android:grantUriPermissions="true"/>'
        )
    lines += ["  </application>", "</manifest>", ""]
    return "\n".join(lines)


def needs_dex(m):
    """Whether the APK carries Stipple's classes.dex (receiver_dex): for the
    Alarm receiver, the Files provider, or the Link activity."""
    return bool(m.android_alarms or m.android_drag_files or m.url_schemes or m.link_domains)


def link_activity(m):
    """Stipple's Link activity, with a VIEW intent filter for the app's URL
    schemes and one verified (autoVerify) https filter per associated domain,
    when the manifest has any: a windowless trampoline that hands the URL to
    the app and brings its NativeActivity forward, which doesn't pass
    onNewIntent on (src/platform/backend/android/open_url.rs)."""
    if not (m.url_schemes or m.link_domains):
        return []
    view = [
        '        <action android:name="android.intent.action.VIEW"/>',
        '        <category android:name="android.intent.category.DEFAULT"/>',
        '        <category android:name="android.intent.category.BROWSABLE"/>',
    ]
    lines = [
        '    <activity android:name="org.stipple.android.Link" android:exported="true"',
        '        android:taskAffinity="" android:excludeFromRecents="true" android:noHistory="true"',
        '        android:theme="@android:style/Theme.NoDisplay">',
    ]
    if m.url_schemes:
        lines += ["      <intent-filter>", *view]
        lines += [f"        <data android:scheme={quoteattr(s)}/>" for s in m.url_schemes]
        lines.append("      </intent-filter>")
    for d in m.link_domains:
        lines += ['      <intent-filter android:autoVerify="true">', *view]
        lines.append('        <data android:scheme="https"/>')
        lines.append(f"        <data android:host={quoteattr(d['host'])}/>")
        if d["path"] and d["path"] != "/":
            lines.append(f"        <data android:pathPrefix={quoteattr(d['path'])}/>")
        lines.append("      </intent-filter>")
    lines.append("    </activity>")
    return lines


# --- Signing -----------------------------------------------------------------


class Keystore:
    """The upload key, from STIPPLE_ANDROID_KEYSTORE (and its _PASSWORD,
    _KEY_ALIAS, _KEY_PASSWORD), else a debug key made on the spot."""

    def __init__(self, work):
        env = os.environ
        if env.get("STIPPLE_ANDROID_KEYSTORE"):
            self.path = Path(env["STIPPLE_ANDROID_KEYSTORE"]).expanduser()
            if not self.path.exists():
                raise PackageError(f"STIPPLE_ANDROID_KEYSTORE: no file {self.path}")
            self.store_pass = env.get("STIPPLE_ANDROID_KEYSTORE_PASSWORD")
            if not self.store_pass:
                raise PackageError("set STIPPLE_ANDROID_KEYSTORE_PASSWORD")
            self.alias = env.get("STIPPLE_ANDROID_KEY_ALIAS") or self._only_alias()
            self.key_pass = env.get("STIPPLE_ANDROID_KEY_PASSWORD") or self.store_pass
            self.debug = False
        else:
            self.path = Path(work) / "debug.keystore"
            self.store_pass = self.key_pass = "android"
            self.alias = "androiddebugkey"
            self.debug = True
            if not self.path.exists():
                run(
                    ["keytool", "-genkeypair", "-keystore", self.path, "-storepass", "android",
                     "-keypass", "android", "-alias", self.alias, "-dname", "CN=Android Debug",
                     "-keyalg", "RSA", "-keysize", "2048", "-validity", "10000"],
                    quiet=True,
                )

    def _only_alias(self):
        out = subprocess.run(
            ["keytool", "-list", "-keystore", str(self.path), "-storepass", self.store_pass],
            capture_output=True, text=True,
        ).stdout
        aliases = [line.split(",")[0] for line in out.splitlines() if "PrivateKeyEntry" in line]
        if len(aliases) != 1:
            raise PackageError("set STIPPLE_ANDROID_KEY_ALIAS (the keystore has several keys)")
        return aliases[0]

    def jarsign(self, path):
        run(
            ["jarsigner", "-keystore", self.path, "-storepass:env", "STIPPLE_KS_PASS",
             "-keypass:env", "STIPPLE_KEY_PASS", "-sigalg", "SHA256withRSA",
             "-digestalg", "SHA-256", path, self.alias],
            quiet=True, env=self._env(),
        )

    def apksign(self, sdk, src, out):
        run(
            [sdk.apksigner, "sign", "--ks", self.path, "--ks-pass", "env:STIPPLE_KS_PASS",
             "--key-pass", "env:STIPPLE_KEY_PASS", "--ks-key-alias", self.alias,
             "--out", out, src],
            quiet=True, env=self._env(),
        )

    def _env(self):
        return {**os.environ, "STIPPLE_KS_PASS": self.store_pass, "STIPPLE_KEY_PASS": self.key_pass}


# --- Building ----------------------------------------------------------------


def build_libs(m, sdk, args, target, target_dir, work):
    """The cdylib for every ABI, as {abi: path}."""
    bin_dir = sdk.ndk_bin()
    api = max(m.android_min_sdk, 21)
    libs = {}
    for abi in m.android_abis:
        triple = ANDROID_ABIS[abi]
        clang = bin_dir / f"{CLANG_TRIPLE[triple]}{api}-clang"
        if platform.system() == "Windows":
            clang = clang.with_suffix(".cmd")
        if not clang.exists():
            raise PackageError(f"no NDK clang at {clang}")
        up = triple.upper().replace("-", "_")
        low = triple.replace("-", "_")
        env = {
            f"CARGO_TARGET_{up}_LINKER": str(clang),
            f"CC_{low}": str(clang),
            f"AR_{low}": str(bin_dir / "llvm-ar"),
        }
        # 16 KB pages (NDK r28 does it by default; older ones don't).
        flag = f"-C link-arg=-Wl,-z,max-page-size={PAGE_16K}"
        if os.environ.get("RUSTFLAGS") is not None:
            env["RUSTFLAGS"] = (os.environ["RUSTFLAGS"] + " " + flag).strip()
        else:
            key = f"CARGO_TARGET_{up}_RUSTFLAGS"
            env[key] = (os.environ.get(key, "") + " " + flag).strip()
        log(f"building {target.name} for {abi} ({triple})")
        cargo_build(args.manifest_path, target, triple, args.profile, args.features, env)
        lib = target.artifact(target_dir, triple, args.profile, cdylib=True)
        if not lib.exists():
            raise PackageError(f"no {lib}: is the {target.kind} a cdylib (crate-type = [\"cdylib\"])?")
        align, bits = elf_load_alignment(lib)
        if bits == 64 and align < PAGE_16K:
            raise PackageError(
                f"{lib.name} ({abi}) has {align}-byte page alignment; Play requires 16 KB"
            )
        libs[abi] = lib
    return libs


def package(m, args, target, target_dir, work, helper):
    sdk = Sdk(m.android_target_sdk)
    work = Path(work) / "android"
    if work.exists():
        shutil.rmtree(work)
    work.mkdir(parents=True)
    out_dir = Path(args.out or Path(target_dir) / "stipple-package" / "android")
    out_dir.mkdir(parents=True, exist_ok=True)

    lib_name = m.android_lib_name or target.name.replace("-", "_")
    libs = build_libs(m, sdk, args, target, target_dir, work)

    dex = None
    if needs_dex(m):
        dex = work / "classes.dex"
        helper.dex(lib_name, dex)

    # Resources and the manifest.
    refs = write_resources(m, work / "res", sdk, helper)
    manifest = work / "AndroidManifest.xml"
    if m.android_manifest:
        shutil.copy(m.android_manifest, manifest)
    else:
        manifest.write_text(manifest_xml(m, lib_name, refs), encoding="utf-8")
    # The dex files: the receiver's, the Play Billing Library's, then the
    # app's own (its Java sources, then its prebuilt dex files).
    dexes = [dex] if dex else []
    if m.android_billing:
        dexes += billing(m, manifest, work / "billing")
    if m.android_java:
        dexes += android_code.compile_java(
            sdk, m.android_java, work / "java", m.android_min_sdk, args.profile == "release"
        )
    dexes += [android_code.check_dex(p) for p in m.android_dex]
    dexes = android_code.dex_names(dexes)
    if m.android_manifest_fragment:
        android_code.merge_fragment_file(
            manifest, m.android_manifest_fragment, m.android_package,
            has_code(m) or bool(m.android_manifest),
        )
    compiled = work / "compiled.zip"
    run([sdk.aapt2, "compile", "--dir", work / "res", "-o", compiled], quiet=True)
    link = [
        sdk.aapt2, "link", "-I", sdk.android_jar, "--manifest", manifest,
        "--min-sdk-version", m.android_min_sdk, "--target-sdk-version", m.android_target_sdk,
        "--version-code", m.build, "--version-name", m.version, "--replace-version",
    ]
    if m.android_manifest:
        link += ["--rename-manifest-package", m.android_package]
    proto_apk, binary_apk = work / "linked-proto.apk", work / "linked.apk"
    run(link + ["--proto-format", "-o", proto_apk, compiled], quiet=True)
    run(link + ["-o", binary_apk, compiled], quiet=True)

    assets = walk(m.assets) if m.assets and m.assets.is_dir() else []
    if m.assets and not m.assets.is_dir():
        warn(f"no assets directory {m.assets}")
    stem = f"{_file_name(m.name)}-{m.version}"
    keystore = Keystore(work.parent)

    # The bundle.
    aab = out_dir / f"{stem}.aab"
    unsigned = work / "unsigned.aab"
    with zipfile.ZipFile(proto_apk) as src, zipfile.ZipFile(unsigned, "w") as z:
        globs = [f"assets/**{ext}" for ext in sorted(STORED)]
        zip_add(z, "BundleConfig.pb", bundle_config(globs))
        for info in src.infolist():
            data = src.read(info)
            if info.filename == "AndroidManifest.xml":
                zip_add(z, "base/manifest/AndroidManifest.xml", data)
            else:
                zip_add(z, f"base/{info.filename}", data)
        for path, name in assets:
            zip_add(z, f"base/assets/{name}", path=path)
        for abi, lib in libs.items():
            zip_add(z, f"base/lib/{abi}/lib{lib_name}.so", path=lib)
        zip_add(z, "base/native.pb", native_config(list(libs)))
        for name, path in dexes:
            zip_add(z, f"base/dex/{name}", path=path)
    shutil.copy(unsigned, aab)
    keystore.jarsign(aab)
    log(f"wrote {aab}")

    # The universal APK: everything in one, libraries stored (and aligned to
    # 16 KB) so they load straight from the APK.
    apk = out_dir / f"{stem}-universal.apk"
    raw = work / "universal-unaligned.apk"
    with zipfile.ZipFile(binary_apk) as src, zipfile.ZipFile(raw, "w") as z:
        for info in src.infolist():
            zip_add(z, info.filename, src.read(info), stored=info.compress_type == zipfile.ZIP_STORED)
        for name, path in dexes:
            zip_add(z, name, path=path)
        for path, name in assets:
            zip_add(z, f"assets/{name}", path=path, stored=path.suffix.lower() in STORED)
        for abi, lib in libs.items():
            zip_add(z, f"lib/{abi}/lib{lib_name}.so", path=lib, stored=True)
    aligned = work / "universal-aligned.apk"
    help_text = subprocess.run([str(sdk.zipalign)], capture_output=True, text=True)
    page = ["-P", "16"] if "-P" in (help_text.stdout + help_text.stderr) else ["-p"]
    run([sdk.zipalign, "-f", *page, "4", raw, aligned], quiet=True)
    keystore.apksign(sdk, aligned, apk)
    log(f"wrote {apk}")
    if keystore.debug:
        warn(
            "signed with a debug key: set STIPPLE_ANDROID_KEYSTORE (and _PASSWORD, "
            "_KEY_ALIAS) to sign with your upload key for Google Play"
        )
    if m.android_debuggable:
        warn("android.debuggable is set: Google Play rejects debuggable apps")

    bundletool = find_bundletool()
    if bundletool:
        run(bundletool + ["validate", "--bundle", aab], quiet=True)
        log("bundletool validate: ok")
    else:
        log("bundletool not found (set BUNDLETOOL to its jar to validate the bundle)")
    return aab, apk


def billing(m, manifest, out):
    """The Play Billing Library and its dependencies for `platform::store`
    (tools/android/billing.py: fetched from Google's Maven repository, dexed),
    as dex files in order; adds the library's manifest entries to
    `manifest`."""
    script = Path(__file__).resolve().parent.parent / "android" / "billing.py"
    run(
        [
            sys.executable, script, out, "--version", m.android_billing,
            "--manifest", manifest, "--package", m.android_package,
            "--min-api", m.android_min_sdk,
        ],
        quiet=True,
    )
    shutil.copy(out / "AndroidManifest.xml", manifest)
    files = sorted(out.glob("classes*.dex"), key=lambda p: int(p.stem[7:] or 1))
    log(f"Play Billing Library {m.android_billing}: {len(files)} dex file(s)")
    return files


def find_bundletool():
    bt = os.environ.get("BUNDLETOOL")
    if bt:
        return ["java", "-jar", bt] if bt.endswith(".jar") else [bt]
    exe = shutil.which("bundletool")
    return [exe] if exe else None


def _file_name(name):
    keep = "".join(c if c.isalnum() or c in "-_." else "-" for c in name)
    return keep.strip("-") or "app"
