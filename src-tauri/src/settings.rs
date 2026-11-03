//! Where the vault is, and the settings that live in it.
//!
//! The vault's location is the one thing that can't live in the vault, so it is kept in a small `config.json` in
//! the app's config folder, together with the other choice that belongs to this machine's copy of Ledger rather than
//! to a vault: whether it looks for updates by itself. Everything else belongs to the vault and sits in
//! `<vault>/.ledger/`, which the vault's `.gitignore` lists, so none of it is ever pushed to GitHub:
//!
//! ```text
//! .ledger/settings.json     the repository, the branch, the screenshot hotkey and tool, whether sync encrypts
//! .ledger/.env              the GitHub access token and the vault's encryption key (the secrets, kept in a file)
//! .ledger/sync-state.json   the sync's own bookkeeping
//! .ledger/conflicts/        versions set aside when a sync resolved a conflict
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

pub const LEDGER_DIR: &str = ".ledger";
const KEY_TOKEN: &str = "GITHUB_TOKEN";

const DEFAULT_GITIGNORE: &str = "\
# Local-only: settings, the access token and sync bookkeeping never leave this machine.
.ledger/
.env
.obsidian/
";

// ---- the app's own config ---------------------------------------------------------------------------

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppConfig {
    pub vault_path: Option<String>,
    /// Read leniently: a value edited into something else by hand falls back to the default instead of making the
    /// whole file unreadable, which would also lose the vault's location.
    #[serde(deserialize_with = "lenient")]
    pub updates: UpdateConfig,
    /// The most the app's log may take on disk; unset means `applog::DEFAULT_MAX_BYTES`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_max_bytes: Option<u64>,
}

fn lenient<'de, D: serde::Deserializer<'de>, T: serde::de::DeserializeOwned + Default>(deserializer: D) -> std::result::Result<T, D::Error> {
    Ok(serde_json::from_value(serde_json::Value::deserialize(deserializer)?).unwrap_or_default())
}

/// Looking for new releases (see `updates.rs`). Per machine, so not in the vault: it is about the program installed
/// here, it must not change when another vault is opened, and an update never touches this file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UpdateConfig {
    /// Look shortly after starting and every few hours. A check from Settings works either way.
    pub automatic: bool,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        UpdateConfig { automatic: true }
    }
}

fn config_file(dir: &Path) -> PathBuf {
    dir.join("config.json")
}

pub fn load_config(dir: &Path) -> AppConfig {
    fs::read_to_string(config_file(dir)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

pub fn save_config(dir: &Path, config: &AppConfig) -> Result<()> {
    fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    let file = config_file(dir);
    let text = serde_json::to_string_pretty(config).unwrap_or_default();
    fs::write(&file, text).map_err(|e| Error::io(&file, e))
}

/// Changes some settings and leaves the others as they were.
pub fn update_config(dir: &Path, change: impl FnOnce(&mut AppConfig)) -> Result<()> {
    let mut config = load_config(dir);
    change(&mut config);
    save_config(dir, &config)
}

/// Where the vault is: `LEDGER_VAULT` if set (and then not changeable from the UI), else the folder chosen in
/// Settings, else `Ledger` inside the user's documents.
pub fn resolve_vault(config_dir: &Path, documents_dir: Option<PathBuf>) -> (PathBuf, bool) {
    if let Some(path) = std::env::var_os("LEDGER_VAULT").filter(|p| !p.is_empty()) {
        return (PathBuf::from(path), true);
    }
    if let Some(path) = load_config(config_dir).vault_path.filter(|p| !p.trim().is_empty()) {
        return (PathBuf::from(path), false);
    }
    (documents_dir.map_or_else(|| config_dir.join("vault"), |d| d.join("Ledger")), false)
}

// ---- the vault's own settings -----------------------------------------------------------------------

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GithubSettings {
    pub repo: String,
    pub branch: String,
}

/// Whether what goes to GitHub is encrypted. The key itself is not here: it is a secret, so it sits in `.ledger/.env`
/// with the access token (see `keystore.rs`).
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EncryptionSettings {
    pub enabled: bool,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct VaultSettings {
    pub github: GithubSettings,
    pub screenshot_hotkey: Option<String>,
    pub screenshot_tool: ScreenshotTool,
    pub encryption: EncryptionSettings,
}

/// How a screenshot is taken: Ledger's built-in capture, or a program of the user's own (Flameshot, say). The
/// command is remembered while the built-in one is in use, so switching back and forth loses nothing.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ScreenshotTool {
    pub use_command: bool,
    pub command: String,
}

pub fn settings_path(root: &Path) -> PathBuf {
    root.join(LEDGER_DIR).join("settings.json")
}

pub fn secrets_path(root: &Path) -> PathBuf {
    root.join(LEDGER_DIR).join(".env")
}

pub fn read_settings(root: &Path) -> VaultSettings {
    fs::read_to_string(settings_path(root)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

/// Changes some settings and leaves the others as they were.
pub fn update_settings(root: &Path, change: impl FnOnce(&mut VaultSettings)) -> Result<()> {
    let mut settings = read_settings(root);
    change(&mut settings);
    let file = settings_path(root);
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    }
    fs::write(&file, serde_json::to_string_pretty(&settings).unwrap_or_default()).map_err(|e| Error::io(&file, e))?;
    ensure_gitignore(root)
}

// ---- GitHub sync: settings plus the secret ----------------------------------------------------------

/// The repository and branch from `settings.json` and the token from `.ledger/.env`, filled in or not.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncEnv {
    pub repo: String,
    pub branch: String,
    pub token: String,
}

/// Sync is on when all three are filled in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncConfig {
    pub repo: String,
    pub branch: String,
    pub token: String,
}

impl SyncEnv {
    pub fn complete(&self) -> Option<SyncConfig> {
        if self.repo.is_empty() || self.branch.is_empty() || self.token.is_empty() {
            return None;
        }
        Some(SyncConfig { repo: self.repo.clone(), branch: self.branch.clone(), token: self.token.clone() })
    }
}

pub fn read_env(root: &Path) -> SyncEnv {
    let github = read_settings(root).github;
    SyncEnv { repo: github.repo, branch: github.branch, token: read_secret(root, KEY_TOKEN).unwrap_or_default() }
}

/// The repository and branch go to `settings.json`, the token to `.env`; other lines in `.env` stay as they were.
pub fn write_env(root: &Path, env: &SyncEnv) -> Result<()> {
    update_settings(root, |s| s.github = GithubSettings { repo: env.repo.clone(), branch: env.branch.clone() })?;
    write_secret(root, KEY_TOKEN, &env.token)
}

/// One value from `.ledger/.env`; the last line that sets it wins. `None` when the file or the line isn't there.
pub fn read_secret(root: &Path, name: &str) -> Option<String> {
    let text = fs::read_to_string(secrets_path(root)).ok()?;
    text.lines().filter_map(parse_line).filter(|(key, _)| *key == name).map(|(_, value)| value).last()
}

/// Sets one line in `.ledger/.env` and leaves every other line as it was. The file stays on this machine: it is
/// gitignored and never synced, and where the system allows it only its owner can read it.
pub fn write_secret(root: &Path, name: &str, value: &str) -> Result<()> {
    let file = secrets_path(root);
    let existing = fs::read_to_string(&file).unwrap_or_default();
    let mut lines: Vec<String> = existing.lines().map(str::to_string).collect();
    if lines.is_empty() {
        lines.push("# Ledger's secrets: the GitHub access token and the vault's encryption key. This file stays on this machine: it is gitignored and never synced.".into());
    }
    let line = format!("{name}={}", quote(value));
    match lines.iter().position(|l| parse_line(l).is_some_and(|(k, _)| k == name)) {
        Some(i) => lines[i] = line,
        None => lines.push(line),
    }

    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    }
    fs::write(&file, lines.join("\n") + "\n").map_err(|e| Error::io(&file, e))?;
    restrict_to_owner(&file);
    ensure_gitignore(root)
}

fn parse_line(line: &str) -> Option<(&str, String)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (key, value) = line.strip_prefix("export ").unwrap_or(line).split_once('=')?;
    let value = value.trim();
    let value = ['"', '\''].iter().find_map(|q| value.strip_prefix(*q).and_then(|v| v.strip_suffix(*q))).unwrap_or(value);
    Some((key.trim(), value.to_string()))
}

fn quote(value: &str) -> String {
    if value.chars().any(|c| c.is_whitespace() || c == '#' || c == '"' || c == '\'') {
        format!("\"{}\"", value.replace('"', ""))
    } else {
        value.to_string()
    }
}

#[cfg(unix)]
fn restrict_to_owner(file: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(file, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_to_owner(_file: &Path) {}

/// Makes sure the vault's `.gitignore` keeps `.ledger/` (settings, token, sync bookkeeping) and any stray `.env`
/// out of GitHub.
pub fn ensure_gitignore(root: &Path) -> Result<()> {
    let file = root.join(".gitignore");
    let Ok(existing) = fs::read_to_string(&file) else {
        return fs::write(&file, DEFAULT_GITIGNORE).map_err(|e| Error::io(&file, e));
    };

    let has = |names: &[&str]| existing.lines().any(|l| names.contains(&l.trim()));
    let mut missing = String::new();
    if !has(&[".ledger/", ".ledger", "/.ledger/", "/.ledger"]) {
        missing.push_str(".ledger/\n");
    }
    if !has(&[".env", "/.env"]) {
        missing.push_str(".env\n");
    }
    if missing.is_empty() {
        return Ok(());
    }
    let separator = if existing.is_empty() || existing.ends_with('\n') { "" } else { "\n" };
    fs::write(&file, format!("{existing}{separator}{missing}")).map_err(|e| Error::io(&file, e))
}

/// `owner/name`, from either that or a github.com URL.
pub fn normalize_repo(input: &str) -> Result<String> {
    let mut s = input.trim();
    for prefix in ["https://github.com/", "http://github.com/", "git@github.com:", "github.com/"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest;
            break;
        }
    }
    let s = s.trim_end_matches('/');
    let s = s.strip_suffix(".git").unwrap_or(s);

    let valid = |part: &str| !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    match s.split_once('/') {
        Some((owner, name)) if valid(owner) && valid(name) => Ok(format!("{owner}/{name}")),
        _ => Err(Error::invalid("The repository must look like owner/name, or be a github.com URL")),
    }
}

pub fn normalize_branch(input: &str) -> Result<String> {
    let branch = input.trim().strip_prefix("refs/heads/").unwrap_or(input.trim());
    if branch.is_empty() || branch.contains(char::is_whitespace) || branch.contains("..") || branch.starts_with('/') || branch.ends_with('/') {
        return Err(Error::invalid("Enter the name of the branch to sync, for example main"));
    }
    Ok(branch.to_string())
}
