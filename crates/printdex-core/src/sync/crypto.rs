//! Optional encryption of a sync space with a passphrase (design §10).
//!
//! The passphrase is stretched with scrypt (parameters and salt in
//! `space.toml`, never the passphrase) into a 256-bit key. Every batch,
//! snapshot and photo is sealed with XChaCha20-Poly1305 under a fresh
//! random 192-bit nonce, stored in front of the ciphertext.

use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};

use crate::error::SyncError;

const NONCE_LEN: usize = 24;
/// What the check value in `space.toml` seals, to tell a wrong passphrase
/// from a damaged file.
const CHECK_PLAIN: &[u8] = b"printdex-sync-check";

/// A space's key, derived from its passphrase.
#[derive(Clone)]
pub struct SpaceKey([u8; 32]);

impl std::fmt::Debug for SpaceKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SpaceKey(…)")
    }
}

/// scrypt parameters of a space.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdfParams {
    /// Hex-encoded 16-byte salt.
    pub salt: String,
    pub log_n: u8,
    pub r: u32,
    pub p: u32,
}

impl KdfParams {
    /// Fresh parameters: a random salt, N = 2^15, r = 8, p = 1 (about 32 MiB
    /// and a few hundred milliseconds, once per session).
    #[expect(
        clippy::indexing_slicing,
        reason = "an XChaCha20 nonce is 24 bytes, so [..16] is in range"
    )]
    pub fn generate() -> KdfParams {
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        KdfParams {
            salt: hex(&nonce[..16]),
            log_n: 15,
            r: 8,
            p: 1,
        }
    }
}

/// Lowercase hex of `bytes`.
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Bytes of a lowercase or uppercase hex string.
pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// Derives the key for `pass`. CPU-heavy: call it off the async workers.
pub fn derive(pass: &str, params: &KdfParams) -> Result<SpaceKey, SyncError> {
    let salt = unhex(&params.salt).ok_or(SyncError::Corrupt)?;
    let p =
        scrypt::Params::new(params.log_n, params.r, params.p).map_err(|_| SyncError::Corrupt)?;
    let mut out = [0_u8; 32];
    scrypt::scrypt(pass.as_bytes(), &salt, &p, &mut out).map_err(|_| SyncError::Corrupt)?;
    Ok(SpaceKey(out))
}

/// Encrypts `plain`: nonce followed by ciphertext and tag.
pub fn seal(key: &SpaceKey, plain: &[u8]) -> Result<Vec<u8>, SyncError> {
    let cipher = XChaCha20Poly1305::new((&key.0).into());
    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let body = cipher
        .encrypt(&nonce, plain)
        .map_err(|_| SyncError::Storage("encrypting a sync file failed".to_owned()))?;
    let mut out = Vec::with_capacity(NONCE_LEN + body.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&body);
    Ok(out)
}

/// Decrypts what [`seal`] wrote. A wrong key and a damaged file look the
/// same here; [`verify`] tells them apart when a space is opened.
pub fn open(key: &SpaceKey, sealed: &[u8]) -> Result<Vec<u8>, SyncError> {
    if sealed.len() < NONCE_LEN + 16 {
        return Err(SyncError::Truncated);
    }
    let (nonce, body) = sealed.split_at(NONCE_LEN);
    let nonce: [u8; NONCE_LEN] = nonce.try_into().map_err(|_| SyncError::Truncated)?;
    XChaCha20Poly1305::new((&key.0).into())
        .decrypt(&XNonce::from(nonce), body)
        .map_err(|_| SyncError::WrongPassphrase)
}

/// The check value a new space stores (hex).
pub fn check_value(key: &SpaceKey) -> Result<String, SyncError> {
    Ok(hex(&seal(key, CHECK_PLAIN)?))
}

/// Whether `key` opens a space whose check value is `check`.
pub fn verify(key: &SpaceKey, check: &str) -> Result<(), SyncError> {
    let sealed = unhex(check).ok_or(SyncError::Corrupt)?;
    match open(key, &sealed) {
        Ok(p) if p == CHECK_PLAIN => Ok(()),
        _ => Err(SyncError::WrongPassphrase),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fast() -> KdfParams {
        KdfParams {
            log_n: 4,
            ..KdfParams::generate()
        }
    }

    #[test]
    fn seal_roundtrip() {
        let k = derive("secret", &fast()).unwrap();
        let s = seal(&k, b"hello").unwrap();
        assert_ne!(&s[NONCE_LEN..], b"hello");
        assert_eq!(open(&k, &s).unwrap(), b"hello");
        // Fresh nonce every time.
        assert_ne!(seal(&k, b"hello").unwrap(), s);
    }

    #[test]
    fn wrong_key_fails() {
        let p = fast();
        let k = derive("secret", &p).unwrap();
        let other = derive("other", &p).unwrap();
        let s = seal(&k, b"hello").unwrap();
        assert_eq!(open(&other, &s), Err(SyncError::WrongPassphrase));
        let check = check_value(&k).unwrap();
        assert!(verify(&k, &check).is_ok());
        assert_eq!(verify(&other, &check), Err(SyncError::WrongPassphrase));
    }

    #[test]
    fn same_passphrase_same_key() {
        let p = fast();
        let a = derive("pw", &p).unwrap();
        let b = derive("pw", &p).unwrap();
        assert_eq!(open(&b, &seal(&a, b"x").unwrap()).unwrap(), b"x");
    }

    #[test]
    fn hex_roundtrip() {
        assert_eq!(hex(&[0, 15, 255]), "000fff");
        assert_eq!(unhex("000fff").unwrap(), vec![0, 15, 255]);
        assert!(unhex("abc").is_none());
        assert!(unhex("zz").is_none());
    }
}
