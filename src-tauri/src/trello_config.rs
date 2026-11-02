//! The Trello sync's own files, kept apart from the vault's general settings. Both sit in the vault's `.ledger/`
//! folder, which the vault's `.gitignore` lists and the GitHub sync never looks at, so neither is ever uploaded:
//!
//! ```text
//! .ledger/trello.json        which Trello person to sync for, and which board each project is linked to
//! .ledger/trello-sync.json   the sync's own bookkeeping: what both sides last agreed on, for every linked card
//! .ledger/.env               the API key and token (TRELLO_API_KEY, TRELLO_TOKEN), with the other secrets
//! .ledger/conflicts/         the losing version when both sides changed the same thing (shared with the GitHub sync)
//! ```
//!
//! The card id of a linked item is *not* here: it is in the item's own frontmatter (`trelloCard`), so it travels
//! with the item to other machines.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::settings::{self, LEDGER_DIR};
use crate::trello_api::Member;
use crate::trello_card::{Content, Kind};

pub const KEY_API_KEY: &str = "TRELLO_API_KEY";
pub const KEY_TOKEN: &str = "TRELLO_TOKEN";

// ---- settings -----------------------------------------------------------------------------------------

/// A project's link to a Trello board.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProjectLink {
    pub board: String,
    pub board_name: String,
    /// Column id → Trello list id, for the columns whose list isn't simply the one with the same name. An empty
    /// list id means "don't sync this column".
    pub columns: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TrelloSettings {
    /// The one person whose cards are synced.
    pub member: Option<Member>,
    /// Project slug → board.
    pub projects: BTreeMap<String, ProjectLink>,
}

fn settings_file(root: &Path) -> PathBuf {
    root.join(LEDGER_DIR).join("trello.json")
}

fn state_file(root: &Path) -> PathBuf {
    root.join(LEDGER_DIR).join("trello-sync.json")
}

pub fn settings_path(root: &Path) -> PathBuf {
    settings_file(root)
}

pub fn read_settings(root: &Path) -> TrelloSettings {
    fs::read_to_string(settings_file(root)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

fn write_file(file: &Path, text: String) -> Result<()> {
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    }
    fs::write(file, text).map_err(|e| Error::io(file, e))
}

/// Changes some settings and leaves the others as they were.
pub fn update_settings(root: &Path, change: impl FnOnce(&mut TrelloSettings)) -> Result<TrelloSettings> {
    let mut settings = read_settings(root);
    change(&mut settings);
    write_file(&settings_file(root), serde_json::to_string_pretty(&settings).unwrap_or_default())?;
    settings::ensure_gitignore(root)?;
    Ok(settings)
}

// ---- the secrets --------------------------------------------------------------------------------------

#[derive(Clone, Default)]
pub struct Credentials {
    pub api_key: String,
    pub token: String,
}

// Deliberately not `Debug`/`Serialize`: nothing should be able to print these by accident.
impl Credentials {
    pub fn complete(&self) -> bool {
        !self.api_key.is_empty() && !self.token.is_empty()
    }
}

pub fn read_credentials(root: &Path) -> Credentials {
    Credentials {
        api_key: settings::read_secret(root, KEY_API_KEY).unwrap_or_default(),
        token: settings::read_secret(root, KEY_TOKEN).unwrap_or_default(),
    }
}

pub fn write_credentials(root: &Path, credentials: &Credentials) -> Result<()> {
    settings::write_secret(root, KEY_API_KEY, &credentials.api_key)?;
    settings::write_secret(root, KEY_TOKEN, &credentials.token)
}

/// The last four characters of a secret, for telling which one is saved. Nothing for a short one.
pub fn hint(secret: &str) -> String {
    if secret.chars().count() >= 8 {
        format!("…{}", secret.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect::<String>())
    } else {
        String::new()
    }
}

/// Whether there is something to sync: both credentials, a person, and at least one linked project.
pub fn is_configured(root: &Path) -> bool {
    let settings = read_settings(root);
    read_credentials(root).complete() && settings.member.is_some() && settings.projects.values().any(|p| !p.board.is_empty())
}

// ---- the sync's bookkeeping ---------------------------------------------------------------------------

/// A linked card, with what both sides agreed on when they were last in step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tracked {
    pub project: String,
    pub kind: Kind,
    pub id: String,
    pub base: Content,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncState {
    /// Card id → the item it is linked to here.
    pub items: BTreeMap<String, Tracked>,
    /// Cards that were once linked to an item that is gone (deleted, or linked again to a new card) and must not
    /// come back as new items. Nothing on Trello is ever deleted, so such a card stays where it is.
    pub released: BTreeSet<String>,
}

pub fn read_state(root: &Path) -> SyncState {
    fs::read_to_string(state_file(root)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

pub fn write_state(root: &Path, state: &SyncState) -> Result<()> {
    write_file(&state_file(root), serde_json::to_string(state).unwrap_or_default())?;
    settings::ensure_gitignore(root)
}

pub fn update_state(root: &Path, change: impl FnOnce(&mut SyncState)) -> Result<()> {
    let mut state = read_state(root);
    change(&mut state);
    write_state(root, &state)
}
