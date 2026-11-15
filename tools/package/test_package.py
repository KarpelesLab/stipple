"""Tests for cargo-stipple's pure parts (the manifest, the Android manifest
and bundle config, the Info.plist): `python3 -m unittest discover tools/package`."""

import plistlib
import struct
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import android  # noqa: E402
import ios  # noqa: E402
from manifest import ANDROID_TARGET_SDK, Manifest, ManifestError, android_qualifier  # noqa: E402

PACKAGE = {"name": "my-app", "version": "0.3.1"}


def manifest(**kw):
    data = {"id": "com.example.app"}
    data.update(kw)
    return Manifest(data, Path("."), PACKAGE)


class ManifestTests(unittest.TestCase):
    def test_defaults(self):
        m = manifest()
        self.assertEqual(m.name, "my-app")
        self.assertEqual(m.version, "0.3.1")
        self.assertEqual(m.build, 1)
        self.assertEqual(len(m.orientations), 4)
        self.assertEqual(m.android_target_sdk, ANDROID_TARGET_SDK)
        self.assertEqual(m.android_abis, ["arm64-v8a", "armeabi-v7a", "x86_64"])
        self.assertEqual(m.ios_family, [1])

    def test_rejects(self):
        for bad in (
            {"id": "noDots"},
            {"bogus": 1},
            {"build": 0},
            {"orientations": ["sideways"]},
            {"icon-background": "white"},
            {"languages": ["english"]},
            {"android": {"abis": ["mips"]}},
            {"android": {"min-sdk": 19}},
            {"ios": {"usage": {"telepathy": "x"}}},
            {"ios": {"nope": 1}},
        ):
            with self.subTest(bad=bad), self.assertRaises(ManifestError):
                data = {"id": "com.example.app"}
                data.update(bad)
                Manifest(data, Path("."), PACKAGE)

    def test_permissions_and_usage(self):
        m = manifest(
            android={"permissions": ["camera", "com.example.permission.X"]},
            ios={"usage": {"camera": "To scan codes.", "NSFooUsageDescription": "x"}},
        )
        self.assertEqual(
            m.android_permissions, ["android.permission.CAMERA", "com.example.permission.X"]
        )
        self.assertEqual(m.ios_usage["NSCameraUsageDescription"], "To scan codes.")
        self.assertIn("NSFooUsageDescription", m.ios_usage)

    def test_localized_adds_languages(self):
        m = manifest(languages=["en"], localized={"pt-BR": {"name": "Meu App"}})
        self.assertEqual(m.languages, ["en", "pt-BR"])
        self.assertEqual(m.name_in("pt-BR"), "Meu App")

    def test_qualifiers(self):
        self.assertEqual(android_qualifier("fr"), "fr")
        self.assertEqual(android_qualifier("pt-BR"), "pt-rBR")
        self.assertEqual(android_qualifier("zh-Hant"), "b+zh+Hant")
        self.assertEqual(android_qualifier("es-419"), "b+es+419")


class AndroidTests(unittest.TestCase):
    def test_orientation(self):
        so = android.screen_orientation
        self.assertIsNone(so(["portrait", "landscape-left"]))
        self.assertEqual(so(["portrait"]), "portrait")
        self.assertEqual(so(["portrait", "portrait-upside-down"]), "sensorPortrait")
        self.assertEqual(so(["landscape-left", "landscape-right"]), "sensorLandscape")

    def test_manifest_xml(self):
        m = manifest(
            name="Café 'App'",
            build=42,
            orientations=["portrait"],
            android={"permissions": ["CAMERA"], "scheduled-notifications": True},
        )
        xml = android.manifest_xml(m, "my_app", {"label": "@string/app_name", "theme": "@style/AppTheme"})
        self.assertIn('android:versionCode="42"', xml)
        self.assertIn('android:versionName="0.3.1"', xml)
        self.assertIn('android:targetSdkVersion="35"', xml)
        self.assertIn('android:value="my_app"', xml)
        self.assertIn("android.permission.CAMERA", xml)
        self.assertIn("org.stipple.android.Alarm", xml)
        self.assertIn('android:hasCode="true"', xml)
        self.assertIn('android:screenOrientation="portrait"', xml)
        self.assertNotIn("debuggable", xml)

    def test_string_escapes(self):
        self.assertEqual(android._string("It's <b>"), "It\\'s &lt;b&gt;")
        self.assertEqual(android._string("@home"), "\\@home")

    def test_protobuf(self):
        # NativeLibraries { directory { path: "lib/x86_64" targeting { abi { alias: X86_64 } } } }
        self.assertEqual(
            android.native_config(["x86_64"]),
            b"\x0a\x12\x0a\x0alib/x86_64\x12\x04\x0a\x02\x08\x05",
        )
        cfg = android.bundle_config(["assets/**.png"])
        v = android.BUNDLETOOL_VERSION.encode()
        self.assertTrue(cfg.startswith(bytes([0x0A, len(v) + 2, 0x12, len(v)]) + v))
        self.assertIn(b"\x12\x08\x0a\x06\x0a\x04\x08\x03\x10\x01", cfg)
        self.assertTrue(cfg.endswith(b"\x1a\x0f\x0a\x0dassets/**.png"))

    def test_elf_alignment(self):
        # A 64-bit ELF header and one PT_LOAD with 16 KB alignment.
        header = bytearray(64)
        header[:6] = b"\x7fELF\x02\x01"
        struct.pack_into("<Q", header, 0x20, 64)
        struct.pack_into("<HH", header, 0x36, 56, 1)
        ph = bytearray(56)
        struct.pack_into("<I", ph, 0, 1)
        struct.pack_into("<Q", ph, 48, 16384)
        with tempfile.NamedTemporaryFile(suffix=".so") as f:
            f.write(header + ph)
            f.flush()
            self.assertEqual(android.elf_load_alignment(f.name), (16384, 64))


class IosTests(unittest.TestCase):
    def test_info_plist(self):
        m = manifest(
            name="My App",
            build=9,
            orientations=["portrait"],
            languages=["en", "fr"],
            ios={"device-family": ["iphone", "ipad"], "usage": {"camera": "Scan."},
                 "info": {"UIStatusBarHidden": True}},
        )
        info = ios.info_plist(m, "my-app", simulator=False)
        self.assertEqual(info["CFBundleIdentifier"], "com.example.app")
        self.assertEqual(info["CFBundleVersion"], "9")
        self.assertEqual(info["CFBundleShortVersionString"], "0.3.1")
        self.assertEqual(info["UIDeviceFamily"], [1, 2])
        self.assertEqual(info["UISupportedInterfaceOrientations"], ["UIInterfaceOrientationPortrait"])
        self.assertEqual(len(info["UISupportedInterfaceOrientations~ipad"]), 4)
        self.assertNotIn("UIRequiresFullScreen", info)
        self.assertEqual(info["CFBundleLocalizations"], ["en", "fr"])
        self.assertEqual(info["NSCameraUsageDescription"], "Scan.")
        self.assertIs(info["UIStatusBarHidden"], True)
        self.assertEqual(info["UILaunchScreen"], {})
        plistlib.dumps(info)

    def test_short_version(self):
        self.assertEqual(ios._short_version("1.2.3-beta.1"), "1.2.3")
        self.assertEqual(ios._short_version("2.0.1.7"), "2.0.1")
        with self.assertRaises(Exception):
            ios._short_version("one")

    def test_privacy_manifest(self):
        m = manifest(ios={"privacy-api": {"UserDefaults": ["CA92.1"]}})
        apis = {a["NSPrivacyAccessedAPIType"]: a["NSPrivacyAccessedAPITypeReasons"]
                for a in ios.privacy_manifest(m)["NSPrivacyAccessedAPITypes"]}
        self.assertEqual(apis["NSPrivacyAccessedAPICategoryUserDefaults"], ["CA92.1"])
        self.assertIn("NSPrivacyAccessedAPICategoryFileTimestamp", apis)

    def test_entitlements(self):
        m = manifest()
        profile = {
            "TeamIdentifier": ["ABCDE12345"],
            "Entitlements": {
                "application-identifier": "ABCDE12345.*",
                "keychain-access-groups": ["ABCDE12345.*"],
                "get-task-allow": False,
            },
        }
        ent = ios.entitlements(m, profile)
        self.assertEqual(ent["application-identifier"], "ABCDE12345.com.example.app")
        self.assertEqual(ent["keychain-access-groups"], ["ABCDE12345.com.example.app"])
        profile["Entitlements"]["application-identifier"] = "ABCDE12345.com.other"
        with self.assertRaises(Exception):
            ios.entitlements(m, profile)

    def test_strings(self):
        self.assertEqual(ios._strings_file({"A": 'say "hi"'}), '"A" = "say \\"hi\\"";\n')


if __name__ == "__main__":
    unittest.main()
