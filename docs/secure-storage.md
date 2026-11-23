# Secure storage

[`platform::secure_store`] keeps small secrets (an account token, a private
key) where the OS keeps passwords, instead of in a file under
[`platform::dirs`](os-integration.md#app-directories-and-bundle-information),
which only the app sandbox protects and which backups copy.

```rust
use stipple::platform::{secure_store, SecureStoreError};

fn save_token(token: &[u8]) -> Result<(), SecureStoreError> {
    secure_store().set("account-token", token)
}

fn load_token() -> Result<Option<Vec<u8>>, SecureStoreError> {
    secure_store().get("account-token")
}

fn sign_out() -> Result<(), SecureStoreError> {
    secure_store().remove("account-token")
}
```

- `get(key)` is the value, or `None` when there is none.
- `set(key, bytes)` stores it, replacing what was there.
- `remove(key)` removes it; removing nothing succeeds.

A key is 1 to 120 bytes of UTF-8 with no control characters; a value is at
most 2560 bytes (`SecureStore::MAX_KEY_LEN`, `SecureStore::MAX_VALUE_LEN`),
everywhere, so what works on one platform works on all. That is room for
keys and tokens, not documents: encrypt a large file with a key kept here
instead.

The calls block. Most take a few milliseconds, but the first one on Android
may make the Keystore key (longer in a StrongBox), and on Linux a locked
keyring asks the user for its password before answering. Call them at
startup or from a worker thread ([`AppHandle`](async.md)), not on every
frame.

## Platforms

| | Where | Encrypted by | Backups, other devices |
|---|---|---|---|
| iOS | Keychain generic passwords | the device's data protection | `ThisDeviceOnly`: never leaves the device |
| macOS | the data protection keychain (a signed app), else the login keychain | the keychain | the data protection keychain's items stay on the Mac |
| Android | a file per key in `getNoBackupFilesDir()`, AES-256-GCM | a key in the `AndroidKeyStore` (StrongBox when there is one) | not backed up, not transferred |
| Windows | Credential Manager, generic credentials | DPAPI, for the user | `CRED_PERSIST_LOCAL_MACHINE`: doesn't roam |
| Linux | the Secret Service (GNOME Keyring, KeePassXC, KWallet) | the keyring | the keyring's |
| Web | — (`SecureStoreError::Unsupported`) | | |

Items are named by a **service** and the key. The service is the app's
[`app_info`](os-integration.md#app-directories-and-bundle-information) id:
the bundle identifier on iOS and macOS, the package name on Android, and
on Windows and Linux what [`set_app_info`] (or `App::app_info`) gave, else
the executable's name. Give the desktop app an id before the first call,
so renaming the executable doesn't lose its secrets. `.service(...)`
chooses another:

```rust
let store = stipple::platform::secure_store().service("com.example.vpn");
```

### iOS and macOS: the Keychain

Each item is a generic password (`kSecClassGenericPassword`): the service is
`kSecAttrService`, the key `kSecAttrAccount`. It is readable once the device
has been unlocked after it started, also by the app running in the
background, and is never restored to another device from a backup
(`kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`). The queries are
CoreFoundation dictionaries built by hand and handed to `SecItemAdd`,
`SecItemCopyMatching`, `SecItemUpdate` and `SecItemDelete`
(Security.framework).

Before the first unlock after a restart, a read fails with
`SecureStoreError::Locked`: a background task started then (a VPN
reconnecting at boot) has to wait for it.

**Sharing with an app extension.** An extension (a Packet Tunnel Provider,
a widget) has its own bundle id, so it gives the app's as the service, and
both name the same keychain access group:

```rust
let store = stipple::platform::secure_store()
    .service("com.example.vpn")
    .access_group("TEAMID1234.com.example.vpn.shared");
```

The group must be in both targets' `keychain-access-groups` entitlement
(`$(AppIdentifierPrefix)com.example.vpn.shared`, `TEAMID1234.` being the
team's prefix), or be an app group both are in (`group.com.example.vpn`,
from `com.apple.security.application-groups`). With `cargo stipple`, put
the entitlement in the file `ios.entitlements` names (see
[packaging.md](packaging.md)); the provisioning profile must allow it.
Without the entitlement, the calls fail with `SecureStoreError::Denied`
(`errSecMissingEntitlement`). Without an access group, items go to the
app's default group (its application identifier), which only the app
reads.

**macOS: two keychains.** A macOS app gets iOS's data protection keychain
(`kSecUseDataProtectionKeychain`) when it is signed with an application
identifier (a Developer ID or App Store app with a provisioning profile).
A binary run with `cargo run`, unsigned or signed ad hoc, has no such
entitlement in its signature (Stipple reads it with
`SecTaskCopyValueForEntitlement`) and uses the user's login keychain from
its first call, without an accessibility class or access group, which that
keychain doesn't have. (Deciding up front matters: the data protection
keychain refuses an unentitled write with `errSecMissingEntitlement`, but
answers an unentitled read with "not found", so a process that only reads
would otherwise miss what an earlier run stored.) A write refused with
`errSecMissingEntitlement` also switches the process to the login keychain.
So a development build works, but:

- items stored by the development build aren't there for the signed app,
  and the other way round: they're in different keychains;
- the login keychain lets the binary that stored an item read it, and asks
  the user ("… wants to use your confidential information …") when another
  one does, which a rebuilt binary is, since its signature changes;
- with an access group the fallback doesn't happen: asking for a group is
  asking for the data protection keychain, so its absence is an error.

`STIPPLE_DEBUG_SECURE_STORE=1` prints which keychain the process ended up
with.

### Android: the Keystore and no-backup files

This is what Jetpack's `EncryptedSharedPreferences` did before it was
deprecated, over JNI without a Java library. One AES-256-GCM key,
`stipple.secure_store`, is made in the `AndroidKeyStore` the first time it's
needed: in the StrongBox secure element on a device that has one
(API 28 and later, `FEATURE_STRONGBOX_KEYSTORE`), in the TEE otherwise or
when the StrongBox refuses it. The key can't be read out of the Keystore,
only used by the app. Each value is encrypted with it (the Keystore picks
the IV; the service and the key are authenticated data, so a file renamed
to another key doesn't decrypt) into a file of its own under
`getNoBackupFilesDir()/stipple-secure-store/`, written to the side and
renamed over the old one. Neither Auto Backup nor a device-to-device
transfer copies that directory.

- It needs Android 6.0 (API 23), and the application context
  ([`platform::android::application_context`](platforms/android.md#calling-android-apis-yourself)):
  the calls work from any thread, in a process an activity started in or
  in one started for a service (a `VpnService`) or a receiver. Without
  them they fail with `SecureStoreError::Unavailable`.
- If the Keystore loses the key (the user resets it from the settings, the
  app's data was copied to another device), the files no longer decrypt and
  reads fail with `SecureStoreError::Corrupt`. Remove the key and set it
  again (after signing in again, typically).
- `.access_group(...)` has no meaning here and is ignored; `.service(...)`
  picks the directory.

### Windows: Credential Manager

Each value is a generic credential (`CRED_TYPE_GENERIC`) named
`<service>/<key>` (the user name is the key), written by `CredWriteW` with
`CRED_PERSIST_LOCAL_MACHINE`, so it stays on this machine rather than
roaming with a roaming profile. Windows encrypts it with DPAPI for the
user. It shows in Control Panel's Credential Manager, under Windows
Credentials, and `cmdkey /list:<service>/*` lists them.

Credential Manager rather than DPAPI over files of Stipple's own: the
encryption is the same DPAPI either way, but a credential is written
atomically, needs no file format or locking, and the user can see and
remove it. Its limit of 2560 bytes a credential is the store's
`MAX_VALUE_LEN` everywhere. A process without a loaded user profile (a
service under some accounts) gets `SecureStoreError::Unavailable`
(`ERROR_NO_SUCH_LOGON_SESSION`).

### Linux: the Secret Service

Stipple talks to `org.freedesktop.secrets` on the session bus with its own
D-Bus client (no libsecret). Any implementation works: GNOME Keyring,
KeePassXC with its Secret Service integration on, KWallet 5.97 and later.
An item carries the attributes `xdg:schema` = `org.stipple.SecureStore`,
`service` and `account` (the key), labelled `<service> (<key>)`:

```sh
secret-tool lookup service org.example.Notes account account-token
secret-tool search --all xdg:schema org.stipple.SecureStore
```

- A new item goes to the `default` collection (the login keyring); if there
  is none, the service is asked to make one, which prompts for its
  password.
- A locked item or collection is unlocked through the service's prompt
  (the keyring's password dialog, from the service, not the app). If the
  user dismisses it, or it can't show (no display), the call fails with
  `SecureStoreError::Locked`; it waits up to five minutes for an answer.
- With no Secret Service on the bus, or no session bus, the calls fail
  with `SecureStoreError::Unavailable`. Nothing is written to a plain
  file instead: an app that wants a fallback chooses it itself.
- The secret crosses the bus with the `plain` algorithm, not the
  Diffie-Hellman one. The session bus belongs to the user, and any of the
  user's processes may ask the Secret Service for the same unlocked item
  anyway, so the encrypted transfer wouldn't keep the secret from anyone
  who could read the bus.
- `.access_group(...)` is ignored.

### The web

There is no store the page could keep a secret in that the page's own
scripts (and so an injected one) couldn't read: every call fails with
`SecureStoreError::Unsupported`.

## Errors

`SecureStoreError` is `#[non_exhaustive]`:

| | When |
|---|---|
| `Unsupported` | the web |
| `Unavailable(why)` | no Secret Service, no session bus; Android before 6.0, or before the application exists; no user profile on Windows |
| `Locked` | iOS before the first unlock since boot; a keyring whose unlock prompt was dismissed |
| `Denied(why)` | a missing `keychain-access-groups` entitlement, a cancelled keychain prompt |
| `InvalidKey` | an empty key, one over 120 bytes, or with a control character |
| `TooLarge` | a value over 2560 bytes |
| `Corrupt` | an Android value whose Keystore key is gone, or whose file was changed |
| `Os(why)` | anything else, as the OS said it |

## Not yet

- Requiring the user's authentication (Face ID, Touch ID, a fingerprint) to
  read an item: `kSecAttrAccessControl` with `.userPresence` on Apple,
  `setUserAuthenticationRequired` on Android.
- Choosing the accessibility class (`WhenUnlocked`, or items that move to a
  new device through a backup).
- Listing the keys stored.

## The example and what CI checks

`examples/securestore` is a terminal tool over the API (`securestore set
KEY VALUE`, `get KEY`, `remove KEY`, and `selftest`, which stores every
byte value, reads it back, replaces it and removes it). On Android,
`androiddemo` does the same at startup and logs it, and on iOS
`touchdemo`'s self-test does.

- **Linux** (`.github/scripts/secure-store-linux.sh`, in the OS integration
  job): GNOME Keyring in a private session bus, its keyrings in a
  throwaway home, unlocked with a password on stdin. The selftest;
  `secret-tool` reading what Stipple stored and Stipple reading what
  `secret-tool` stored, each in its own process; the keyring locked
  through `Service.Lock` and no display for its prompt, which must answer
  `Locked`; a bus without a Secret Service, which must answer
  `Unavailable` and leave no file; and the crate's own round-trip test
  (`STIPPLE_SECRET_SERVICE_TEST=1 cargo test --lib platform::secure_store`).
  All of it passes locally against GNOME Keyring 48.
- **Android** (`examples/androiddemo/test-secure-store.sh`, on the API 34,
  33 and 30 emulators): a fresh install stores nothing; the round trip
  passes; the stored value's file is in `no_backup/`, starts with `SSv1`
  and doesn't hold the plain text; after `am force-stop` the value comes
  back; a byte added to the file makes it read as `Corrupt`. Passes on
  local API 34 and API 30 emulators (neither has a StrongBox).
- **Windows**: the selftest, then a value stored by one process, listed by
  `cmdkey`, read back by another and removed.
- **macOS**: the selftest of the unsigned example (the login keychain
  fallback), then `security find-generic-password` finding the item one
  process stored (its attributes: reading its secret from `security`, another
  binary, would ask the user), and another process reading it back.
- **iOS**: `touchdemo`'s self-test on the simulator logs the Keychain round
  trip, in the bundle the visual job assembles and in the one
  `cargo stipple ios --simulator` packages; both executables carry an
  application identifier and a keychain access group in their
  `__TEXT,__entitlements` section (and `__TEXT,__ents_der`), as Xcode links
  a simulator build, where the simulator reads them.

The Windows, macOS and iOS checks run on CI only. The Linux and Windows
steps pass in CI and are required. On macOS the selftest passed (the data
protection keychain refused to the unsigned binary, the login keychain
used), but the cross-process check failed in every run: a fresh unsigned
process that only read went to the data protection keychain and missed
the item in the login keychain. The keychain is now chosen up front from
the process's signature, which passed in a CI scratch run; the step stays
non-blocking until it has passed on master. The iOS simulator's round
trip passes and is required in both jobs: it failed with
`errSecMissingEntitlement` while the bundle carried no application
identifier (one signed ad hoc into its code signature was refused a
launch), and passes with the entitlements linked into the executable's
section instead. The data protection keychain of
a signed macOS app, and access groups shared with an extension, need
signing that CI doesn't do: they are unverified.

[`platform::secure_store`]: https://docs.rs/stipple/latest/stipple/platform/fn.secure_store.html
[`set_app_info`]: https://docs.rs/stipple/latest/stipple/platform/fn.set_app_info.html
