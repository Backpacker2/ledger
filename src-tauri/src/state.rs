//! The running app: the in-memory database, the vault it is mirrored to, and the sync worker.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use rusqlite::Connection;

use crate::error::{Error, Result};
use crate::keystore::{EnvFileKeyStore, KeyStore};
use crate::screenshot::Screenshots;
use crate::settings::{self, SyncConfig};
use crate::syncer::Syncer;
use crate::vault::Vault;

pub struct Core {
    /// The working copy every read is answered from.
    pub conn: Connection,
    pub vault: Vault,
    /// From the vault's `.env`; `None` when any of repo, branch or token is missing.
    pub sync: Option<SyncConfig>,
    pub config_dir: PathBuf,
    /// The vault location came from `LEDGER_VAULT`, so Settings can't change it.
    pub vault_from_env: bool,
    /// Where the sync encryption key is kept: the vault's `.ledger/.env`, unless a test puts another here.
    pub keys: Arc<dyn KeyStore>,
}

impl Core {
    /// Opens the vault wherever it is configured to be, and loads it.
    pub fn open(config_dir: PathBuf, documents_dir: Option<PathBuf>) -> Result<Core> {
        let (root, from_env) = settings::resolve_vault(&config_dir, documents_dir);
        Core::open_at(root, config_dir, from_env)
    }

    pub fn open_at(root: PathBuf, config_dir: PathBuf, vault_from_env: bool) -> Result<Core> {
        let mut vault = Vault::open(root)?;
        let conn = vault.load()?;
        let sync = settings::read_env(vault.root()).complete();
        Ok(Core { conn, vault, sync, config_dir, vault_from_env, keys: Arc::new(EnvFileKeyStore) })
    }

    /// Reads everything in again from the files.
    pub fn reload(&mut self) -> Result<()> {
        self.conn = self.vault.load()?;
        self.sync = settings::read_env(self.vault.root()).complete();
        Ok(())
    }

    /// Switches to the vault in `path`. With `copy`, and when that folder is empty, the current vault's files
    /// are copied there first, so "move my vault" is one step.
    pub fn set_vault(&mut self, path: &Path, copy: bool) -> Result<()> {
        if self.vault_from_env {
            return Err(Error::invalid("The vault location is set by the LEDGER_VAULT environment variable"));
        }
        if !path.is_absolute() {
            return Err(Error::invalid("Enter the full path of the folder"));
        }
        if path == self.vault.root() {
            return Ok(());
        }
        if path.starts_with(self.vault.root()) || self.vault.root().starts_with(path) {
            return Err(Error::invalid("The new vault can't be inside the current one, or the other way around"));
        }

        fs::create_dir_all(path).map_err(|e| Error::io(path, e))?;
        let empty = fs::read_dir(path).map_err(|e| Error::io(path, e))?.next().is_none();
        if copy && empty {
            copy_dir(self.vault.root(), path)?;
        }

        let mut vault = Vault::open(path.to_path_buf())?;
        let conn = vault.load()?;
        settings::update_config(&self.config_dir, |config| config.vault_path = Some(path.display().to_string()))?;

        self.sync = settings::read_env(vault.root()).complete();
        self.vault = vault;
        self.conn = conn;
        Ok(())
    }
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    for entry in fs::read_dir(from).map_err(|e| Error::io(from, e))?.flatten() {
        let target = to.join(entry.file_name());
        let kind = entry.file_type().map_err(|e| Error::io(&entry.path(), e))?;
        if kind.is_dir() {
            if entry.file_name() == ".git" {
                continue;
            }
            fs::create_dir_all(&target).map_err(|e| Error::io(&target, e))?;
            copy_dir(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), &target).map_err(|e| Error::io(&target, e))?;
        }
    }
    Ok(())
}

/// What the Tauri commands hold on to.
pub struct App {
    core: Arc<Mutex<Core>>,
    pub syncer: Syncer,
    pub screenshots: Screenshots,
}

impl App {
    pub fn new(core: Arc<Mutex<Core>>, syncer: Syncer, screenshots: Screenshots) -> App {
        App { core, syncer, screenshots }
    }

    pub fn lock(&self) -> MutexGuard<'_, Core> {
        self.core.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Answers from memory. The closure also gets the vault's root, for the files the database only points at.
    pub fn read<T>(&self, f: impl FnOnce(&Connection, &Path) -> Result<T>) -> Result<T> {
        let core = self.lock();
        f(&core.conn, core.vault.root())
    }

    /// Changes memory, writes the difference through to the vault's files, and lets the sync worker know.
    pub fn mutate<T>(&self, f: impl FnOnce(&Connection, &Path) -> Result<T>) -> Result<T> {
        let out = {
            let mut guard = self.lock();
            let core = &mut *guard;
            let out = f(&core.conn, core.vault.root())?;
            core.vault.persist(&core.conn)?;
            out
        };
        // Always, not only when `persist` wrote something: some changes are made to files directly (a published
        // image, an upload), which the write-through doesn't count.
        self.syncer.dirty();
        Ok(out)
    }
}
