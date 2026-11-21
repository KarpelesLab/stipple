"""Tests for cargo-stipple's pure parts (the manifest, the Android manifest
and bundle config, the Info.plist): `python3 -m unittest discover tools/package`."""

import os
import plistlib
import xml.etree.ElementTree as ET
import struct
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import android  # noqa: E402
import android_code  # noqa: E402
from common import PackageError  # noqa: E402
import ios  # noqa: E402
import ios_extensions  # noqa: E402
import linux  # noqa: E402
from common import Target  # noqa: E402
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

    def test_drag_files(self):
        refs = {"label": "@string/app_name", "theme": "@style/AppTheme"}
        xml = android.manifest_xml(manifest(), "my_app", refs)
        self.assertNotIn("org.stipple.android.Files", xml)
        m = manifest(android={"drag-files": True})
        self.assertTrue(m.android_drag_files)
        xml = android.manifest_xml(m, "my_app", refs)
        self.assertIn('<provider android:name="org.stipple.android.Files"', xml)
        self.assertIn(f'android:authorities="{m.android_package}.stipple.files"', xml)
        self.assertIn('android:exported="false" android:grantUriPermissions="true"', xml)
        # The provider's class is in the dex.
        self.assertIn('android:hasCode="true"', xml)

    def test_network(self):
        refs = {"label": "@string/app_name", "theme": "@style/AppTheme"}
        xml = android.manifest_xml(manifest(), "my_app", refs)
        self.assertNotIn("ACCESS_NETWORK_STATE", xml)
        m = manifest(android={"network": True})
        self.assertTrue(m.android_network)
        xml = android.manifest_xml(m, "my_app", refs)
        self.assertIn('<uses-permission android:name="android.permission.ACCESS_NETWORK_STATE"/>', xml)
        # Not twice when it's in the permissions too.
        m = manifest(android={"network": True, "permissions": ["ACCESS_NETWORK_STATE"]})
        xml = android.manifest_xml(m, "my_app", refs)
        self.assertEqual(xml.count("ACCESS_NETWORK_STATE"), 1)

    def test_billing(self):
        self.assertIsNone(manifest().android_billing)
        self.assertEqual(manifest(android={"billing": True}).android_billing, "8.3.0")
        m = manifest(android={"billing": "8.0.0"})
        self.assertEqual(m.android_billing, "8.0.0")
        with self.assertRaises(ManifestError):
            manifest(android={"billing": 8})
        # The library's code needs hasCode, even without the receiver.
        xml = android.manifest_xml(m, "my_app", {"label": "@string/app_name", "theme": "@style/AppTheme"})
        self.assertIn('android:hasCode="true"', xml)
        self.assertNotIn("org.stipple.android.Alarm", xml)

    def test_url_schemes_and_app_links(self):
        refs = {"label": "@string/app_name", "theme": "@style/AppTheme"}
        plain = android.manifest_xml(manifest(), "my_app", refs)
        self.assertNotIn("org.stipple.android.Link", plain)
        self.assertNotIn("android.intent.action.VIEW", plain)
        self.assertIn('android:hasCode="false"', plain)
        self.assertFalse(android.needs_dex(manifest()))
        m = manifest(
            **{"url-schemes": ["myapp", "com.example.app"],
               "associated-domains": ["example.com", "*.example.org/auth", "dev.example.com?mode=developer"]}
        )
        self.assertTrue(android.needs_dex(m))
        xml = android.manifest_xml(m, "my_app", refs)
        self.assertIn('android:hasCode="true"', xml)
        self.assertIn('<activity android:name="org.stipple.android.Link" android:exported="true"', xml)
        self.assertIn('android:theme="@android:style/Theme.NoDisplay"', xml)
        # The app's own activity keeps only its launcher filter.
        main = xml.split('android:name="android.app.NativeActivity"')[1].split("</activity>")[0]
        self.assertNotIn("VIEW", main)
        link = xml.split('android:name="org.stipple.android.Link"')[1].split("</activity>")[0]
        self.assertEqual(link.count("<intent-filter"), 4, "the schemes' and one per domain")
        self.assertEqual(link.count('android:autoVerify="true"'), 3)
        self.assertEqual(link.count("android.intent.category.BROWSABLE"), 4)
        self.assertEqual(link.count("android.intent.category.DEFAULT"), 4)
        self.assertIn('<data android:scheme="myapp"/>', link)
        self.assertIn('<data android:scheme="com.example.app"/>', link)
        self.assertEqual(link.count('<data android:scheme="https"/>'), 3)
        self.assertIn('<data android:host="*.example.org"/>', link)
        self.assertIn('<data android:pathPrefix="/auth"/>', link)
        self.assertIn('<data android:host="dev.example.com"/>', link)
        self.assertNotIn("mode=", link)
        # The alias, and what's refused.
        self.assertEqual(manifest(**{"app-links": ["a.example"]}).link_domains[0]["host"], "a.example")
        for bad in (
            {"url-schemes": ["MyApp"]},
            {"url-schemes": ["https"]},
            {"url-schemes": ["my app"]},
            {"associated-domains": ["https://example.com"]},
            {"associated-domains": ["localhost"]},
            {"associated-domains": ["a.com"], "app-links": ["b.com"]},
        ):
            with self.subTest(bad=bad), self.assertRaises(ManifestError):
                manifest(**bad)

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



REFS = {"label": "@string/app_name", "theme": "@style/AppTheme"}

FRAGMENT = """<?xml version="1.0" encoding="utf-8"?>
<manifest xmlns:android="http://schemas.android.com/apk/res/android">
  <uses-permission android:name="android.permission.FOREGROUND_SERVICE"/>
  <queries><package android:name="com.example.other"/></queries>
  <application android:allowBackup="false">
    <service android:name=".Tunnel" android:exported="false"
        android:permission="android.permission.BIND_VPN_SERVICE">
      <intent-filter><action android:name="android.net.VpnService"/></intent-filter>
    </service>
    <receiver android:name="com.example.app.Boot" android:exported="true"/>
    <provider android:name=".Data" android:authorities="${applicationId}.data"/>
    <meta-data android:name="com.example.flag" android:value="on"/>
  </application>
</manifest>
"""


class AndroidCodeTests(unittest.TestCase):
    def generated(self, **android_keys):
        m = manifest(android=android_keys)
        return m, android.manifest_xml(m, "my_app", REFS)

    def test_keys(self):
        with tempfile.TemporaryDirectory() as d:
            (Path(d) / "java").mkdir()
            (Path(d) / "x.dex").write_bytes(b"dex\n035\0")
            m = Manifest(
                {"id": "com.example.app", "android": {"java": "java", "dex": ["x.dex"]}},
                Path(d), PACKAGE,
            )
            self.assertEqual(m.android_java, [(Path(d) / "java").resolve()])
            self.assertEqual(m.android_dex, [(Path(d) / "x.dex").resolve()])
            self.assertIn('android:hasCode="true"', android.manifest_xml(m, "a", REFS))
            self.assertEqual(android_code.check_dex(m.android_dex[0]), m.android_dex[0])
            (Path(d) / "y.dex").write_bytes(b"PK\3\4....")
            with self.assertRaises(PackageError):
                android_code.check_dex(Path(d) / "y.dex")
            for bad in ({"java": "nope"}, {"dex": ["nope.dex"]}, {"manifest-fragment": "nope.xml"}):
                with self.subTest(bad=bad), self.assertRaises(ManifestError):
                    Manifest({"id": "com.example.app", "android": bad}, Path(d), PACKAGE)
        self.assertIn('android:hasCode="false"', self.generated()[1])

    def test_dex_names(self):
        names = [n for n, _ in android_code.dex_names(["a", "b", "c"])]
        self.assertEqual(names, ["classes.dex", "classes2.dex", "classes3.dex"])

    def test_merge(self):
        m, xml = self.generated(**{"scheduled-notifications": True, "drag-files": True})
        out, components = android_code.merge_fragment(xml, FRAGMENT, m.android_package)
        root = ET.fromstring(out)
        A = android_code.A
        app = root.find("application")
        self.assertEqual(app.get(A + "allowBackup"), "false")
        # Stipple's entries are all still there, the fragment's after them.
        names = [c.get(A + "name") for c in app]
        self.assertEqual(
            names,
            ["android.app.NativeActivity", "org.stipple.android.Alarm", "org.stipple.android.Files",
             "com.example.app.Tunnel", "com.example.app.Boot", "com.example.app.Data",
             "com.example.flag"],
        )
        self.assertEqual(len(components), 3)
        authorities = [p.get(A + "authorities") for p in app.iter("provider")]
        self.assertIn("com.example.app.data", authorities)
        perms = [p.get(A + "name") for p in root.iter("uses-permission")]
        self.assertIn("android.permission.FOREGROUND_SERVICE", perms)
        self.assertIn("android.permission.RECEIVE_BOOT_COMPLETED", perms)
        self.assertIsNotNone(root.find("queries"))
        # Top-level entries come before <application>.
        self.assertLess(out.index("FOREGROUND_SERVICE"), out.index("<application"))
        service = app.find("service")
        self.assertEqual(service.find("intent-filter/action").get(A + "name"), "android.net.VpnService")
        self.assertNotIn("xmlns:android", out[out.index("<application"):])

    def test_conflicts(self):
        m, xml = self.generated(**{"scheduled-notifications": True, "drag-files": True,
                                   "permissions": ["CAMERA"]})
        pkg = m.android_package

        def frag(body):
            return ('<manifest xmlns:android="http://schemas.android.com/apk/res/android">'
                    + body + "</manifest>")

        for body in (
            # Already generated: a permission, Stipple's receiver and activity,
            # the FileProvider's authority, an <application> attribute.
            '<uses-permission android:name="android.permission.CAMERA"/>',
            '<uses-permission android:name="android.permission.RECEIVE_BOOT_COMPLETED"/>',
            '<application><receiver android:name="org.stipple.android.Alarm"/></application>',
            '<application><activity android:name="android.app.NativeActivity"/></application>',
            '<application><provider android:name=".P" android:authorities="${applicationId}.stipple.files"/></application>',
            '<application android:hasCode="false"/>',
            # Twice in the fragment itself (".S" and "S" are the same class).
            '<application><service android:name=".S"/><receiver android:name="S"/></application>',
            '<application><meta-data android:name="x" android:value="1"/><meta-data android:name="x" android:value="2"/></application>',
            # Not mergeable.
            '<uses-sdk android:minSdkVersion="21"/>',
            '<instrumentation android:name=".T"/>',
            '<application><service/></application>',
            '<application/><application/>',
            '<application><service android:name=".S" xmlns:tools="http://schemas.android.com/tools" tools:node="remove"/></application>',
        ):
            with self.subTest(body=body), self.assertRaises(PackageError):
                android_code.merge_fragment(xml, frag(body), pkg)
        with self.assertRaises(PackageError):
            android_code.merge_fragment(xml, '<fragment/>', pkg)
        with self.assertRaises(PackageError):
            android_code.merge_fragment(xml, '<manifest package="x"/>', pkg)
        with self.assertRaises(PackageError):
            android_code.merge_fragment(xml, '<manifest><oops></manifest>', pkg)
        # The same attribute value isn't a conflict, and an empty fragment
        # changes nothing.
        out, _ = android_code.merge_fragment(xml, frag('<application android:hasCode="true"/>'), pkg)
        self.assertEqual(out, xml)


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
        self.assertIs(info["UIApplicationSupportsIndirectInputEvents"], True)
        plistlib.dumps(info)

    def test_url_types_and_associated_domains(self):
        info = ios.info_plist(manifest(), "my-app", simulator=False)
        self.assertNotIn("CFBundleURLTypes", info)
        self.assertEqual(ios.link_entitlements(manifest()), {})
        m = manifest(
            **{"url-schemes": ["myapp"],
               "associated-domains": ["example.com/auth", "dev.example.com?mode=developer"]}
        )
        info = ios.info_plist(m, "my-app", simulator=False)
        self.assertEqual(
            info["CFBundleURLTypes"],
            [{"CFBundleURLName": "com.example.app", "CFBundleTypeRole": "Viewer",
              "CFBundleURLSchemes": ["myapp"]}],
        )
        plistlib.dumps(info)
        domains = ["applinks:example.com", "applinks:dev.example.com?mode=developer"]
        self.assertEqual(
            ios.link_entitlements(m), {"com.apple.developer.associated-domains": domains}
        )
        profile = {
            "TeamIdentifier": ["ABCDE12345"],
            "Entitlements": {
                "application-identifier": "ABCDE12345.com.example.app",
                "com.apple.developer.associated-domains": "*",
            },
        }
        ent = ios.entitlements(m, profile)
        self.assertEqual(ent["com.apple.developer.associated-domains"], domains)
        plistlib.dumps(ent)

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


def with_extension(groups=("group.com.example.app",), **ext):
    e = {"name": "Tunnel", "bin": "tunnel", "point": "com.apple.networkextension.packet-tunnel",
         "principal-class": "PacketTunnelProvider"}
    e.update(ext)
    return manifest(ios={"app-groups": list(groups), "extension": [e]})


class IosExtensionTests(unittest.TestCase):
    def test_manifest(self):
        m = with_extension(**{"app-groups": True})
        self.assertEqual(m.ios_app_groups, ["group.com.example.app"])
        (e,) = m.ios_extensions
        self.assertEqual(e.bundle_id, "com.example.app.tunnel")
        self.assertEqual(e.bundle_dir, "Tunnel.appex")
        self.assertEqual(e.profile_env, "STIPPLE_IOS_PROVISIONING_PROFILE_TUNNEL")
        self.assertEqual(e.app_groups, ["group.com.example.app"])
        self.assertEqual((e.kind, e.target, e.entry), ("bin", "tunnel", "main"))
        self.assertEqual(e.minimum_os, m.ios_minimum)
        self.assertEqual(with_extension().ios_extensions[0].app_groups, [])
        e = with_extension(**{"bundle-id-suffix": "vpn.provider"}).ios_extensions[0]
        self.assertEqual(e.bundle_id, "com.example.app.vpn.provider")
        e = with_extension(**{"bundle-id": "com.example.app.x"}).ios_extensions[0]
        self.assertEqual(e.bundle_id, "com.example.app.x")
        self.assertEqual(ios_extensions.profile_env("share-ext"),
                         "STIPPLE_IOS_PROVISIONING_PROFILE_SHARE_EXT")
        self.assertEqual(manifest().ios_extensions, [])
        self.assertIn("Tunnel", repr(m.ios_extensions))

    def test_rejects(self):
        for bad in (
            {"bundle-id": "com.example.other.tunnel"},
            {"bundle-id": "com.example.application"},
            {"bundle-id": "com.example.app"},
            {"bundle-id": "com.example.app."},
            {"bundle-id": "com.example.app.x", "bundle-id-suffix": "x"},
            {"bundle-id-suffix": "a b"},
            {"point": None},
            {"example": "tunnel"},
            {"name": "Two Words"},
            {"entry": "start"},
            {"app-groups": ["group.com.example.other"]},
            {"nope": 1},
        ):
            with self.subTest(bad=bad), self.assertRaises(ManifestError):
                ext = {"name": "Tunnel", "bin": "tunnel", "point": "p", **bad}
                ext = {k: v for k, v in ext.items() if v is not None}
                manifest(ios={"app-groups": ["group.com.example.app"], "extension": [ext]})
        for groups in (["com.example.app"], ["group.x"], "group.com.example.app",
                       ["group.com.a.b", "group.com.a.b"]):
            with self.subTest(groups=groups), self.assertRaises(ManifestError):
                manifest(ios={"app-groups": groups})
        with self.assertRaises(ManifestError):
            with_extension(groups=(), **{"app-groups": True})
        two = {"name": "A", "bin": "a", "point": "p"}
        for clash in ({"name": "A"}, {"name": "B", "bundle-id-suffix": "a"}, {"name": "a"}):
            other = {"bin": "b", "point": "p", **clash}
            with self.subTest(clash=clash), self.assertRaises(ManifestError):
                manifest(ios={"extension": [two, other]})

    def test_info_plist(self):
        m = with_extension(
            attributes={"NSExtensionActivationRule": "TRUEPREDICATE"},
            info={"CFBundleDisplayName": "My Tunnel"},
        )
        m.build = 12
        info = ios_extensions.info_plist(m, m.ios_extensions[0], "tunnel", False,
                                         {"DTPlatformName": "iphoneos"})
        self.assertEqual(info["CFBundlePackageType"], "XPC!")
        self.assertEqual(info["CFBundleExecutable"], "tunnel")
        self.assertEqual(info["CFBundleIdentifier"], "com.example.app.tunnel")
        self.assertEqual(info["CFBundleVersion"], "12")
        self.assertEqual(info["CFBundleShortVersionString"], "0.3.1")
        self.assertEqual(info["CFBundleDisplayName"], "My Tunnel")
        self.assertEqual(info["CFBundleSupportedPlatforms"], ["iPhoneOS"])
        self.assertEqual(info["DTPlatformName"], "iphoneos")
        self.assertEqual(info["NSExtension"], {
            "NSExtensionPointIdentifier": "com.apple.networkextension.packet-tunnel",
            "NSExtensionPrincipalClass": "PacketTunnelProvider",
            "NSExtensionAttributes": {"NSExtensionActivationRule": "TRUEPREDICATE"},
        })
        # An app's keys stay out of an extension's.
        for key in ("LSRequiresIPhoneOS", "UILaunchScreen", "UIApplicationSceneManifest"):
            self.assertNotIn(key, info)
        m = with_extension(extension={"NSExtensionMainStoryboard": "Main"})
        m.ios_extensions[0].principal_class = None
        info = ios_extensions.info_plist(m, m.ios_extensions[0], "tunnel", True, {})
        self.assertEqual(info["NSExtension"]["NSExtensionMainStoryboard"], "Main")
        self.assertNotIn("NSExtensionPrincipalClass", info["NSExtension"])
        self.assertEqual(info["CFBundleSupportedPlatforms"], ["iPhoneSimulator"])
        plistlib.dumps(info)

    def test_entitlements(self):
        m = with_extension(**{"app-groups": True})
        key = "com.apple.security.application-groups"
        self.assertEqual(ios.entitlements(m, None), {key: ["group.com.example.app"]})
        self.assertEqual(ios.entitlements(manifest(), None), {})
        e = m.ios_extensions[0]
        profile = {
            "TeamIdentifier": ["ABCDE12345"],
            "Entitlements": {
                "application-identifier": "ABCDE12345.*",
                key: ["group.com.example.app", "group.com.example.old"],
            },
        }
        ent = ios.bundle_entitlements(e.bundle_id, profile, e.entitlements, e.app_groups)
        self.assertEqual(ent["application-identifier"], "ABCDE12345.com.example.app.tunnel")
        self.assertEqual(ent[key], ["group.com.example.app"])
        # An extension that doesn't opt in keeps none of the app's groups.
        plain = with_extension().ios_extensions[0]
        ent = ios.bundle_entitlements(plain.bundle_id, None, None, plain.app_groups)
        self.assertNotIn(key, ent)
        profile["Entitlements"]["application-identifier"] = "ABCDE12345.com.example.app"
        with self.assertRaises(Exception):
            ios.bundle_entitlements(e.bundle_id, profile, None, e.app_groups)
        check = ios_extensions.check_profile_groups
        self.assertEqual(check(["group.a.b"], {key: ["group.a.b"]}, "x"), [])
        self.assertEqual(len(check(["group.a.b"], {key: ["group.c.d"]}, "x")), 1)
        self.assertEqual(check(["group.a.b"], {}, "x"), [])

    def test_cargo_command(self):
        package = {"name": "app", "targets": [
            {"name": "app", "kind": ["bin"]}, {"name": "tunnel", "kind": ["bin"]},
            {"name": "ext", "kind": ["example"]},
        ]}

        class Args:
            manifest_path = "Cargo.toml"
            profile = "release"
            features = None

        e = with_extension().ios_extensions[0]
        cmd = ios_extensions.cargo_command(Args, Target(package, "bin", "tunnel"),
                                           "aarch64-apple-ios", e)
        self.assertEqual(cmd[:2], ["cargo", "build"])
        self.assertIn("--bin", cmd)
        self.assertNotIn("--", cmd)
        e = with_extension(entry="NSExtensionMain").ios_extensions[0]
        cmd = ios_extensions.cargo_command(Args, Target(package, "example", "ext"),
                                           "aarch64-apple-ios", e)
        self.assertEqual(cmd[:2], ["cargo", "rustc"])
        self.assertEqual(cmd[cmd.index("--example") + 1], "ext")
        tail = cmd[cmd.index("--") + 1:]
        self.assertIn("link-arg=_NSExtensionMain", tail)
        self.assertIn("link-arg=Foundation", tail)


class XcodeSigningTests(unittest.TestCase):
    def test_signing_through_xcode_needs_a_team_and_no_identity(self):
        import os
        from unittest import mock

        with mock.patch.dict(os.environ, {"STIPPLE_IOS_TEAM_ID": "ABCDE12345"}, clear=True):
            self.assertTrue(ios.xcode_signing())
        with mock.patch.dict(os.environ, {"STIPPLE_IOS_TEAM_ID": "ABCDE12345",
                                          "STIPPLE_IOS_SIGN_IDENTITY": "Apple Distribution: X"}, clear=True):
            self.assertFalse(ios.xcode_signing())
        with mock.patch.dict(os.environ, {}, clear=True):
            self.assertFalse(ios.xcode_signing())

    def test_export_options(self):
        o = ios.export_options({"STIPPLE_IOS_TEAM_ID": "ABCDE12345"})
        self.assertEqual(o["method"], "app-store-connect")
        self.assertEqual(o["signingStyle"], "automatic")
        self.assertEqual(o["teamID"], "ABCDE12345")
        self.assertEqual(o["destination"], "export")
        self.assertFalse(o["manageAppVersionAndBuildNumber"])
        up = ios.export_options({"STIPPLE_IOS_TEAM_ID": "T", "STIPPLE_IOS_UPLOAD": "1"})
        self.assertEqual(up["destination"], "upload")
        with self.assertRaises(PackageError):
            ios.export_options({"STIPPLE_IOS_TEAM_ID": "T", "STIPPLE_IOS_EXPORT_METHOD": "store"})
        with self.assertRaises(PackageError):
            ios.export_options({"STIPPLE_IOS_TEAM_ID": "T", "STIPPLE_IOS_EXPORT_METHOD": "debugging",
                                "STIPPLE_IOS_UPLOAD": "1"})

    def test_asc_key_args(self):
        self.assertEqual(ios.asc_key_args({}), [])
        with self.assertRaises(PackageError):
            ios.asc_key_args({"STIPPLE_ASC_KEY_ID": "KEY123"})
        with tempfile.TemporaryDirectory() as d:
            key = Path(d) / "AuthKey_KEY123.p8"
            key.write_text("-----BEGIN PRIVATE KEY-----\n")
            args = ios.asc_key_args({"STIPPLE_ASC_KEY_ID": "KEY123", "STIPPLE_ASC_ISSUER_ID": "ISS",
                                     "STIPPLE_ASC_KEY_PATH": str(key)})
            self.assertEqual(args[0], "-authenticationKeyPath")
            self.assertEqual(Path(args[1]), key.resolve())
            self.assertEqual(args[2:], ["-authenticationKeyID", "KEY123", "-authenticationKeyIssuerID", "ISS"])
            with self.assertRaises(PackageError):
                ios.asc_key_args({"STIPPLE_ASC_KEY_ID": "KEY123", "STIPPLE_ASC_ISSUER_ID": "ISS",
                                  "STIPPLE_ASC_KEY_PATH": str(Path(d) / "missing.p8")})

    def test_archive_layout(self):
        import plistlib

        with tempfile.TemporaryDirectory() as d:
            app = Path(d) / "My App.app"
            app.mkdir()
            (app / "my-app").write_bytes(b"\0")
            with open(app / "Info.plist", "wb") as f:
                plistlib.dump({"CFBundleIdentifier": "com.example.app",
                               "CFBundleShortVersionString": "1.2.3", "CFBundleVersion": "7"}, f)
            dsym = Path(d) / "My App.app.dSYM"
            (dsym / "Contents").mkdir(parents=True)
            archive = Path(d) / "My App.xcarchive"
            ios.write_archive(app, dsym, archive, "ABCDE12345")
            self.assertTrue((archive / "Products/Applications/My App.app/my-app").is_file())
            self.assertTrue((archive / "dSYMs/My App.app.dSYM/Contents").is_dir())
            with open(archive / "Info.plist", "rb") as f:
                info = plistlib.load(f)
            props = info["ApplicationProperties"]
            self.assertEqual(props["ApplicationPath"], "Applications/My App.app")
            self.assertEqual(props["CFBundleIdentifier"], "com.example.app")
            self.assertEqual(props["CFBundleVersion"], "7")
            self.assertEqual(props["Team"], "ABCDE12345")
            self.assertEqual(info["ArchiveVersion"], 2)


class LinuxTests(unittest.TestCase):
    def test_manifest(self):
        m = manifest()
        self.assertEqual(m.linux_prefix, "/usr")
        self.assertFalse(m.linux_dbus_activatable)
        self.assertEqual(m.linux_categories, ["Utility"])
        for bad in (
            {"prefix": "usr"},
            {"categories": ["Not a category"]},
            {"mime-types": ["text"]},
            {"actions": [{"id": "new window", "name": "x"}]},
            {"actions": [{"id": "a"}]},
            {"nope": 1},
        ):
            with self.subTest(bad=bad), self.assertRaises(ManifestError):
                manifest(linux=bad)

    def test_desktop_entry_and_service(self):
        m = manifest(
            name="My App",
            languages=["fr"],
            localized={"fr": {"name": "Mon appli"}},
            **{"url-schemes": ["myapp"]},
            linux={
                "dbus-activatable": True,
                "categories": ["Development", "IDE"],
                "comment": "Edits\nthings",
                "mime-types": ["text/plain"],
                "actions": [{"id": "new-window", "name": "New Window"}],
            },
        )
        entry = linux.desktop_entry(m, "/usr/bin/my-app")
        lines = entry.splitlines()
        self.assertEqual(lines[0], "[Desktop Entry]")
        for line in (
            "Name=My App",
            "Name[fr]=Mon appli",
            "Comment=Edits\\nthings",
            "Exec=/usr/bin/my-app %U",
            "Icon=com.example.app",
            "Categories=Development;IDE;",
            "StartupWMClass=com.example.app",
            "MimeType=x-scheme-handler/myapp;text/plain;",
            "DBusActivatable=true",
            "Actions=new-window;",
            "[Desktop Action new-window]",
            "Name=New Window",
        ):
            self.assertIn(line, lines)
        self.assertEqual(
            linux.dbus_service(m, "/usr/bin/my-app"),
            "[D-BUS Service]\nName=com.example.app\nExec=/usr/bin/my-app --gapplication-service\n",
        )
        plain = linux.desktop_entry(manifest(), "/usr/bin/x")
        self.assertNotIn("DBusActivatable", plain)
        self.assertNotIn("MimeType", plain)

    def test_exec_quoting(self):
        self.assertEqual(linux.exec_quote("/usr/bin/app"), "/usr/bin/app")
        self.assertEqual(linux.exec_quote("/opt/My App/app"), '"/opt/My App/app"')
        self.assertEqual(linux.exec_quote('/a"$b'), '"/a\\"\\$b"')
        self.assertEqual(linux.exec_quote("50%"), "50%%")

    def test_stage(self):
        with tempfile.TemporaryDirectory() as d:
            d = Path(d)
            exe = d / "built"
            exe.write_bytes(b"\x7fELF")
            m = manifest(linux={"dbus-activatable": True, "prefix": "/opt/ex"})
            files = linux.stage(m, d / "root", exe, "my-app")
            self.assertEqual(
                sorted(files),
                [
                    "opt/ex/bin/my-app",
                    "opt/ex/share/applications/com.example.app.desktop",
                    "opt/ex/share/dbus-1/services/com.example.app.service",
                ],
            )
            self.assertTrue(os.access(d / "root/opt/ex/bin/my-app", os.X_OK))
            service = (d / "root/opt/ex/share/dbus-1/services/com.example.app.service").read_text()
            self.assertIn("Exec=/opt/ex/bin/my-app --gapplication-service", service)
            without = linux.stage(manifest(), d / "root2", exe, "my-app")
            self.assertFalse(any("dbus-1" in f for f in without))


class InstalledToolTests(unittest.TestCase):
    def test_the_installed_binary_is_the_helper_only_for_its_own_release(self):
        import os
        from unittest import mock

        import common

        registry = "registry+https://github.com/rust-lang/crates.io-index"
        env = {"STIPPLE_PACKAGE_HELPER": "/bin/cargo-stipple", "STIPPLE_PACKAGE_VERSION": "0.1.0"}
        cases = [
            ({"version": "0.1.0", "source": registry}, ["/bin/cargo-stipple", "__stipple-helper"]),
            ({"version": "0.0.9", "source": registry}, None),
            ({"version": "0.1.0", "source": None}, None),  # a path dependency: maybe edited
            (None, None),
        ]
        for pkg, want in cases:
            with mock.patch.dict(os.environ, env, clear=True), \
                    mock.patch.object(common, "stipple_package", return_value=pkg):
                self.assertEqual(common.installed_helper("Cargo.toml"), want, pkg)
        with mock.patch.dict(os.environ, {}, clear=True), \
                mock.patch.object(common, "stipple_package", return_value=cases[0][0]):
            self.assertIsNone(common.installed_helper("Cargo.toml"))

    def test_the_binary_carries_every_script(self):
        # src/bin/cargo-stipple.rs embeds the scripts, and Cargo.toml's
        # `include` ships them: each module the tool imports is in both.
        root = Path(__file__).resolve().parent.parent.parent
        bin_rs = (root / "src/bin/cargo-stipple.rs").read_text()
        cargo = (root / "Cargo.toml").read_text()
        for f in sorted(Path(__file__).resolve().parent.glob("*.py")):
            if f.name.startswith("test_"):
                continue
            self.assertIn(f'"tools/package/{f.name}"', bin_rs, f.name)
        for f in ("tools/package/cargo-stipple", "tools/package/helper.rs", "tools/android/billing.py"):
            self.assertIn(f'"{f}"', bin_rs, f)
            self.assertIn(f'"{f}"', cargo, f)
        self.assertIn('"tools/package/*.py"', cargo)


if __name__ == "__main__":
    unittest.main()
