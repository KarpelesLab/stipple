//! The Android store's files: one per key, in a directory per service under
//! `getNoBackupFilesDir()/stipple-secure-store`, each holding the value
//! sealed with AES-GCM by the Keystore's key.
//!
//! A file is `SSv1`, the IV's length (one byte), the IV (the Keystore makes
//! it: an app can't choose one for a key that requires randomized
//! encryption), then the ciphertext with its 16-byte tag at the end, as
//! `Cipher.doFinal` returns it. The service and the key are the
//! authenticated data, so a file copied under another key's name doesn't
//! decrypt.

use super::SecureStoreError;

/// A file's first four bytes.
const MAGIC: &[u8; 4] = b"SSv1";
/// GCM's tag, at the end of the ciphertext.
pub(crate) const TAG_LEN: usize = 16;

/// The directory under `getNoBackupFilesDir()` that holds the services'.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) const ROOT: &str = "stipple-secure-store";

/// `bytes` as lowercase hex: names that are safe file names whatever the
/// service or key holds (a key of [`MAX_KEY_LEN`](super::SecureStore::MAX_KEY_LEN)
/// bytes makes 240 characters, under the 255 a file name may have).
pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The service's directory name.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn dir_name(service: &str) -> String {
    // A service can be longer than a key; past 120 bytes the name would
    // pass 240 characters, so long ones are cut (still unique in practice:
    // reverse-DNS ids are short).
    let bytes = service.as_bytes();
    hex(&bytes[..bytes.len().min(120)])
}

/// The key's file name.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn file_name(key: &str) -> String {
    format!("{}.sec", hex(key.as_bytes()))
}

/// The authenticated data that binds a sealed value to its service and key.
pub(crate) fn aad(service: &str, key: &str) -> Vec<u8> {
    let mut out = b"stipple-secure-store\0".to_vec();
    out.extend_from_slice(service.as_bytes());
    out.push(0);
    out.extend_from_slice(key.as_bytes());
    out
}

/// A file's bytes, from the IV and the ciphertext (tag included).
pub(crate) fn encode(iv: &[u8], ciphertext: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + iv.len() + ciphertext.len());
    out.extend_from_slice(MAGIC);
    out.push(iv.len() as u8);
    out.extend_from_slice(iv);
    out.extend_from_slice(ciphertext);
    out
}

/// A file's IV and ciphertext, or [`SecureStoreError::Corrupt`] for bytes
/// that aren't one of these files.
pub(crate) fn decode(file: &[u8]) -> Result<(&[u8], &[u8]), SecureStoreError> {
    let rest = file.strip_prefix(MAGIC).ok_or(SecureStoreError::Corrupt)?;
    let (&iv_len, rest) = rest.split_first().ok_or(SecureStoreError::Corrupt)?;
    let iv_len = iv_len as usize;
    // GCM's IV is 12 bytes; allow up to 16 in case a Keystore picks more.
    if !(12..=16).contains(&iv_len) || rest.len() < iv_len + TAG_LEN {
        return Err(SecureStoreError::Corrupt);
    }
    Ok(rest.split_at(iv_len))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_round_trip() {
        let iv = [7u8; 12];
        let ct = [0xAAu8; 40];
        let file = encode(&iv, &ct);
        assert_eq!(&file[..5], b"SSv1\x0c");
        assert_eq!(file.len(), 5 + 12 + 40);
        let (i, c) = decode(&file).unwrap();
        assert_eq!((i, c), (&iv[..], &ct[..]));
        // An empty value still has its tag.
        let file = encode(&iv, &[1u8; TAG_LEN]);
        assert_eq!(decode(&file).unwrap().1.len(), TAG_LEN);
    }

    #[test]
    fn other_bytes_are_corrupt() {
        let good = encode(&[1u8; 12], &[2u8; 20]);
        assert!(decode(&good).is_ok());
        for bad in [
            &b""[..],
            b"SSv1",
            b"SSv2\x0c",
            &good[..good.len() - 5], // shorter than its tag
            b"plain text secret",
        ] {
            assert_eq!(decode(bad), Err(SecureStoreError::Corrupt), "{bad:?}");
        }
        let mut long_iv = encode(&[0u8; 17], &[0u8; 16]);
        assert_eq!(decode(&long_iv), Err(SecureStoreError::Corrupt));
        long_iv[4] = 8;
        assert_eq!(decode(&long_iv), Err(SecureStoreError::Corrupt));
    }

    #[test]
    fn names_are_hex_and_bounded() {
        assert_eq!(file_name("a/b"), "612f62.sec");
        assert_eq!(dir_name("org.x"), "6f72672e78");
        let key = "k".repeat(crate::platform::SecureStore::MAX_KEY_LEN);
        assert!(file_name(&key).len() <= 255);
        assert_eq!(dir_name(&"s".repeat(400)).len(), 240);
        // The service and the key are both in the authenticated data, apart.
        assert_ne!(aad("ab", "c"), aad("a", "bc"));
        assert!(aad("svc", "key").ends_with(b"svc\0key"));
    }
}
