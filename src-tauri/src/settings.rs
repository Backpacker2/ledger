//! Where the vault is, and how it syncs.
//!
//! The vault's location is the one thing that can't live in the vault, so it is kept in a small `config.json`
//! in the app's config folder. Everything about GitHub — repository, branch and the access token — lives in a
//! `.env` file *inside the vault*, next to the notes it syncs. A `.gitignore` in the vault lists `.env`, so the
//! token never reaches GitHub, and the app reads `.env` at startup to see whether sync is set up.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

pub const ENV_FILE: &str = ".env";
const KEY_REPO: &str = "GITHUB_REPO";
const KEY_BRANCH: &str = "GITHUB_BRANCH";
const KEY_TOKEN: &str = "GITHUB_TOKEN";

const DEFAULT_GITIGNORE: &str = "\
# Local-only files: these never leave this machine.
.env
.ledger/
.obsidian/
";

// ---- the app's own config ---------------------------------------------------------------------------

/// Per-machine settings that live outside the vault, so they are never synced to other machines.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppConfig {
    pub vault_path: Option<String>,
    pub screenshot_hotkey: Option<String>,
    pub screenshot_upload_url: Option<String>,
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

// ---- GitHub sync settings in the vault's .env -------------------------------------------------------

/// What `.env` says, filled in or not.
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
    let mut env = SyncEnv::default();
    let Ok(text) = fs::read_to_string(root.join(ENV_FILE)) else { return env };
    for line in text.lines() {
        match parse_line(line) {
            Some((KEY_REPO, value)) => env.repo = value,
            Some((KEY_BRANCH, value)) => env.branch = value,
            Some((KEY_TOKEN, value)) => env.token = value,
            _ => {}
        }
    }
    env
}

/// Rewrites the three keys and leaves every other line (comments, unrelated variables) as it was.
pub fn write_env(root: &Path, env: &SyncEnv) -> Result<()> {
    let file = root.join(ENV_FILE);
    let existing = fs::read_to_string(&file).unwrap_or_default();

    let mut lines: Vec<String> = existing.lines().map(str::to_string).collect();
    if lines.is_empty() {
        lines.push("# Ledger's GitHub sync settings. This file stays on this machine: .gitignore keeps it out of the repo.".into());
    }
    for (key, value) in [(KEY_REPO, &env.repo), (KEY_BRANCH, &env.branch), (KEY_TOKEN, &env.token)] {
        let line = format!("{key}={}", quote(value));
        match lines.iter().position(|l| parse_line(l).is_some_and(|(k, _)| k == key)) {
            Some(i) => lines[i] = line,
            None => lines.push(line),
        }
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

/// Makes sure the vault's `.gitignore` keeps `.env` and the sync bookkeeping out of GitHub.
pub fn ensure_gitignore(root: &Path) -> Result<()> {
    let file = root.join(".gitignore");
    let Ok(existing) = fs::read_to_string(&file) else {
        return fs::write(&file, DEFAULT_GITIGNORE).map_err(|e| Error::io(&file, e));
    };

    let has = |names: &[&str]| existing.lines().any(|l| names.contains(&l.trim()));
    let mut missing = String::new();
    if !has(&[".env", "/.env"]) {
        missing.push_str(".env\n");
    }
    if !has(&[".ledger/", ".ledger", "/.ledger/", "/.ledger"]) {
        missing.push_str(".ledger/\n");
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
