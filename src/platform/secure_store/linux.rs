//! Linux: the freedesktop Secret Service (`org.freedesktop.secrets`: GNOME
//! Keyring, KeePassXC, KWallet 5.97 and later) over the session bus,
//! through Stipple's own D-Bus client.
//!
//! An item carries the attributes `xdg:schema` = `org.stipple.SecureStore`,
//! `service` and `account` (the key), so `secret-tool lookup service <id>
//! account <key>` finds it. New items go to the `default` collection (the
//! login keyring), made through the service's prompt if there is none. A
//! locked item or collection is unlocked through the service's prompt (the
//! keyring's password dialog); a dismissed prompt is
//! [`SecureStoreError::Locked`].
//!
//! Secrets cross the bus with the `plain` algorithm, not the Diffie-Hellman
//! one: the session bus is the user's own, and any process of the user's
//! may ask the service for the same item anyway. With no Secret Service on
//! the bus the calls fail ([`SecureStoreError::Unavailable`]); nothing is
//! ever written to a plain file instead.

use super::{SecureStoreError, Target};
use crate::platform::a11y::{DBus, align, align_to, put_string, read_string, read_u32, skip_value};
use std::time::Duration;

const SECRETS: &str = "org.freedesktop.secrets";
const SERVICE_PATH: &str = "/org/freedesktop/secrets";
const SERVICE: &str = "org.freedesktop.Secret.Service";
const COLLECTION: &str = "org.freedesktop.Secret.Collection";
const ITEM: &str = "org.freedesktop.Secret.Item";
const PROMPT: &str = "org.freedesktop.Secret.Prompt";
const SCHEMA: &str = "org.stipple.SecureStore";
/// How long a call may take: activating the service included.
const CALL_TIMEOUT: Duration = Duration::from_secs(25);
/// How long a prompt (the user typing the keyring's password) may take.
const PROMPT_TIMEOUT: Duration = Duration::from_secs(300);

/// A connection to the Secret Service with an open session.
struct Secrets {
    bus: DBus,
    session: String,
}

impl Secrets {
    fn open() -> Result<Self, SecureStoreError> {
        let bus = DBus::connect_session()
            .map_err(|e| SecureStoreError::Unavailable(format!("no session bus: {e}")))?;
        bus.set_read_timeout(Some(CALL_TIMEOUT));
        let mut s = Self {
            bus,
            session: String::new(),
        };
        let mut body = Vec::new();
        put_string(&mut body, "plain");
        put_variant_str(&mut body, "");
        let reply = s.call(SERVICE_PATH, SERVICE, "OpenSession", "sv", &body)?;
        let mut off = 0;
        skip_value(&reply, &mut off, b"v", &mut 0).map_err(SecureStoreError::Os)?;
        s.session = read_string(&reply, &mut off).map_err(SecureStoreError::Os)?;
        super::debug(&format!("Secret Service session {}", s.session));
        Ok(s)
    }

    /// Call `member` on the service's object `path` and wait for its reply.
    fn call(
        &mut self,
        path: &str,
        interface: &str,
        member: &str,
        signature: &str,
        body: &[u8],
    ) -> Result<Vec<u8>, SecureStoreError> {
        let serial = self
            .bus
            .send_call(SECRETS, path, interface, member, signature, body)
            .map_err(SecureStoreError::Os)?;
        loop {
            let m = self.bus.read_message().map_err(SecureStoreError::Os)?;
            match m.mtype {
                2 if m.reply_serial == serial => return Ok(m.body),
                3 if m.reply_serial == serial => {
                    let text = read_string(&m.body, &mut 0).unwrap_or_default();
                    return Err(remote_error(member, &m.error_name, &text));
                }
                _ => continue,
            }
        }
    }

    /// Run the prompt at `path` (`/` is none) and return its result's
    /// variant (its signature, the body, the value's offset), or `dismissed`
    /// if the user dismissed it.
    fn prompt(
        &mut self,
        path: &str,
        dismissed: SecureStoreError,
    ) -> Result<Option<(String, Vec<u8>, usize)>, SecureStoreError> {
        if path == "/" {
            return Ok(None);
        }
        super::debug(&format!("prompting through {path}"));
        // Subscribe first, so the Completed signal can't be missed.
        self.bus
            .add_match(&format!(
                "type='signal',interface='{PROMPT}',member='Completed',path='{path}'"
            ))
            .map_err(SecureStoreError::Os)?;
        let mut body = Vec::new();
        put_string(&mut body, ""); // no parent window
        let serial = self
            .bus
            .send_call(SECRETS, path, PROMPT, "Prompt", "s", &body)
            .map_err(SecureStoreError::Os)?;
        self.bus.set_read_timeout(Some(PROMPT_TIMEOUT));
        let result = loop {
            let m = match self.bus.read_message() {
                Ok(m) => m,
                Err(e) => break Err(SecureStoreError::Os(format!("waiting on the prompt: {e}"))),
            };
            match m.mtype {
                3 if m.reply_serial == serial => {
                    let text = read_string(&m.body, &mut 0).unwrap_or_default();
                    break Err(remote_error("Prompt", &m.error_name, &text));
                }
                4 if m.member == "Completed" && m.path == path => break Ok(m.body),
                _ => continue,
            }
        };
        self.bus.set_read_timeout(Some(CALL_TIMEOUT));
        let body = result?;
        let (was_dismissed, sig, off) = completed(&body).map_err(SecureStoreError::Os)?;
        if was_dismissed {
            return Err(dismissed);
        }
        Ok(Some((sig, body, off)))
    }

    /// The items with our attributes for `service` and `key`: unlocked,
    /// then locked.
    fn search(
        &mut self,
        service: &str,
        key: &str,
    ) -> Result<(Vec<String>, Vec<String>), SecureStoreError> {
        let mut body = Vec::new();
        put_dict_ss(&mut body, &attributes(service, key));
        let reply = self.call(SERVICE_PATH, SERVICE, "SearchItems", "a{ss}", &body)?;
        let mut off = 0;
        let unlocked = read_paths(&reply, &mut off).map_err(SecureStoreError::Os)?;
        let locked = read_paths(&reply, &mut off).map_err(SecureStoreError::Os)?;
        Ok((unlocked, locked))
    }

    /// Unlock `objects` (items or collections), prompting if the service
    /// wants to; the ones unlocked.
    fn unlock(&mut self, objects: &[String]) -> Result<Vec<String>, SecureStoreError> {
        let mut body = Vec::new();
        put_paths(&mut body, objects);
        let reply = self.call(SERVICE_PATH, SERVICE, "Unlock", "ao", &body)?;
        let mut off = 0;
        let mut unlocked = read_paths(&reply, &mut off).map_err(SecureStoreError::Os)?;
        let prompt = read_string(&reply, &mut off).map_err(SecureStoreError::Os)?;
        if let Some((sig, body, mut off)) = self.prompt(&prompt, SecureStoreError::Locked)?
            && sig == "ao"
        {
            unlocked.extend(read_paths(&body, &mut off).map_err(SecureStoreError::Os)?);
        }
        Ok(unlocked)
    }

    /// The unlocked items for `service` and `key`, unlocking locked ones.
    fn items(&mut self, service: &str, key: &str) -> Result<Vec<String>, SecureStoreError> {
        let (mut unlocked, locked) = self.search(service, key)?;
        if !locked.is_empty() {
            let now = self.unlock(&locked)?;
            if unlocked.is_empty() && now.is_empty() {
                return Err(SecureStoreError::Locked);
            }
            unlocked.extend(now);
        }
        Ok(unlocked)
    }

    /// The default collection, made if there is none, unlocked.
    fn default_collection(&mut self) -> Result<String, SecureStoreError> {
        let mut body = Vec::new();
        put_string(&mut body, "default");
        let reply = self.call(SERVICE_PATH, SERVICE, "ReadAlias", "s", &body)?;
        let mut collection = read_string(&reply, &mut 0).map_err(SecureStoreError::Os)?;
        if collection == "/" {
            super::debug("no default collection: creating one");
            let mut body = Vec::new();
            put_dict_sv(
                &mut body,
                &[(
                    "org.freedesktop.Secret.Collection.Label",
                    Value::Str("Default keyring"),
                )],
            );
            put_string(&mut body, "default");
            let reply = self.call(SERVICE_PATH, SERVICE, "CreateCollection", "a{sv}s", &body)?;
            let mut off = 0;
            collection = read_string(&reply, &mut off).map_err(SecureStoreError::Os)?;
            let prompt = read_string(&reply, &mut off).map_err(SecureStoreError::Os)?;
            if let Some((sig, body, mut off)) = self.prompt(&prompt, SecureStoreError::Locked)?
                && sig == "o"
            {
                collection = read_string(&body, &mut off).map_err(SecureStoreError::Os)?;
            }
            if collection == "/" {
                return Err(SecureStoreError::Unavailable(
                    "the Secret Service has no default collection".into(),
                ));
            }
        }
        if !self
            .unlock(std::slice::from_ref(&collection))?
            .contains(&collection)
        {
            return Err(SecureStoreError::Locked);
        }
        Ok(collection)
    }
}

/// Map an error reply to a [`SecureStoreError`].
fn remote_error(member: &str, name: &str, text: &str) -> SecureStoreError {
    let what = format!("{member}: {name}: {text}");
    match name {
        "org.freedesktop.DBus.Error.ServiceUnknown"
        | "org.freedesktop.DBus.Error.NameHasNoOwner"
        | "org.freedesktop.DBus.Error.UnknownObject" => SecureStoreError::Unavailable(format!(
            "no Secret Service ({SECRETS}) on the session bus ({what})"
        )),
        n if n.starts_with("org.freedesktop.DBus.Error.Spawn") => {
            SecureStoreError::Unavailable(format!("the Secret Service didn't start ({what})"))
        }
        "org.freedesktop.Secret.Error.IsLocked" => SecureStoreError::Locked,
        "org.freedesktop.DBus.Error.AccessDenied" => SecureStoreError::Denied(what),
        _ => SecureStoreError::Os(what),
    }
}

/// The attributes an item for `service` and `key` carries.
fn attributes<'a>(service: &'a str, key: &'a str) -> [(&'a str, &'a str); 3] {
    [
        ("xdg:schema", SCHEMA),
        ("service", service),
        ("account", key),
    ]
}

pub(super) fn get(target: &Target, key: &str) -> Result<Option<Vec<u8>>, SecureStoreError> {
    let mut s = Secrets::open()?;
    let Some(item) = s.items(&target.service, key)?.into_iter().next() else {
        return Ok(None);
    };
    let mut body = Vec::new();
    put_string(&mut body, &s.session);
    let reply = s.call(&item, ITEM, "GetSecret", "o", &body)?;
    let (_, value) = read_secret(&reply, &mut 0).map_err(SecureStoreError::Os)?;
    Ok(Some(value))
}

pub(super) fn set(target: &Target, key: &str, value: &[u8]) -> Result<(), SecureStoreError> {
    let mut s = Secrets::open()?;
    let mut secret = Vec::new();
    put_secret(&mut secret, &s.session, value);
    // Already stored (in whichever collection): replace its secret.
    if let Some(item) = s.items(&target.service, key)?.into_iter().next() {
        s.call(&item, ITEM, "SetSecret", "(oayays)", &secret)?;
        return Ok(());
    }
    let collection = s.default_collection()?;
    let attrs = attributes(&target.service, key);
    let label = format!("{} ({key})", target.service);
    let mut body = Vec::new();
    put_dict_sv(
        &mut body,
        &[
            ("org.freedesktop.Secret.Item.Label", Value::Str(&label)),
            (
                "org.freedesktop.Secret.Item.Attributes",
                Value::Dict(&attrs),
            ),
        ],
    );
    put_secret(&mut body, &s.session, value);
    align(&mut body, 4);
    body.extend_from_slice(&1u32.to_le_bytes()); // replace
    let reply = s.call(
        &collection,
        COLLECTION,
        "CreateItem",
        "a{sv}(oayays)b",
        &body,
    )?;
    let mut off = 0;
    let item = read_string(&reply, &mut off).map_err(SecureStoreError::Os)?;
    let prompt = read_string(&reply, &mut off).map_err(SecureStoreError::Os)?;
    if item == "/" {
        s.prompt(&prompt, SecureStoreError::Locked)?;
    }
    Ok(())
}

pub(super) fn remove(target: &Target, key: &str) -> Result<(), SecureStoreError> {
    let mut s = Secrets::open()?;
    for item in s.items(&target.service, key)? {
        let reply = s.call(&item, ITEM, "Delete", "", &[])?;
        let prompt = read_string(&reply, &mut 0).map_err(SecureStoreError::Os)?;
        s.prompt(
            &prompt,
            SecureStoreError::Denied("the user dismissed the prompt".into()),
        )?;
    }
    Ok(())
}

// ---- Marshalling ---------------------------------------------------------------

/// A value in an `a{sv}` dictionary.
enum Value<'a> {
    Str(&'a str),
    Dict(&'a [(&'a str, &'a str)]),
}

/// A variant holding a string.
fn put_variant_str(buf: &mut Vec<u8>, s: &str) {
    put_signature(buf, "s");
    put_string(buf, s);
}

/// A SIGNATURE: its length in one byte, the signature, a NUL.
fn put_signature(buf: &mut Vec<u8>, sig: &str) {
    buf.push(sig.len() as u8);
    buf.extend_from_slice(sig.as_bytes());
    buf.push(0);
}

/// An array whose elements `put` writes, aligned to `elem_align`, its byte
/// length patched in after.
fn put_array(buf: &mut Vec<u8>, elem_align: usize, put: impl FnOnce(&mut Vec<u8>)) {
    align(buf, 4);
    let len_pos = buf.len();
    buf.extend_from_slice(&0u32.to_le_bytes());
    align(buf, elem_align);
    let start = buf.len();
    put(buf);
    let len = (buf.len() - start) as u32;
    buf[len_pos..len_pos + 4].copy_from_slice(&len.to_le_bytes());
}

/// An `a{ss}`.
fn put_dict_ss(buf: &mut Vec<u8>, entries: &[(&str, &str)]) {
    put_array(buf, 8, |buf| {
        for (k, v) in entries {
            align(buf, 8);
            put_string(buf, k);
            put_string(buf, v);
        }
    });
}

/// An `a{sv}` of strings and `a{ss}` dictionaries.
fn put_dict_sv(buf: &mut Vec<u8>, entries: &[(&str, Value)]) {
    put_array(buf, 8, |buf| {
        for (k, v) in entries {
            align(buf, 8);
            put_string(buf, k);
            match v {
                Value::Str(s) => put_variant_str(buf, s),
                Value::Dict(d) => {
                    put_signature(buf, "a{ss}");
                    put_dict_ss(buf, d);
                }
            }
        }
    });
}

/// An `ay`.
fn put_bytes(buf: &mut Vec<u8>, bytes: &[u8]) {
    align(buf, 4);
    buf.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    buf.extend_from_slice(bytes);
}

/// An `ao`.
fn put_paths(buf: &mut Vec<u8>, paths: &[String]) {
    put_array(buf, 4, |buf| {
        for p in paths {
            put_string(buf, p);
        }
    });
}

/// A Secret Service secret, `(oayays)`: the session, no parameters (the
/// `plain` algorithm), the value, its content type.
fn put_secret(buf: &mut Vec<u8>, session: &str, value: &[u8]) {
    align(buf, 8);
    put_string(buf, session);
    put_bytes(buf, &[]);
    put_bytes(buf, value);
    put_string(buf, "application/octet-stream");
}

/// An `ay` at `*off`.
fn read_bytes(buf: &[u8], off: &mut usize) -> Result<Vec<u8>, String> {
    let len = read_u32(buf, off)? as usize;
    let bytes = buf
        .get(*off..*off + len)
        .ok_or("truncated byte array")?
        .to_vec();
    *off += len;
    Ok(bytes)
}

/// An `ao` at `*off`.
fn read_paths(buf: &[u8], off: &mut usize) -> Result<Vec<String>, String> {
    let len = read_u32(buf, off)? as usize;
    let end = *off + len;
    if end > buf.len() {
        return Err("truncated object path array".into());
    }
    let mut out = Vec::new();
    while *off < end {
        out.push(read_string(buf, off)?);
    }
    Ok(out)
}

/// A secret `(oayays)` at `*off`: its content type and value.
fn read_secret(buf: &[u8], off: &mut usize) -> Result<(String, Vec<u8>), String> {
    align_to(off, 8);
    read_string(buf, off)?; // the session
    read_bytes(buf, off)?; // the parameters
    let value = read_bytes(buf, off)?;
    let content_type = read_string(buf, off)?;
    Ok((content_type, value))
}

/// A prompt's `Completed` body, `(bv)`: whether it was dismissed, and its
/// result's signature and offset.
fn completed(body: &[u8]) -> Result<(bool, String, usize), String> {
    let mut off = 0;
    let dismissed = read_u32(body, &mut off)? != 0;
    let len = *body.get(off).ok_or("truncated variant")? as usize;
    let sig = body
        .get(off + 1..off + 1 + len)
        .ok_or("truncated variant")?;
    let sig = String::from_utf8_lossy(sig).into_owned();
    Ok((dismissed, sig, off + 1 + len + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_is_a_struct_of_session_parameters_value_and_type() {
        let mut buf = vec![0u8; 3]; // an offset the struct must align past
        put_secret(&mut buf, "/s/1", b"key");
        assert_eq!(&buf[3..8], &[0; 5]); // padding to 8
        let mut off = 3;
        let (ty, value) = read_secret(&buf, &mut off).unwrap();
        assert_eq!(
            (ty.as_str(), value.as_slice()),
            ("application/octet-stream", &b"key"[..])
        );
        assert_eq!(off, buf.len());
        // o: len 4, "/s/1", NUL; ay (empty); ay "key"; then the type.
        assert_eq!(&buf[8..17], b"\x04\0\0\0/s/1\0");
        assert_eq!(&buf[20..24], &0u32.to_le_bytes());
        assert_eq!(&buf[24..31], b"\x03\0\0\0key");
    }

    #[test]
    fn attribute_dictionaries_marshal_as_d_bus_does() {
        let mut buf = Vec::new();
        put_dict_ss(&mut buf, &[("a", "b")]);
        // Length 14 (key "a" 6 bytes + pad 2 + value "b" 6), entry at 8.
        assert_eq!(&buf[..4], &14u32.to_le_bytes());
        assert_eq!(&buf[4..8], &[0; 4]);
        assert_eq!(&buf[8..], b"\x01\0\0\0a\0\0\0\x01\0\0\0b\0");

        let mut buf = Vec::new();
        let attrs = attributes("org.x", "k");
        put_dict_sv(
            &mut buf,
            &[("L", Value::Str("lbl")), ("A", Value::Dict(&attrs))],
        );
        let mut off = 0;
        let mut si = 0;
        skip_value(&buf, &mut off, b"a{sv}", &mut si).unwrap();
        assert_eq!(off, buf.len());
        assert!(buf.windows(5).any(|w| w == b"a{ss}"));
        assert!(buf.windows(23).any(|w| w == b"org.stipple.SecureStore"));
    }

    #[test]
    fn object_path_arrays_and_prompt_results_read_back() {
        let mut buf = Vec::new();
        let paths = vec!["/a/1".to_string(), "/a/22".to_string()];
        put_paths(&mut buf, &paths);
        put_paths(&mut buf, &[]);
        let mut off = 0;
        assert_eq!(read_paths(&buf, &mut off).unwrap(), paths);
        assert!(read_paths(&buf, &mut off).unwrap().is_empty());
        assert!(read_paths(&buf[..6], &mut 0).is_err());

        // Completed(false, <ao ["/c"]>).
        let mut body = 0u32.to_le_bytes().to_vec();
        put_signature(&mut body, "ao");
        put_paths(&mut body, &["/c".to_string()]);
        let (dismissed, sig, mut off) = completed(&body).unwrap();
        assert!(!dismissed);
        assert_eq!(sig, "ao");
        assert_eq!(read_paths(&body, &mut off).unwrap(), ["/c"]);
        let mut body = 1u32.to_le_bytes().to_vec();
        put_signature(&mut body, "s");
        put_string(&mut body, "");
        assert!(completed(&body).unwrap().0);
    }

    #[test]
    fn missing_services_are_unavailable_not_errors() {
        assert!(matches!(
            remote_error(
                "OpenSession",
                "org.freedesktop.DBus.Error.ServiceUnknown",
                "x"
            ),
            SecureStoreError::Unavailable(_)
        ));
        assert!(matches!(
            remote_error(
                "OpenSession",
                "org.freedesktop.DBus.Error.Spawn.ExecFailed",
                "x"
            ),
            SecureStoreError::Unavailable(_)
        ));
        assert_eq!(
            remote_error("GetSecret", "org.freedesktop.Secret.Error.IsLocked", ""),
            SecureStoreError::Locked
        );
        assert!(matches!(
            remote_error("GetSecret", "org.freedesktop.Secret.Error.NoSuchObject", ""),
            SecureStoreError::Os(_)
        ));
    }

    /// A real round trip, against the session bus's Secret Service: run with
    /// `STIPPLE_SECRET_SERVICE_TEST=1` inside a private D-Bus session with a
    /// keyring (CI starts gnome-keyring-daemon in `dbus-run-session`), never
    /// against the user's own keyring by accident.
    #[test]
    fn round_trip_against_the_session_secret_service() {
        if std::env::var_os("STIPPLE_SECRET_SERVICE_TEST").is_none_or(|v| v != "1") {
            return;
        }
        let store = crate::platform::secure_store().service("org.stipple.test");
        let key = format!("round-trip-{}", std::process::id());
        let value: Vec<u8> = (0..=255u8).collect();
        assert_eq!(store.get(&key), Ok(None));
        store.set(&key, &value).unwrap();
        assert_eq!(store.get(&key).unwrap(), Some(value.clone()));
        store.set(&key, b"").unwrap();
        assert_eq!(store.get(&key).unwrap(), Some(Vec::new()));
        // Another service's item of the same key is another item.
        let other = crate::platform::secure_store().service("org.stipple.other");
        assert_eq!(other.get(&key), Ok(None));
        store.remove(&key).unwrap();
        assert_eq!(store.get(&key), Ok(None));
        store.remove(&key).unwrap();
    }
}
