//! The OS's secure storage from a terminal: `platform::secure_store`, which
//! keeps small secrets in the Keychain (macOS), Credential Manager
//! (Windows) or the Secret Service (Linux: GNOME Keyring, KeePassXC).
//!
//! ```text
//! securestore set KEY VALUE     store VALUE (UTF-8) under KEY
//! securestore get KEY           print it (or "none")
//! securestore remove KEY        remove it
//! securestore selftest          store, read back, replace, remove and check
//! ```
//!
//! Items go under the service `org.stipple.securestore`, so the OS's own
//! tools find them: `secret-tool lookup service org.stipple.securestore
//! account KEY` on Linux, `security find-generic-password -s
//! org.stipple.securestore -a KEY -w` on macOS (an unbundled binary uses
//! the login keychain), `cmdkey /list:org.stipple.securestore/*` on
//! Windows. With no Secret Service on Linux each command reports that
//! instead of writing a file. `STIPPLE_DEBUG_SECURE_STORE=1` says which
//! store a backend chose.

use std::process::ExitCode;
use stipple::platform::{self, AppInfo, SecureStoreError};

const SERVICE: &str = "org.stipple.securestore";

fn main() -> ExitCode {
    platform::set_app_info(AppInfo::new(SERVICE).name("Secure store demo"));
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let store = platform::secure_store();
    let result = match args.as_slice() {
        ["set", key, value] => store.set(key, value.as_bytes()).map(|()| "stored".into()),
        ["get", key] => store.get(key).map(|v| match v {
            Some(v) => String::from_utf8_lossy(&v).into_owned(),
            None => "none".into(),
        }),
        ["remove", key] => store.remove(key).map(|()| "removed".into()),
        ["selftest"] => return selftest(),
        _ => {
            eprintln!("usage: securestore set KEY VALUE | get KEY | remove KEY | selftest");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(out) => {
            println!("{out}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("securestore: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Store a value, read it back, replace it, remove it, each step logged;
/// what CI runs.
fn selftest() -> ExitCode {
    let store = platform::secure_store();
    let key = format!("selftest-{}", std::process::id());
    // Every byte value, as a private key would have.
    let value: Vec<u8> = (0..=255u8).rev().collect();
    let steps = || -> Result<(), String> {
        let e = |what: &str, e: SecureStoreError| format!("{what}: {e} ({e:?})");
        let before = store.get(&key).map_err(|x| e("get before set", x))?;
        println!("securestore: get before set -> {before:?}");
        if before.is_some() {
            return Err("a value before set".into());
        }
        store.set(&key, &value).map_err(|x| e("set", x))?;
        println!("securestore: set {} bytes -> Ok", value.len());
        let back = store.get(&key).map_err(|x| e("get", x))?;
        println!(
            "securestore: get -> {} bytes, same={}",
            back.as_ref().map_or(0, Vec::len),
            back.as_deref() == Some(&value[..])
        );
        if back.as_deref() != Some(&value[..]) {
            return Err("read back a different value".into());
        }
        store
            .set(&key, b"replaced")
            .map_err(|x| e("set again", x))?;
        let back = store.get(&key).map_err(|x| e("get again", x))?;
        println!(
            "securestore: replaced -> {:?}",
            back.as_deref().map(String::from_utf8_lossy)
        );
        if back.as_deref() != Some(&b"replaced"[..]) {
            return Err("the value wasn't replaced".into());
        }
        store.remove(&key).map_err(|x| e("remove", x))?;
        let after = store.get(&key).map_err(|x| e("get after remove", x))?;
        println!("securestore: removed, get -> {after:?}");
        if after.is_some() {
            return Err("still there after remove".into());
        }
        store.remove(&key).map_err(|x| e("remove again", x))?;
        let too_big = vec![0u8; platform::SecureStore::MAX_VALUE_LEN + 1];
        println!(
            "securestore: oversized set -> {:?}",
            store.set(&key, &too_big).err()
        );
        Ok(())
    };
    match steps() {
        Ok(()) => {
            println!("securestore: selftest passed");
            ExitCode::SUCCESS
        }
        Err(why) => {
            // Leave nothing behind.
            let _ = store.remove(&key);
            println!("securestore: selftest FAILED: {why}");
            ExitCode::FAILURE
        }
    }
}
