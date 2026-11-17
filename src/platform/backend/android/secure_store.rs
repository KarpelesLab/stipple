//! [`platform::secure_store`](crate::platform::secure_store) on Android: what
//! `EncryptedSharedPreferences` did, over JNI. One AES-256-GCM key,
//! `stipple.secure_store`, lives in the `AndroidKeyStore` (StrongBox-backed
//! on a device with a StrongBox, API 28 and later, else the TEE's); it never
//! leaves the Keystore. Each value is sealed with it into a file of its own
//! under `getNoBackupFilesDir()`, which neither Auto Backup nor a device
//! transfer copies (the file format: `platform::secure_store::sealed`).
//!
//! Needs Android 6.0 (API 23: `KeyGenParameterSpec`). The calls work from
//! any thread, also in a process no activity started in (a service), once
//! there is an application context (`platform::android::application_context`).

use super::jni::{self, JObject, JValue, Jni};
use crate::platform::secure_store::sealed;
use crate::platform::secure_store::{SecureStoreError, Target};
use std::ffi::c_void;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

/// The key's alias in the `AndroidKeyStore`.
const ALIAS: &str = "stipple.secure_store";
const PURPOSE_ENCRYPT_DECRYPT: i32 = 1 | 2;
const ENCRYPT_MODE: i32 = 1;
const DECRYPT_MODE: i32 = 2;
const TAG_BITS: i32 = (sealed::TAG_LEN * 8) as i32;

// `JNINativeInterface` indices for byte arrays (jni.rs has the others).
const GET_ARRAY_LENGTH: usize = 171;
const NEW_BYTE_ARRAY: usize = 176;
const GET_BYTE_ARRAY_REGION: usize = 200;
const SET_BYTE_ARRAY_REGION: usize = 208;

impl Jni {
    /// A `byte[]` holding `bytes`.
    fn byte_array(&self, bytes: &[u8]) -> Option<JObject> {
        unsafe {
            let new: unsafe extern "C" fn(jni::Env, i32) -> JObject =
                std::mem::transmute(self.f(NEW_BYTE_ARRAY));
            let array = self.ok(new(self.0, bytes.len() as i32))?;
            let set: unsafe extern "C" fn(jni::Env, JObject, i32, i32, *const u8) =
                std::mem::transmute(self.f(SET_BYTE_ARRAY_REGION));
            set(self.0, array, 0, bytes.len() as i32, bytes.as_ptr());
            (!self.clear()).then_some(array)
        }
    }

    /// A `byte[]`'s bytes.
    fn bytes(&self, array: JObject) -> Option<Vec<u8>> {
        if array.is_null() {
            return None;
        }
        unsafe {
            let len: unsafe extern "C" fn(jni::Env, JObject) -> i32 =
                std::mem::transmute(self.f(GET_ARRAY_LENGTH));
            let n = len(self.0, array).max(0) as usize;
            let mut out = vec![0u8; n];
            let get: unsafe extern "C" fn(jni::Env, JObject, i32, i32, *mut u8) =
                std::mem::transmute(self.f(GET_BYTE_ARRAY_REGION));
            get(self.0, array, 0, n as i32, out.as_mut_ptr());
            (!self.clear()).then_some(out)
        }
    }
}

/// What the store keeps: the VM, the no-backup directory, and (once made or
/// found) the Keystore key, as a global reference.
struct Java {
    vm: *mut c_void,
    sdk: i32,
    no_backup: PathBuf,
    /// `context.getPackageManager().hasSystemFeature(FEATURE_STRONGBOX_KEYSTORE)`.
    strongbox: bool,
    key: JObject,
}

// The VM pointer and global references are valid on every thread.
unsafe impl Send for Java {}

/// The store's state; held through each call, which also keeps two threads
/// from making the key, or writing one file, at once.
static JAVA: Mutex<Option<Java>> = Mutex::new(None);

fn unavailable(what: &str) -> SecureStoreError {
    SecureStoreError::Unavailable(what.into())
}

/// Run `f` with the store's state, set up the first time.
fn with_java<R>(
    f: impl FnOnce(&mut Java) -> Result<R, SecureStoreError>,
) -> Result<R, SecureStoreError> {
    let mut guard = JAVA.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        *guard = Some(setup()?);
    }
    f(guard.as_mut().expect("set up above"))
}

fn setup() -> Result<Java, SecureStoreError> {
    let vm = super::java::java_vm();
    if vm.is_null() {
        return Err(unavailable("no Java VM yet"));
    }
    let context = super::java::application_context()
        .ok_or_else(|| unavailable("no application context yet"))?;
    let java = unsafe {
        jni::with_env(vm, |jni| {
            let sdk = jni.static_int(c"android/os/Build$VERSION", c"SDK_INT")?;
            if sdk < 23 {
                return Some(Err(unavailable(
                    "the Android Keystore's AES keys need Android 6.0 (API 23)",
                )));
            }
            let dir = jni.call_object(context, c"getNoBackupFilesDir", c"()Ljava/io/File;", &[])?;
            let path = jni.call_object(dir, c"getAbsolutePath", c"()Ljava/lang/String;", &[])?;
            let strongbox = sdk >= 28
                && jni
                    .call_object(
                        context,
                        c"getPackageManager",
                        c"()Landroid/content/pm/PackageManager;",
                        &[],
                    )
                    .and_then(|pm| {
                        let feature = jni.string("android.hardware.strongbox_keystore")?;
                        jni.call_bool(
                            pm,
                            c"hasSystemFeature",
                            c"(Ljava/lang/String;)Z",
                            &[JValue { l: feature }],
                        )
                    })
                    .unwrap_or(false);
            Some(Ok(Java {
                vm,
                sdk,
                no_backup: PathBuf::from(jni.to_rust16(path)?),
                strongbox,
                key: std::ptr::null_mut(),
            }))
        })
    };
    java.unwrap_or_else(|| Err(unavailable("getNoBackupFilesDir failed")))
}

impl Java {
    /// The Keystore key, found or made (and kept as a global reference).
    fn key(&mut self) -> Result<JObject, SecureStoreError> {
        if !self.key.is_null() {
            return Ok(self.key);
        }
        let (strongbox, sdk) = (self.strongbox, self.sdk);
        let key = unsafe {
            jni::with_env(self.vm, |jni| {
                let key = find_key(jni)
                    .or_else(|| {
                        // StrongBox can still refuse the key
                        // (StrongBoxUnavailableException): the TEE then.
                        let made = (strongbox && sdk >= 28)
                            .then(|| make_key(jni, true))
                            .flatten();
                        if strongbox && made.is_none() {
                            super::log("stipple: secure store: no StrongBox key, using the TEE");
                        }
                        made.or_else(|| make_key(jni, false))
                    })
                    .map(|k| jni.global(k))?;
                if strongbox {
                    super::log("stipple: secure store: key ready (StrongBox device)");
                }
                Some(key)
            })
        };
        let key = key
            .filter(|k| !k.is_null())
            .ok_or_else(|| SecureStoreError::Os("the Android Keystore made no key".into()))?;
        self.key = key;
        Ok(key)
    }

    fn file(&self, target: &Target, key: &str) -> PathBuf {
        self.no_backup
            .join(sealed::ROOT)
            .join(sealed::dir_name(&target.service))
            .join(sealed::file_name(key))
    }
}

/// The key under [`ALIAS`] in the `AndroidKeyStore`, if it is there.
fn find_key(jni: &Jni) -> Option<JObject> {
    let name = jni.string("AndroidKeyStore")?;
    let ks = jni.call_static_object(
        c"java/security/KeyStore",
        c"getInstance",
        c"(Ljava/lang/String;)Ljava/security/KeyStore;",
        &[JValue { l: name }],
    )?;
    jni.call_void(
        ks,
        c"load",
        c"(Ljava/security/KeyStore$LoadStoreParameter;)V",
        &[JValue {
            l: std::ptr::null_mut(),
        }],
    )?;
    let alias = jni.string(ALIAS)?;
    jni.call_object(
        ks,
        c"getKey",
        c"(Ljava/lang/String;[C)Ljava/security/Key;",
        &[
            JValue { l: alias },
            JValue {
                l: std::ptr::null_mut(),
            },
        ],
    )
}

/// Make the AES-256-GCM key (in StrongBox if `strongbox`): randomized
/// encryption required, no user authentication.
fn make_key(jni: &Jni, strongbox: bool) -> Option<JObject> {
    const BUILDER: &std::ffi::CStr = c"(I)Landroid/security/keystore/KeyGenParameterSpec$Builder;";
    const STRINGS: &std::ffi::CStr =
        c"([Ljava/lang/String;)Landroid/security/keystore/KeyGenParameterSpec$Builder;";
    let alias = jni.string(ALIAS)?;
    let b = jni.new_named(
        c"android/security/keystore/KeyGenParameterSpec$Builder",
        c"(Ljava/lang/String;I)V",
        &[
            JValue { l: alias },
            JValue {
                i: PURPOSE_ENCRYPT_DECRYPT,
            },
        ],
    )?;
    let gcm = jni.string_array(&["GCM"])?;
    jni.call_object(b, c"setBlockModes", STRINGS, &[JValue { l: gcm }])?;
    let none = jni.string_array(&["NoPadding"])?;
    jni.call_object(b, c"setEncryptionPaddings", STRINGS, &[JValue { l: none }])?;
    jni.call_object(b, c"setKeySize", BUILDER, &[JValue { i: 256 }])?;
    if strongbox {
        jni.call_object(
            b,
            c"setIsStrongBoxBacked",
            c"(Z)Landroid/security/keystore/KeyGenParameterSpec$Builder;",
            &[JValue { z: 1 }],
        )?;
    }
    let spec = jni.call_object(
        b,
        c"build",
        c"()Landroid/security/keystore/KeyGenParameterSpec;",
        &[],
    )?;
    let (aes, provider) = (jni.string("AES")?, jni.string("AndroidKeyStore")?);
    let kg = jni.call_static_object(
        c"javax/crypto/KeyGenerator",
        c"getInstance",
        c"(Ljava/lang/String;Ljava/lang/String;)Ljavax/crypto/KeyGenerator;",
        &[JValue { l: aes }, JValue { l: provider }],
    )?;
    jni.call_void(
        kg,
        c"init",
        c"(Ljava/security/spec/AlgorithmParameterSpec;)V",
        &[JValue { l: spec }],
    )?;
    jni.call_object(kg, c"generateKey", c"()Ljavax/crypto/SecretKey;", &[])
}

/// A new `AES/GCM/NoPadding` cipher.
fn cipher(jni: &Jni) -> Option<JObject> {
    let name = jni.string("AES/GCM/NoPadding")?;
    jni.call_static_object(
        c"javax/crypto/Cipher",
        c"getInstance",
        c"(Ljava/lang/String;)Ljavax/crypto/Cipher;",
        &[JValue { l: name }],
    )
}

/// `value` sealed with `key`: the Keystore's IV and the ciphertext.
fn seal(jni: &Jni, key: JObject, aad: &[u8], value: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let c = cipher(jni)?;
    jni.call_void(
        c,
        c"init",
        c"(ILjava/security/Key;)V",
        &[JValue { i: ENCRYPT_MODE }, JValue { l: key }],
    )?;
    let aad = jni.byte_array(aad)?;
    jni.call_void(c, c"updateAAD", c"([B)V", &[JValue { l: aad }])?;
    let input = jni.byte_array(value)?;
    let sealed = jni.call_object(c, c"doFinal", c"([B)[B", &[JValue { l: input }])?;
    let iv = jni.call_object(c, c"getIV", c"()[B", &[])?;
    Some((jni.bytes(iv)?, jni.bytes(sealed)?))
}

/// The value sealed in `ciphertext`; `None` if it doesn't open (another
/// key's, or changed).
fn open(jni: &Jni, key: JObject, aad: &[u8], iv: &[u8], ciphertext: &[u8]) -> Option<Vec<u8>> {
    let c = cipher(jni)?;
    let iv = jni.byte_array(iv)?;
    let spec = jni.new_named(
        c"javax/crypto/spec/GCMParameterSpec",
        c"(I[B)V",
        &[JValue { i: TAG_BITS }, JValue { l: iv }],
    )?;
    jni.call_void(
        c,
        c"init",
        c"(ILjava/security/Key;Ljava/security/spec/AlgorithmParameterSpec;)V",
        &[
            JValue { i: DECRYPT_MODE },
            JValue { l: key },
            JValue { l: spec },
        ],
    )?;
    let aad = jni.byte_array(aad)?;
    jni.call_void(c, c"updateAAD", c"([B)V", &[JValue { l: aad }])?;
    let input = jni.byte_array(ciphertext)?;
    let plain = jni.call_object(c, c"doFinal", c"([B)[B", &[JValue { l: input }])?;
    jni.bytes(plain)
}

fn io(what: &str, path: &std::path::Path, e: std::io::Error) -> SecureStoreError {
    SecureStoreError::Os(format!("{what} {}: {e}", path.display()))
}

pub(crate) fn get(target: &Target, key: &str) -> Result<Option<Vec<u8>>, SecureStoreError> {
    with_java(|java| {
        let path = java.file(target, key);
        let file = match std::fs::read(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io("can't read", &path, e)),
        };
        let (iv, ciphertext) = sealed::decode(&file)?;
        let k = java.key()?;
        let aad = sealed::aad(&target.service, key);
        unsafe { jni::with_env(java.vm, |jni| open(jni, k, &aad, iv, ciphertext)) }
            .map(Some)
            .ok_or(SecureStoreError::Corrupt)
    })
}

pub(crate) fn set(target: &Target, key: &str, value: &[u8]) -> Result<(), SecureStoreError> {
    with_java(|java| {
        let k = java.key()?;
        let aad = sealed::aad(&target.service, key);
        let (iv, ciphertext) =
            unsafe { jni::with_env(java.vm, |jni| seal(jni, k, &aad, value)) }
                .ok_or_else(|| SecureStoreError::Os("Cipher.doFinal failed to encrypt".into()))?;
        let path = java.file(target, key);
        let dir = path.parent().expect("a file in a directory");
        std::fs::create_dir_all(dir).map_err(|e| io("can't create", dir, e))?;
        // A whole file or none: written beside, synced, renamed over.
        let tmp = path.with_extension("tmp");
        let mut f = std::fs::File::create(&tmp).map_err(|e| io("can't create", &tmp, e))?;
        f.write_all(&sealed::encode(&iv, &ciphertext))
            .and_then(|()| f.sync_all())
            .map_err(|e| io("can't write", &tmp, e))?;
        std::fs::rename(&tmp, &path).map_err(|e| io("can't replace", &path, e))
    })
}

pub(crate) fn remove(target: &Target, key: &str) -> Result<(), SecureStoreError> {
    with_java(|java| {
        let path = java.file(target, key);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io("can't remove", &path, e)),
        }
    })
}
