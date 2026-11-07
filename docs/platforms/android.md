# Android

Apps run in a `NativeActivity`, so no Java code is needed. The app is a
`cdylib` that Android loads from the APK.

## Building

The app's crate builds as a `cdylib`, under the name the manifest gives:

```toml
[lib]
name = "stipple_android"
crate-type = ["cdylib"]
```

Point Cargo at the NDK's linker for each target:

```sh
rustup target add aarch64-linux-android x86_64-linux-android
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android21-clang"
cargo build --release --target aarch64-linux-android
```

See [`examples/androiddemo`](../../examples/androiddemo) for the entry point.

## Packaging

The APK needs no Gradle. With the SDK's build tools:

```xml
<?xml version="1.0" encoding="utf-8"?>
<manifest xmlns:android="http://schemas.android.com/apk/res/android" package="com.example.yourapp">
  <uses-sdk android:minSdkVersion="24" android:targetSdkVersion="34"/>
  <application android:label="Your App" android:hasCode="false">
    <activity android:name="android.app.NativeActivity" android:exported="true">
      <meta-data android:name="android.app.lib_name" android:value="stipple_android"/>
      <intent-filter>
        <action android:name="android.intent.action.MAIN"/>
        <category android:name="android.intent.category.LAUNCHER"/>
      </intent-filter>
    </activity>
  </application>
</manifest>
```

```sh
mkdir -p lib/arm64-v8a
cp target/aarch64-linux-android/release/libstipple_android.so lib/arm64-v8a/
aapt package -f -M AndroidManifest.xml -I "$ANDROID_HOME/platforms/android-34/android.jar" -F base.apk
aapt add base.apk lib/arm64-v8a/libstipple_android.so
zipalign -f -p 4 base.apk aligned.apk
apksigner sign --ks your.keystore --out yourapp.apk aligned.apk
```

`lib_name` must match the library's name, without its `lib` prefix and `.so`
suffix. The ABI directory is `arm64-v8a` for `aarch64` and `x86_64` for the
emulator.

## Known gaps

Android is the least complete platform: it renders, but takes no touch or
keyboard input, runs at 1x, and has no OS-drawn controls. `Theme::native()` gives
the light theme there. Android's widgets live in its Java framework, so drawing
them would mean going through JNI.
