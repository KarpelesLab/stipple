"""The packaging manifest: `[package.metadata.stipple]` in the app's
Cargo.toml (or the same keys at the top of a separate TOML file, given with
`--config`), checked and filled in with defaults. See docs/packaging.md for
every key.
"""

import json
import os
import re
import subprocess
import tomllib
from pathlib import Path


class ManifestError(Exception):
    pass


# The orientations a manifest names, and what each one allows.
ORIENTATIONS = {
    "portrait": ["portrait"],
    "portrait-upside-down": ["portrait-upside-down"],
    "landscape-left": ["landscape-left"],
    "landscape-right": ["landscape-right"],
    "landscape": ["landscape-left", "landscape-right"],
    "all": ["portrait", "portrait-upside-down", "landscape-left", "landscape-right"],
}

# Short names for the iOS usage descriptions (the prompts iOS shows the first
# time an app asks for a permission). A key that starts with "NS" is used as
# it is.
IOS_USAGE = {
    "camera": "NSCameraUsageDescription",
    "microphone": "NSMicrophoneUsageDescription",
    "photo-library": "NSPhotoLibraryUsageDescription",
    "photo-library-add": "NSPhotoLibraryAddUsageDescription",
    "location": "NSLocationWhenInUseUsageDescription",
    "location-always": "NSLocationAlwaysAndWhenInUseUsageDescription",
    "contacts": "NSContactsUsageDescription",
    "calendars": "NSCalendarsFullAccessUsageDescription",
    "reminders": "NSRemindersFullAccessUsageDescription",
    "bluetooth": "NSBluetoothAlwaysUsageDescription",
    "motion": "NSMotionUsageDescription",
    "face-id": "NSFaceIDUsageDescription",
    "speech": "NSSpeechRecognitionUsageDescription",
    "local-network": "NSLocalNetworkUsageDescription",
    "tracking": "NSUserTrackingUsageDescription",
    "health-share": "NSHealthShareUsageDescription",
    "health-update": "NSHealthUpdateUsageDescription",
}

# The ABIs an Android package carries, and the Rust target each is built with.
ANDROID_ABIS = {
    "arm64-v8a": "aarch64-linux-android",
    "armeabi-v7a": "armv7-linux-androideabi",
    "x86_64": "x86_64-linux-android",
    "x86": "i686-linux-android",
}

# Play's requirement for new apps and updates (API 35, Android 15, since
# August 2025).
ANDROID_TARGET_SDK = 35

APP_ID = re.compile(r"^[A-Za-z][A-Za-z0-9_]*(\.[A-Za-z][A-Za-z0-9_]*)+$")
IOS_ID = re.compile(r"^[A-Za-z0-9.-]+$")
COLOR = re.compile(r"^#([0-9A-Fa-f]{6}|[0-9A-Fa-f]{8})$")
LANG = re.compile(r"^[a-z]{2,3}(-[A-Z][a-z]{3})?(-([A-Z]{2}|[0-9]{3}))?$")


# The Play Billing Library version `android.billing = true` packages: the
# newest 8.x (9.x needs resources an aapt2 build without Gradle doesn't merge).
BILLING_VERSION = "8.3.0"


def parse_color(value, key):
    """`#rrggbb` or `#rrggbbaa` as (r, g, b, a)."""
    if not isinstance(value, str) or not COLOR.match(value):
        raise ManifestError(f"{key}: {value!r} isn't a #rrggbb colour")
    v = value[1:]
    if len(v) == 6:
        v += "ff"
    return tuple(int(v[i : i + 2], 16) for i in (0, 2, 4, 6))


def android_qualifier(lang):
    """The resource qualifier for a BCP 47 tag: `fr`, `pt-rBR`, `b+zh+Hant`."""
    parts = lang.split("-")
    if len(parts) == 1:
        return parts[0]
    if len(parts) == 2 and len(parts[1]) == 2:
        return f"{parts[0]}-r{parts[1]}"
    return "b+" + "+".join(parts)


def ios_lproj(lang):
    """The `.lproj` name for a BCP 47 tag (`pt-BR`, `zh-Hans`: as written)."""
    return lang


class Manifest:
    """The checked manifest. Paths are absolute."""

    def __init__(self, data, base, package):
        self.base = Path(base)
        self.package = package
        d = dict(data)
        ios = d.pop("ios", {}) or {}
        android = d.pop("android", {}) or {}
        localized = d.pop("localized", {}) or {}

        self.id = self._req(d, "id")
        if not APP_ID.match(self.id):
            raise ManifestError(
                f"id: {self.id!r} isn't a reverse-DNS identifier (com.example.app)"
            )
        self.name = d.pop("name", None) or package.get("name") or self.id
        self.version = str(d.pop("version", None) or package.get("version") or "1.0")
        build = d.pop("build", 1)
        if os.environ.get("STIPPLE_BUILD_NUMBER"):
            build = os.environ["STIPPLE_BUILD_NUMBER"]
        try:
            self.build = int(build)
        except (TypeError, ValueError):
            raise ManifestError(f"build: {build!r} isn't a whole number")
        if not 1 <= self.build <= 2100000000:
            raise ManifestError("build: must be between 1 and 2100000000")
        self.icon = self._path(d.pop("icon", None), "icon")
        self.icon_background = parse_color(
            d.pop("icon-background", "#ffffff"), "icon-background"
        )
        self.assets = self._path(d.pop("assets", None), "assets", must_exist=False)
        orientations = d.pop("orientations", ["all"])
        self.orientations = []
        for o in orientations:
            if o not in ORIENTATIONS:
                raise ManifestError(
                    f"orientations: {o!r} isn't one of {', '.join(ORIENTATIONS)}"
                )
            for x in ORIENTATIONS[o]:
                if x not in self.orientations:
                    self.orientations.append(x)
        self.languages = list(d.pop("languages", []))
        for lang in self.languages:
            if not LANG.match(lang):
                raise ManifestError(
                    f"languages: {lang!r} isn't a language tag (en, pt-BR, zh-Hans)"
                )
        self.localized = {}
        for lang, strings in localized.items():
            if not LANG.match(lang):
                raise ManifestError(f"localized.{lang}: not a language tag")
            if lang not in self.languages:
                self.languages.append(lang)
            self.localized[lang] = dict(strings)
        self._ios(ios)
        self._android(android)
        if d:
            raise ManifestError(f"unknown keys: {', '.join(sorted(d))}")

    # --- Helpers -----------------------------------------------------------

    def _req(self, d, key):
        if key not in d:
            raise ManifestError(f"{key} is required")
        return d.pop(key)

    def _path(self, value, key, must_exist=True):
        if value is None:
            return None
        p = (self.base / value).resolve()
        if must_exist and not p.exists():
            raise ManifestError(f"{key}: {p} doesn't exist")
        return p

    # --- iOS -----------------------------------------------------------------

    def _ios(self, d):
        d = dict(d)
        self.ios_bundle_id = d.pop("bundle-id", self.id)
        if not IOS_ID.match(self.ios_bundle_id):
            raise ManifestError(f"ios.bundle-id: {self.ios_bundle_id!r}")
        self.ios_minimum = str(d.pop("minimum-os", "15.0"))
        family = d.pop("device-family", ["iphone"])
        self.ios_family = []
        for f in family:
            if f not in ("iphone", "ipad"):
                raise ManifestError(f"ios.device-family: {f!r} (iphone or ipad)")
            self.ios_family.append(1 if f == "iphone" else 2)
        self.ios_ipad_orientations = None
        if "ipad-orientations" in d:
            self.ios_ipad_orientations = []
            for o in d.pop("ipad-orientations"):
                if o not in ORIENTATIONS:
                    raise ManifestError(f"ios.ipad-orientations: {o!r}")
                self.ios_ipad_orientations += [
                    x for x in ORIENTATIONS[o] if x not in self.ios_ipad_orientations
                ]
        self.ios_usage = usage_keys(d.pop("usage", {}), "ios.usage")
        self.ios_encryption = bool(d.pop("uses-encryption", False))
        self.ios_multiple_scenes = bool(d.pop("multiple-scenes", False))
        self.ios_entitlements = self._path(d.pop("entitlements", None), "ios.entitlements")
        self.ios_info = dict(d.pop("info", {}))
        self.ios_privacy = {}
        for cat, reasons in dict(d.pop("privacy-api", {})).items():
            if not cat.startswith("NSPrivacyAccessedAPICategory"):
                cat = "NSPrivacyAccessedAPICategory" + cat
            self.ios_privacy[cat] = list(reasons)
        self.ios_privacy_manifest = self._path(
            d.pop("privacy-manifest", None), "ios.privacy-manifest"
        )
        if d:
            raise ManifestError(f"unknown keys in ios: {', '.join(sorted(d))}")

    # --- Android -------------------------------------------------------------

    def _android(self, d):
        d = dict(d)
        self.android_package = d.pop("package", self.id)
        if not APP_ID.match(self.android_package):
            raise ManifestError(f"android.package: {self.android_package!r}")
        self.android_min_sdk = int(d.pop("min-sdk", 24))
        self.android_target_sdk = int(d.pop("target-sdk", ANDROID_TARGET_SDK))
        if self.android_min_sdk < 21:
            raise ManifestError("android.min-sdk: Stipple needs 21 or later")
        self.android_abis = list(d.pop("abis", ["arm64-v8a", "armeabi-v7a", "x86_64"]))
        for abi in self.android_abis:
            if abi not in ANDROID_ABIS:
                raise ManifestError(
                    f"android.abis: {abi!r} isn't one of {', '.join(ANDROID_ABIS)}"
                )
        self.android_lib_name = d.pop("lib-name", None)
        self.android_permissions = []
        for p in d.pop("permissions", []):
            if "." not in p:
                p = "android.permission." + p.upper()
            self.android_permissions.append(p)
        self.android_alarms = bool(d.pop("scheduled-notifications", False))
        # The Play Billing Library for platform::store: true (the default
        # version tools/android/billing.py packages) or a version.
        billing = d.pop("billing", False)
        if billing is True:
            billing = BILLING_VERSION
        if billing and not isinstance(billing, str):
            raise ManifestError("android.billing: true or a version (\"8.3.0\")")
        self.android_billing = billing or None
        self.android_theme = d.pop(
            "theme", "@android:style/Theme.DeviceDefault.Light.NoActionBar"
        )
        splash = d.pop("splash-background", None)
        self.android_splash = parse_color(splash, "android.splash-background") if splash else None
        self.android_icon_scale = float(d.pop("icon-scale", 72 / 108))
        if not 0.1 <= self.android_icon_scale <= 1.0:
            raise ManifestError("android.icon-scale: between 0.1 and 1")
        self.android_icon_foreground = self._path(
            d.pop("icon-foreground", None), "android.icon-foreground"
        ) or self.icon
        self.android_icon_monochrome = self._path(
            d.pop("icon-monochrome", None), "android.icon-monochrome"
        )
        self.android_manifest = self._path(d.pop("manifest", None), "android.manifest")
        self.android_debuggable = bool(d.pop("debuggable", False))
        if d:
            raise ManifestError(f"unknown keys in android: {', '.join(sorted(d))}")

    def name_in(self, lang):
        return self.localized.get(lang, {}).get("name")


def usage_keys(usage, where):
    out = {}
    for k, v in dict(usage).items():
        if k.startswith("NS"):
            out[k] = v
        elif k in IOS_USAGE:
            out[IOS_USAGE[k]] = v
        else:
            raise ManifestError(
                f"{where}: {k!r} isn't one of {', '.join(IOS_USAGE)} (or an NS… key)"
            )
    return out


# --- Loading ---------------------------------------------------------------


def cargo_metadata(manifest_path, deps=False):
    cmd = ["cargo", "metadata", "--format-version", "1", "--manifest-path", str(manifest_path)]
    if not deps:
        cmd.append("--no-deps")
    out = subprocess.run(cmd, check=True, capture_output=True, text=True).stdout
    return json.loads(out)


def load(manifest_path, config=None):
    """The app's manifest: `config` if given, else its Cargo.toml's
    `[package.metadata.stipple]`. Returns (Manifest, cargo metadata, package)."""
    manifest_path = Path(manifest_path).resolve()
    meta = cargo_metadata(manifest_path)
    package = next(
        (p for p in meta["packages"] if Path(p["manifest_path"]) == manifest_path),
        None,
    )
    if package is None:
        raise ManifestError(f"{manifest_path} isn't a package (a virtual workspace?)")
    if config:
        config = Path(config).resolve()
        with open(config, "rb") as f:
            data = tomllib.load(f)
        base = config.parent
    else:
        data = (package.get("metadata") or {}).get("stipple")
        if data is None:
            raise ManifestError(
                f"{manifest_path} has no [package.metadata.stipple] (or pass --config)"
            )
        base = manifest_path.parent
    return Manifest(data, base, package), meta, package
