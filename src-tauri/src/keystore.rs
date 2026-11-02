//! Where the vault's encryption key lives. For now that is the vault's own `.ledger/.env`, next to the GitHub access
//! token: a plain file, so it works the same on Windows, macOS and Linux with nothing to install, unlock or sign in
//! to. The cost is that the key sits there in plain text, readable by anything that can read this user's files (on
//! Unix the file is owner-only). `.ledger/` is gitignored and never synced, so it is not uploaded, and it is never
//! written to `settings.json`.
//!
//! The store is a trait so that a safer home (the system keychain, say) can replace the file later without touching
//! the rest, and so that tests can use an in-memory one.

use std::path::Path;

use serde::Serialize;
use zeroize::Zeroizing;

use crate::crypto::Key;
use crate::error::{Error, Result};
use crate::settings;
use crate::sync;

/// Somewhere to keep the key of the vault at `root`.
pub trait KeyStore: Send + Sync {
    /// The vault's key, `None` if it has none.
    fn load(&self, root: &Path) -> Result<Option<Key>>;
    fn save(&self, root: &Path, key: &Key) -> Result<()>;
}

// ---- the real one -----------------------------------------------------------------------------------

/// The line in `.ledger/.env` that holds the key.
const ENV_KEY: &str = "LEDGER_ENCRYPTION_KEY";

/// The key as a line of the vault's `.ledger/.env`. It is kept as its recovery string, which carries a checksum, so
/// a damaged line is noticed instead of used. The other lines of the file (the access token) are left alone.
pub struct EnvFileKeyStore;

impl KeyStore for EnvFileKeyStore {
    fn load(&self, root: &Path) -> Result<Option<Key>> {
        match settings::read_secret(root, ENV_KEY).map(Zeroizing::new).filter(|text| !text.trim().is_empty()) {
            None => Ok(None),
            Some(text) => Key::parse(&text).map(Some).map_err(|_| {
                Error::sync("The encryption key saved on this machine is damaged. Import it again from its recovery key.")
            }),
        }
    }

    fn save(&self, root: &Path, key: &Key) -> Result<()> {
        settings::write_secret(root, ENV_KEY, &key.to_recovery())
    }
}

// ---- one for tests ----------------------------------------------------------------------------------

/// Keys in memory, by vault, so that a test can have several "machines" without touching any file.
#[cfg(test)]
#[derive(Default)]
pub struct MemoryKeyStore {
    keys: std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, Zeroizing<String>>>,
}

#[cfg(test)]
impl KeyStore for MemoryKeyStore {
    fn load(&self, root: &Path) -> Result<Option<Key>> {
        let keys = self.keys.lock().unwrap();
        Ok(keys.get(root).map(|text| Key::parse(text).expect("the memory store only holds what it was given")))
    }

    fn save(&self, root: &Path, key: &Key) -> Result<()> {
        self.keys.lock().unwrap().insert(root.to_path_buf(), key.to_recovery());
        Ok(())
    }
}

// ---- what the app does with them --------------------------------------------------------------------

/// What the settings screen learns. Never the key.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EncryptionStatus {
    pub enabled: bool,
    pub has_key: bool,
    /// The one-byte label of the key as two hex digits, so keys can be told apart on different machines.
    pub key_id: Option<String>,
    /// Why the key can't be read, if it can't (a damaged line in `.env`, an unreadable file).
    pub problem: Option<String>,
    /// This vault has already synced the configured repository and branch without encryption, so that branch can't
    /// become an encrypted one: it takes a new repository, or a branch that doesn't exist yet.
    pub needs_fresh_target: bool,
}

pub fn status(root: &Path, keys: &dyn KeyStore) -> EncryptionStatus {
    let enabled = settings::read_settings(root).encryption.enabled;
    let target = settings::read_env(root);
    let needs_fresh_target = sync::has_plain_history(root, &target.repo, &target.branch);
    match keys.load(root) {
        Ok(key) => EncryptionStatus {
            enabled,
            has_key: key.is_some(),
            key_id: key.as_ref().map(Key::id_hex),
            problem: None,
            needs_fresh_target,
        },
        Err(e) => EncryptionStatus { enabled, problem: Some(e.to_string()), needs_fresh_target, ..EncryptionStatus::default() },
    }
}

/// Makes a new random key and keeps it. With a key already here that needs `replace`: files encrypted with the
/// old one can't be read without it.
pub fn generate(root: &Path, keys: &dyn KeyStore, replace: bool) -> Result<()> {
    install(root, keys, Key::generate(), replace)
}

/// Keeps the key from a recovery string (or the bare key as hex or base64).
pub fn import(root: &Path, keys: &dyn KeyStore, text: &str, replace: bool) -> Result<()> {
    let key = Key::parse(text).map_err(|e| Error::invalid(e.to_string()))?;
    install(root, keys, key, replace)
}

fn install(root: &Path, keys: &dyn KeyStore, key: Key, replace: bool) -> Result<()> {
    // A key that can't be read is as good as none: it is not something worth protecting from replacement.
    if matches!(keys.load(root), Ok(Some(_))) && !replace {
        return Err(Error::invalid(
            "There already is a key on this machine. Replace it only if you have a copy of it: files encrypted with it can't be read without it.",
        ));
    }
    keys.save(root, &key)
}

/// The recovery string for the key on this machine: the way to get it onto another one, and the way back if this
/// machine is lost. Only ever handed out on request.
pub fn export(root: &Path, keys: &dyn KeyStore) -> Result<Zeroizing<String>> {
    match keys.load(root)? {
        Some(key) => Ok(key.to_recovery()),
        None => Err(Error::invalid("There is no key on this machine to export")),
    }
}

/// Turns encryption on or off for the next sync. It can't be turned on without a key.
pub fn set_enabled(root: &Path, keys: &dyn KeyStore, enabled: bool) -> Result<()> {
    if enabled && keys.load(root)?.is_none() {
        return Err(Error::invalid("Create or import a key first: without one there is nothing to encrypt with"));
    }
    settings::update_settings(root, |s| s.encryption.enabled = enabled)
}

/// The key a sync should encrypt with. `None` when encryption is off. When it is on but this machine has no key
/// (a new machine, a `.env` that lost it) that is an error: syncing on regardless would push plain files or fail in
/// confusing ways, so it stops with what to do.
pub fn key_for_sync(root: &Path, keys: &dyn KeyStore) -> Result<Option<Key>> {
    if !settings::read_settings(root).encryption.enabled {
        return Ok(None);
    }
    match keys.load(root)? {
        Some(key) => Ok(Some(key)),
        None => Err(Error::sync(
            "Encryption is on, but this machine has no key for it. Import the key in Settings (from its recovery key) to carry on syncing. Nothing was changed.",
        )),
    }
}
