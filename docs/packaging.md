# Packaging for the App Store and Google Play

`cargo-stipple` ([`tools/package`](../tools/package)) turns a Stipple app
into what the stores take: a signed `.ipa` for App Store Connect (and a
`.app` for the simulator), and an Android App Bundle (`.aab`) for Google Play
with a universal APK to install for testing. It reads a manifest in the
app's `Cargo.toml`, builds the app with Cargo, draws the icons from one
image, writes the platform files (`Info.plist`, `AndroidManifest.xml`, the
resources), and signs with keys it finds in environment variables. There is
no Xcode project and no Gradle.

It is a Python script (3.11 or later, standard library only) plus a small
Rust helper it builds against the app's own Stipple, which draws the icons
with Stipple's image code and writes the Android `classes.dex`. Nothing is
added to the app's dependencies.

```sh
# Run it from the app's directory, or put tools/package on PATH and use
# `cargo stipple ...` (Cargo runs cargo-stipple as a subcommand).
path/to/stipple/tools/package/cargo-stipple check     # read the manifest
path/to/stipple/tools/package/cargo-stipple android   # .aab + universal APK
path/to/stipple/tools/package/cargo-stipple ios --simulator
path/to/stipple/tools/package/cargo-stipple ios       # .app + .ipa
```

The results go to `target/stipple-package/<platform>/` (or `--out DIR`).
`--build N` sets the build number (or `STIPPLE_BUILD_NUMBER`, for CI),
`--features`, `--profile` and `-v` (print each command) do what they say.
Stipple's own examples have their manifests in separate files, given with
`--config`: [`examples/androiddemo/package.toml`](../examples/androiddemo/package.toml)
and [`examples/touchdemo/package.toml`](../examples/touchdemo/package.toml).

## The app's crate

One crate serves both platforms: a binary for iOS and a `cdylib` for Android
(see [Android's entry point](platforms/android.md#the-entry-point)).

```toml
[package]
name = "qr-master"
version = "2.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib", "rlib"]   # the Android library (and the code main.rs uses)

[[bin]]
name = "qr-master"                # the iOS executable
path = "src/main.rs"

[dependencies]
stipple = "0.0"

# Debug symbols for crash reports: file and line in the dSYM, at little cost.
[profile.release]
debug = "line-tables-only"
```

`cargo-stipple android` builds the `[lib]` (or `--example NAME`), `ios` the
binary (`--bin NAME` when there are several, or `--example NAME`).

## The manifest

```toml
[package.metadata.stipple]
id = "com.example.qrmaster"        # the bundle id and the Android package
name = "QR Master"                 # the name under the icon
# version = "2.1.0"                # default: the package's version
build = 42                         # versionCode / CFBundleVersion, raised for every upload
icon = "icons/app.svg"             # one square image: SVG, or PNG/JPEG/WebP at 1024x1024
icon-background = "#16181d"        # behind the icon (iOS icons are opaque)
assets = "assets"                  # the files platform::assets() reads
orientations = ["portrait"]        # portrait, portrait-upside-down, landscape(-left/-right), all
languages = ["en", "fr", "ja"]     # the first is the development language
# url-schemes = ["qrmaster"]       # qrmaster://… opens the app (App::on_open_url)
# associated-domains = ["qrmaster.example"]   # its https links too: universal links, App Links

[package.metadata.stipple.localized.fr]
name = "QR Maître"
usage = { camera = "Pour lire les codes QR." }

[package.metadata.stipple.ios]
minimum-os = "15.0"
device-family = ["iphone", "ipad"]
usage = { camera = "To scan QR codes.", photo-library = "To read a code from a picture." }
# uses-encryption = false          # ITSAppUsesNonExemptEncryption (HTTPS alone is exempt)
# entitlements = "ios/app.entitlements"
# privacy-api = { UserDefaults = ["CA92.1"] }   # added to Stipple's own
# info = { UIStatusBarHidden = true }           # any other Info.plist keys

[package.metadata.stipple.android]
permissions = ["CAMERA", "POST_NOTIFICATIONS"]
# min-sdk = 24
# target-sdk = 35                  # Play's current requirement (the default)
# abis = ["arm64-v8a", "armeabi-v7a", "x86_64"]
# lib-name = "qr_master"           # android.app.lib_name: default, the [lib]'s name
# scheduled-notifications = true   # platform::schedule_notification's receiver and dex
# drag-files = true                # the FileProvider files dragged out of the app go through
# billing = true                   # the Play Billing Library for platform::store (or a version: "8.3.0")
# splash-background = "#16181d"    # Android 12's splash screen
# icon-foreground = "icons/fg.svg" # the adaptive icon's layer (default: icon)
# icon-monochrome = "icons/mono.svg"   # Android 13's themed icons
# icon-scale = 0.667               # how much of the 108 dp layer the image covers
```

`cargo-stipple check` prints what it understood; unknown keys are errors.

**Permissions.** iOS asks for each one with the sentence in `usage` (short
names: `camera`, `microphone`, `photo-library`, `photo-library-add`,
`location`, `location-always`, `contacts`, `calendars`, `reminders`,
`bluetooth`, `motion`, `face-id`, `speech`, `local-network`, `tracking`,
`health-share`, `health-update`; any `NS…UsageDescription` key as it is). An
app that touches the camera without one is killed. Android's are
`android.permission.` names (`CAMERA` is short for
`android.permission.CAMERA`).

**Languages.** Each `localized.<lang>` gives the name (and the iOS prompts)
in that language: `<lang>.lproj/InfoPlist.strings` on iOS,
`values-<lang>/strings.xml` on Android. With more than one language, iOS
lists `CFBundleLocalizations` and Android 13 a `locales_config`, so both
offer a per-app language in the system settings, which
`platform::preferred_languages()` then reports (see
[localization.md](localization.md)). Language tags are BCP 47 (`pt-BR`,
`zh-Hans`).

**Orientations.** The phone's allowed orientations: `UISupportedInterfaceOrientations`
on iOS and `android:screenOrientation` on Android. An iPad gets every
orientation (iPad multitasking requires it) unless `ios.ipad-orientations`
says otherwise, which makes the app full screen. The app can narrow them
further at run time with `App::orientations`.

## URLs that open the app

`url-schemes` and `associated-domains` claim the URLs `App::on_open_url`
hears (see [os-integration.md](os-integration.md#urls-that-open-the-app)):

- **`url-schemes`**: custom schemes, lowercase (`myapp`, `com.example.app`
  for the reverse-DNS redirect URIs OAuth providers like). iOS gets a
  `CFBundleURLTypes` entry; Android an intent filter (`VIEW`, `DEFAULT`,
  `BROWSABLE`, the schemes) on Stipple's `Link` activity, with the
  `classes.dex` it lives in. Any app can claim any scheme, so a scheme
  proves nothing about who sends the URL: for a sign-in, use PKCE.
- **`associated-domains`** (or its other name, `app-links`): https domains
  whose links open the app, verified by a file the domain serves.
  `example.com`, `*.example.com` (its subdomains), `example.com/auth` (on
  Android, only links under that path; iOS takes the paths from the
  server's file), `example.com?mode=developer` (iOS: fetched from the server
  itself, on a device in developer mode, rather than Apple's CDN). iOS gets
  the `com.apple.developer.associated-domains` entitlement (`applinks:`),
  which the provisioning profile must allow (the app id's Associated Domains
  capability; signing warns when it doesn't); Android an `android:autoVerify`
  https intent filter per domain on the `Link` activity. The domain serves
  `/.well-known/apple-app-site-association` and
  `/.well-known/assetlinks.json`: their contents are in
  [os-integration.md](os-integration.md#urls-that-open-the-app).

```toml
[package.metadata.stipple]
url-schemes = ["myapp"]
associated-domains = ["example.com", "*.example.com"]
```


## Icons

One square image makes every icon. SVG is drawn at each size; a raster
image is scaled down (give it at 1024×1024). Leave the background
transparent and set `icon-background`:

- **iOS**: the image over the background, at 1024×1024, in an asset catalog
  that Xcode's `actool` compiles (`Assets.car`, and the `CFBundleIcons` keys);
  iOS rounds the corners. Without Xcode the tool writes loose PNGs instead,
  which the simulator and ad hoc installs show but App Store Connect
  rejects.
- **Android**: an adaptive icon (Android 8 and later): the background colour
  and the image as the foreground layer, at `icon-scale` of the 108 dp layer
  (two thirds by default: the 72 dp the launcher shows; it then masks a
  circle or a squircle out of it, so keep what matters in the middle), in
  every density; plus the image over the background as the legacy icon.
  Android 12 shows it on the splash screen too.

## iOS

Needs a Mac, with Xcode for the icon catalog and the toolchain keys App
Store Connect checks, and the Rust targets:

```sh
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
```

**The simulator.** `cargo-stipple ios --simulator` builds for
`aarch64-apple-ios-sim` and signs ad hoc:

```sh
xcrun simctl install booted "target/stipple-package/ios-sim/QR Master.app"
xcrun simctl launch booted com.example.qrmaster
```

**A device and the App Store.** `cargo-stipple ios` builds for
`aarch64-apple-ios`, writes the dSYM next to the `.app`, signs, and zips the
`.ipa` (`Payload/QR Master.app`). It signs with:

| Variable | |
|---|---|
| `STIPPLE_IOS_SIGN_IDENTITY` | the certificate: `"Apple Distribution: Example Inc (TEAMID1234)"` for the App Store, `"Apple Development: …"` for your devices, or its SHA-1 (`security find-identity -v -p codesigning`) |
| `STIPPLE_IOS_PROVISIONING_PROFILE` | the `.mobileprovision` for the app id (App Store profile for the store; development or ad hoc for devices) |
| `STIPPLE_IOS_KEYCHAIN` | optional: the keychain holding the identity (CI) |

The profile is copied into the bundle as `embedded.mobileprovision`, and its
entitlements (the app id, made specific when the profile's is a wildcard;
the team; `get-task-allow` for development) are signed in, with
`ios.entitlements` on top for push, iCloud, associated domains and the
like. Without an identity the bundle is signed ad hoc: fine for the
simulator, refused by devices and the store.

What the bundle holds:

- `Info.plist`: the id, the version (`CFBundleShortVersionString`, up to three
  numbers) and build, the display name, `UILaunchScreen` (an empty one: the
  app's background colour until the first frame, and no letterboxing),
  `UIApplicationSceneManifest`, the device families and orientations, the
  usage descriptions, `ITSAppUsesNonExemptEncryption`, the minimum iOS, and
  the `DT…` keys naming the SDK and Xcode (App Store Connect rejects a build
  without them). `plutil -lint` checks it.
- `PrivacyInfo.xcprivacy`, the privacy manifest App Store Connect checks
  for required-reason APIs. Stipple's own are declared: file timestamps
  (`C617.1`, reading the bundle's assets and the app's files) and the system
  boot time (`35F9.1`, the monotonic clock behind animations and timers).
  Add the app's with `ios.privacy-api`, or give a whole file with
  `ios.privacy-manifest`. No tracking and no collected data are declared:
  if the app collects any, say so in a file of your own.
- The icon, the `.lproj` directories, and `assets/`.

**Uploading.** Use Apple's Transporter app (drag the `.ipa` in), or from a
terminal with an [App Store Connect API key](https://developer.apple.com/documentation/appstoreconnectapi/creating-api-keys-for-app-store-connect-api):

```sh
xcrun altool --validate-app -f "QR Master-2.1.0.ipa" -t ios \
    --apiKey KEYID --apiIssuer ISSUER-UUID
xcrun altool --upload-app -f "QR Master-2.1.0.ipa" -t ios \
    --apiKey KEYID --apiIssuer ISSUER-UUID
```

(The key file goes in `~/.appstoreconnect/private_keys/AuthKey_KEYID.p8`.)
The build then shows in App Store Connect under TestFlight once processed;
the app's record (name, description, screenshots, privacy answers, age
rating) is filled in there. For crash reports with file and line, upload the
`.dSYM` too (Xcode's Organizer, or your crash reporter's tool); it holds what
the release profile kept (`debug = "line-tables-only"`).

## Android

Needs the Android SDK (`ANDROID_HOME`, with `build-tools` 35 or later for
`aapt2`, `zipalign -P` and `apksigner`, and `platforms;android-35`), an NDK
(`ANDROID_NDK_HOME`, or the newest under `$ANDROID_HOME/ndk`), a JDK
(`keytool`, `jarsigner`), and the Rust targets:

```sh
sdkmanager "platforms;android-35" "build-tools;35.0.0" "ndk;28.2.13676358"
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
```

`cargo-stipple android` builds the cdylib for every ABI with the NDK's clang
(and its `ar` and `cc` for build scripts), linked for 16 KB pages, which
Play requires for 64-bit libraries on Android 15 (the tool checks each
library's ELF segments). It writes the manifest (the `NativeActivity` with
`android.app.lib_name`, the `configChanges` and `NoActionBar` theme Stipple
needs, predictive back, the permissions, for scheduled notifications
the `Alarm` receiver with its `classes.dex`, and with `drag-files` the
`FileProvider` files dragged out of the app go through (the same dex), and
for `url-schemes` and `associated-domains` the `Link` activity and its intent
filters (the same dex again); see
[android.md](platforms/android.md#packaging)), adds the Play Billing Library
with `billing` (its dex files and manifest entries, from
`tools/android/billing.py`; see [purchases.md](purchases.md)), compiles the resources with
`aapt2`, and links them twice:

- **The bundle** (`QR Master-2.1.0.aab`): the module `base/` with the manifest
  and resources in protocol buffer format, `lib/<abi>/`, `assets/`, `dex/`,
  and the bundle's configuration (Play splits by ABI and density but keeps
  every language in the base, so the per-app language works; images,
  sounds and other compressed assets stay uncompressed in the APKs Play
  makes). Signed with `jarsigner`. This is what Play takes.
- **The universal APK** (`QR Master-2.1.0-universal.apk`): every ABI in one,
  libraries stored uncompressed and aligned to 16 KB, signed with
  `apksigner` (v2 and v3). `adb install` it to test what you'll upload.

With [bundletool](https://github.com/google/bundletool/releases) at hand
(`BUNDLETOOL=/path/to/bundletool-all.jar`, or `bundletool` on `PATH`), the
bundle is checked with `bundletool validate`; `bundletool build-apks
--bundle app.aab --output app.apks --connected-device` then installs exactly
what Play would send to the device in front of you.

**Signing.** Play signs what users download with a key it keeps (Play App
Signing); you sign uploads with an upload key:

```sh
keytool -genkeypair -keystore upload.jks -alias upload -keyalg RSA -keysize 2048 -validity 10000
```

| Variable | |
|---|---|
| `STIPPLE_ANDROID_KEYSTORE` | the keystore file |
| `STIPPLE_ANDROID_KEYSTORE_PASSWORD` | its password |
| `STIPPLE_ANDROID_KEY_ALIAS` | the key (optional when the keystore holds only one) |
| `STIPPLE_ANDROID_KEY_PASSWORD` | optional: the key's password, when not the keystore's |

Without them the tool signs with a debug key it makes in
`target/stipple-package/work/`: fine for testing, and Play refuses it. Keep
the upload key safe: losing it means asking Play's support to reset it.

**Uploading.** In the [Play Console](https://play.google.com/console),
create the app, then under Testing (internal testing first) or Production
create a release and upload the `.aab`. Every upload needs a higher `build`
(the `versionCode`). The store listing (descriptions, screenshots, the 512
px icon, the feature graphic), the content rating, the data safety form and
the target audience are filled in there. The Play Developer API
(`edits.bundles.upload`) uploads from CI with a service account.

## In CI

The `package-ios` and `package-android` jobs in
[`.github/workflows/visual.yml`](../.github/workflows/visual.yml) package
`touchdemo` and `androiddemo` on every push: the simulator `.app` is
installed and launched (it reports the name, version and build the tool
wrote), the device `.ipa` is checked (signature, icon catalog, plists, the
`Payload/` layout), and the bundle is validated and split by bundletool and
the APK read back by `aapt2`, `apksigner` and `zipalign`. To sign for real
in your own CI, decode the keystore or certificate and profile from secrets
into files, and set the variables above. On macOS, import the `.p12` into a
temporary keychain:

```sh
security create-keychain -p "" build.keychain
security import cert.p12 -k build.keychain -P "$P12_PASSWORD" -T /usr/bin/codesign
security set-key-partition-list -S apple-tool:,apple: -s -k "" build.keychain
security list-keychains -s build.keychain login.keychain
export STIPPLE_IOS_KEYCHAIN=build.keychain
```

## Not yet

- Play Asset Delivery (asset packs) and iOS on-demand resources: all assets
  go into the base module and the bundle.
- App extensions (widgets, share extensions), App Clips and Instant Apps.
- Uploading itself: the tool stops at the signed files.
- macOS (`.app`/`.pkg` for the Mac App Store, notarization), Windows (MSIX)
  and Linux (Flatpak, AppImage) packages.
