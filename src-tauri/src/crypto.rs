//! Encrypting the vault's files on their way to GitHub. The vault on disk stays plain markdown; only what is
//! uploaded is sealed, with a key that stays on this machine (see `keystore.rs`) and is never uploaded.
//!
//! A sealed file is, byte for byte:
//!
//! ```text
//! 0x89 'L' 'D' 'G'   magic (the first byte is not valid UTF-8, so no text file starts like this)
//! version            1
//! key id             one byte taken from the key, so a wrong key is recognised before anything is decrypted
//! nonce              12 random bytes, new for every encryption
//! ciphertext + tag   AES-256-GCM over the deflated file
//! ```
//!
//! The header and the file's vault-relative path are the AEAD's associated data: a blob moved to another path,
//! or with its header altered, fails to decrypt. Files are deflated *before* they are sealed, because ciphertext
//! does not compress. [`armor`] turns a sealed file into base64 text for storage on GitHub, which lets the
//! GitHub client upload it inside the tree request like any markdown file instead of as one blob request per file.

use std::fmt;
use std::io::{Read, Write};

use aes_gcm::aead::{Aead, Generate, Nonce, Payload};
use aes_gcm::{Aes256Gcm, KeyInit};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE_NO_PAD};
use flate2::Compression;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::sync::MAX_FILE_BYTES;

pub const KEY_BYTES: usize = 32;
const MAGIC: [u8; 4] = [0x89, b'L', b'D', b'G'];
const VERSION: u8 = 1;
/// Magic, version and key id: the part of the header that is also authenticated.
const HEADER_BYTES: usize = MAGIC.len() + 2;
const NONCE_BYTES: usize = 12;
const TAG_BYTES: usize = 16;
/// Where a recovery string starts, and the checksum that follows the key inside it.
const RECOVERY_PREFIX: &str = "ledger-key-1:";
const CHECK_BYTES: usize = 4;

// ---- errors -----------------------------------------------------------------------------------------

/// Why a file could not be sealed or opened. Deliberately vague about *which* check failed: that is not for
/// anyone watching the repository to learn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CryptoError {
    /// Not one of ours: no header.
    NotEncrypted,
    /// Sealed by a newer format than this app knows.
    UnsupportedVersion(u8),
    /// Sealed with another key (by its id).
    WrongKey { found: u8, expected: u8 },
    /// The key is wrong after all (ids can coincide), or the file was altered, or it belongs at another path.
    Failed,
    /// Cut short or otherwise not a sealed file.
    Corrupt,
    /// Bigger than the vault syncs once unpacked.
    TooLarge,
}

impl fmt::Display for CryptoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CryptoError::NotEncrypted => f.write_str("isn't encrypted"),
            CryptoError::UnsupportedVersion(v) => write!(f, "was encrypted by a newer version of Ledger (format {v})"),
            CryptoError::WrongKey { found, expected } => {
                write!(f, "was encrypted with a different key (id {found:02x}; this machine's key is {expected:02x})")
            }
            CryptoError::Failed => f.write_str("can't be decrypted: the key is wrong, or the file was altered or moved"),
            CryptoError::Corrupt => f.write_str("is damaged"),
            CryptoError::TooLarge => f.write_str("is larger than Ledger will sync"),
        }
    }
}

impl std::error::Error for CryptoError {}

/// Why text offered as a key was refused. Never says what was typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyError {
    /// Not a recognisable key.
    Malformed,
    /// A recovery string whose check failed: a character was mistyped or lost.
    BadChecksum,
    /// A recovery string from a newer format.
    UnsupportedVersion,
}

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            KeyError::Malformed => "That doesn't look like a Ledger key. Paste the recovery key exactly as it was exported.",
            KeyError::BadChecksum => "That recovery key has a typo or is cut short: its check failed. Copy it again in full.",
            KeyError::UnsupportedVersion => "That recovery key was made by a newer version of Ledger.",
        })
    }
}

// ---- the key ----------------------------------------------------------------------------------------

/// 32 random bytes. Wiped from memory when dropped; not `Clone`, and its `Debug` shows nothing.
pub struct Key(Zeroizing<[u8; KEY_BYTES]>);

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Key(id {:02x})", self.id())
    }
}

impl Key {
    pub fn generate() -> Key {
        Key(Zeroizing::new(<[u8; KEY_BYTES]>::generate()))
    }

    pub fn from_bytes(bytes: [u8; KEY_BYTES]) -> Key {
        Key(Zeroizing::new(bytes))
    }

    #[cfg(test)]
    pub fn as_bytes(&self) -> &[u8; KEY_BYTES] {
        &self.0
    }

    /// A one-byte label for this key: enough to tell keys apart in a header and on screen, far too little to
    /// say anything about the key.
    pub fn id(&self) -> u8 {
        digest(b"ledger key id", &self.0[..])[0]
    }

    pub fn id_hex(&self) -> String {
        format!("{:02x}", self.id())
    }

    /// The text to keep somewhere safe: `ledger-key-1:` and the key, with a checksum so a typo is caught.
    pub fn to_recovery(&self) -> Zeroizing<String> {
        let mut payload = Zeroizing::new(Vec::with_capacity(KEY_BYTES + CHECK_BYTES));
        payload.extend_from_slice(&self.0[..]);
        payload.extend_from_slice(&digest(b"ledger recovery check", &self.0[..])[..CHECK_BYTES]);
        Zeroizing::new(format!("{RECOVERY_PREFIX}{}", URL_SAFE_NO_PAD.encode(&payload[..])))
    }

    /// A recovery string, or the bare key as 64 hex digits or as base64.
    pub fn parse(text: &str) -> std::result::Result<Key, KeyError> {
        // Pasting often brings spaces and line breaks along.
        let text: Zeroizing<String> = Zeroizing::new(text.chars().filter(|c| !c.is_whitespace()).collect());

        if let Some(encoded) = text.strip_prefix(RECOVERY_PREFIX) {
            let payload = Zeroizing::new(URL_SAFE_NO_PAD.decode(encoded.trim_end_matches('=')).map_err(|_| KeyError::Malformed)?);
            if payload.len() != KEY_BYTES + CHECK_BYTES {
                return Err(KeyError::Malformed);
            }
            let key = Key::from_slice(&payload[..KEY_BYTES]).ok_or(KeyError::Malformed)?;
            if digest(b"ledger recovery check", &key.0[..])[..CHECK_BYTES] != payload[KEY_BYTES..] {
                return Err(KeyError::BadChecksum);
            }
            return Ok(key);
        }
        if text.starts_with("ledger-key-") {
            return Err(KeyError::UnsupportedVersion);
        }

        if text.len() == KEY_BYTES * 2 && text.chars().all(|c| c.is_ascii_hexdigit()) {
            let mut bytes = Zeroizing::new([0u8; KEY_BYTES]);
            for (i, pair) in text.as_bytes().chunks(2).enumerate() {
                bytes[i] = u8::from_str_radix(std::str::from_utf8(pair).map_err(|_| KeyError::Malformed)?, 16).map_err(|_| KeyError::Malformed)?;
            }
            return Ok(Key::from_bytes(*bytes));
        }
        let unpadded = text.trim_end_matches('=');
        for engine in [STANDARD_NO_PAD, URL_SAFE_NO_PAD] {
            if let Ok(bytes) = engine.decode(unpadded).map(Zeroizing::new)
                && let Some(key) = Key::from_slice(&bytes)
            {
                return Ok(key);
            }
        }
        Err(KeyError::Malformed)
    }

    fn from_slice(bytes: &[u8]) -> Option<Key> {
        Some(Key::from_bytes(<[u8; KEY_BYTES]>::try_from(bytes).ok()?))
    }

    fn cipher(&self) -> std::result::Result<Aes256Gcm, CryptoError> {
        Aes256Gcm::new_from_slice(&self.0[..]).map_err(|_| CryptoError::Failed)
    }
}

fn digest(domain: &[u8], key: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(key);
    hasher.finalize().into()
}

// ---- sealing and opening ----------------------------------------------------------------------------

/// What the associated data is: the header, then the path.
fn associated_data(header: &[u8], path: &str) -> Vec<u8> {
    let mut aad = Vec::with_capacity(header.len() + path.len());
    aad.extend_from_slice(header);
    aad.extend_from_slice(path.as_bytes());
    aad
}

/// Seals a file for the vault-relative `path`: deflated, then AES-256-GCM with a fresh random nonce.
pub fn encrypt(key: &Key, path: &str, plaintext: &[u8]) -> std::result::Result<Vec<u8>, CryptoError> {
    if plaintext.len() as u64 > MAX_FILE_BYTES {
        return Err(CryptoError::TooLarge);
    }
    let mut deflater = DeflateEncoder::new(Vec::new(), Compression::default());
    deflater.write_all(plaintext).map_err(|_| CryptoError::Corrupt)?;
    let packed = Zeroizing::new(deflater.finish().map_err(|_| CryptoError::Corrupt)?);

    let mut out = Vec::with_capacity(HEADER_BYTES + NONCE_BYTES + packed.len() + TAG_BYTES);
    out.extend_from_slice(&MAGIC);
    out.push(VERSION);
    out.push(key.id());
    let nonce = Nonce::<Aes256Gcm>::generate();
    out.extend_from_slice(&nonce);
    let sealed = key
        .cipher()?
        .encrypt(&nonce, Payload { msg: &packed, aad: &associated_data(&out[..HEADER_BYTES], path) })
        .map_err(|_| CryptoError::Failed)?;
    out.extend_from_slice(&sealed);
    Ok(out)
}

/// Opens a file sealed by [`encrypt`] for the same `path`.
pub fn decrypt(key: &Key, path: &str, blob: &[u8]) -> std::result::Result<Vec<u8>, CryptoError> {
    if !blob.starts_with(&MAGIC) {
        return Err(CryptoError::NotEncrypted);
    }
    if blob.len() < HEADER_BYTES + NONCE_BYTES + TAG_BYTES {
        return Err(CryptoError::Corrupt);
    }
    let (header, rest) = blob.split_at(HEADER_BYTES);
    if header[MAGIC.len()] != VERSION {
        return Err(CryptoError::UnsupportedVersion(header[MAGIC.len()]));
    }
    let found = header[MAGIC.len() + 1];
    if found != key.id() {
        return Err(CryptoError::WrongKey { found, expected: key.id() });
    }
    let (nonce, sealed) = rest.split_at(NONCE_BYTES);
    let nonce = Nonce::<Aes256Gcm>::try_from(nonce).map_err(|_| CryptoError::Corrupt)?;
    let packed = Zeroizing::new(
        key.cipher()?.decrypt(&nonce, Payload { msg: sealed, aad: &associated_data(header, path) }).map_err(|_| CryptoError::Failed)?,
    );

    inflate(&packed, MAX_FILE_BYTES)
}

/// Unpacks deflated data into at most `limit` bytes. What was sealed is authentic by the time this runs, but a
/// bound keeps even someone holding the key from making this machine inflate gigabytes.
fn inflate(packed: &[u8], limit: u64) -> std::result::Result<Vec<u8>, CryptoError> {
    let mut plaintext = Vec::new();
    DeflateDecoder::new(packed).take(limit + 1).read_to_end(&mut plaintext).map_err(|_| CryptoError::Corrupt)?;
    if plaintext.len() as u64 > limit {
        return Err(CryptoError::TooLarge);
    }
    Ok(plaintext)
}

#[cfg(test)]
pub(crate) fn inflate_for_test(packed: &[u8], limit: u64) -> std::result::Result<Vec<u8>, CryptoError> {
    inflate(packed, limit)
}

// ---- how a sealed file is stored --------------------------------------------------------------------

/// A sealed file as the text kept on GitHub.
pub fn armor(sealed: &[u8]) -> Vec<u8> {
    STANDARD.encode(sealed).into_bytes()
}

/// The sealed file inside what [`armor`] made. `NotEncrypted` for anything that doesn't begin like one.
pub fn unarmor(stored: &[u8]) -> std::result::Result<Vec<u8>, CryptoError> {
    if !is_encrypted(stored) {
        return Err(CryptoError::NotEncrypted);
    }
    STANDARD.decode(stored.trim_ascii_end()).map_err(|_| CryptoError::Corrupt)
}

/// Whether a file as stored on GitHub is one of ours. Only looks at how it starts.
pub fn is_encrypted(stored: &[u8]) -> bool {
    // Base64 turns every three bytes into four characters, so the four magic bytes are fixed in the first six.
    let mut head = MAGIC.to_vec();
    head.push(0);
    stored.starts_with(&STANDARD.encode(head).as_bytes()[..6])
}
