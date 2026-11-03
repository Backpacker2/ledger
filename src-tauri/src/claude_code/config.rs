//! The settings for running prompts with Claude Code: where the program is, how much it may do, which model.
//!
//! They live in `<vault>/.ledger/claude-code.json`, next to the vault's other local settings: the folder is
//! gitignored and ignored by the sync, so none of this leaves the machine. (What a run did is not a setting: it is
//! kept in the thread, see `records`.) A missing, old or damaged file loads as
//! the defaults, field by field, so a setting this version doesn't know about never breaks the screen.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::settings::{self, LEDGER_DIR};

pub const SETTINGS_FILE: &str = "claude-code.json";
pub const GUARD_FILE: &str = "claude-guard.json";

/// How much Claude Code may do on its own. Only modes that can't skip permission checks are offered: there is
/// deliberately no way to choose `bypassPermissions` (`--dangerously-skip-permissions`), from here or from a
/// hand-edited settings file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PermissionMode {
    /// Reads the project and creates or edits files inside it. This is the mode that can write a deliverable.
    #[default]
    AcceptEdits,
    /// Reads and thinks, and writes a plan as its answer; changes nothing. For looking before committing to a run.
    Plan,
    /// Reads the project; anything that would need approval (all edits, other commands, the network) is refused.
    DontAsk,
}

impl PermissionMode {
    /// The value the CLI's `--permission-mode` takes.
    pub fn as_str(self) -> &'static str {
        match self {
            PermissionMode::AcceptEdits => "acceptEdits",
            PermissionMode::Plan => "plan",
            PermissionMode::DontAsk => "dontAsk",
        }
    }

    pub fn parse(text: &str) -> Option<PermissionMode> {
        match text.trim() {
            "acceptEdits" => Some(PermissionMode::AcceptEdits),
            "plan" => Some(PermissionMode::Plan),
            "dontAsk" => Some(PermissionMode::DontAsk),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeSettings {
    /// The program to run; empty means "find `claude` on the PATH".
    pub cli_path: String,
    pub permission_mode: PermissionMode,
    /// A model name or alias for `--model`; empty means whatever the CLI defaults to.
    pub model: String,
    /// Extra guard rails (on by default): the run ignores the project folder's own Claude settings and is told not
    /// to read or write outside the project folder or touch `.ledger/` and `.env` files. Needs a recent Claude Code.
    pub confine: bool,
    /// Off by default: a prompt that is created starts running at once, without the confirmation that shows what will
    /// be sent. The window does the starting (it is the one that sees a prompt being created); this is only the choice.
    pub auto_run: bool,
}

impl Default for ClaudeSettings {
    fn default() -> Self {
        ClaudeSettings { cli_path: String::new(), permission_mode: PermissionMode::default(), model: String::new(), confine: true, auto_run: false }
    }
}

pub fn settings_path(root: &Path) -> PathBuf {
    root.join(LEDGER_DIR).join(SETTINGS_FILE)
}

/// The settings file handed to each run for the guard rails, rewritten before every run.
pub fn guard_path(root: &Path) -> PathBuf {
    root.join(LEDGER_DIR).join(GUARD_FILE)
}

impl ClaudeSettings {
    /// What the user typed, checked. The path may be pasted with its quotes; the model has to look like a model name,
    /// since it ends up in the command line.
    pub fn validated(cli_path: &str, permission_mode: &str, model: &str, confine: bool, auto_run: bool) -> Result<ClaudeSettings> {
        let mode = PermissionMode::parse(permission_mode).ok_or_else(|| Error::invalid("Choose one of the permission modes offered"))?;
        let model = model.trim();
        if !model.is_empty() && !valid_model(model) {
            return Err(Error::invalid(
                "That doesn't look like a model name. Use letters, digits and . _ : - / [ ] only, for example sonnet, opus or claude-sonnet-4-5",
            ));
        }
        Ok(ClaudeSettings { cli_path: clean_path(cli_path)?, permission_mode: mode, model: model.to_string(), confine, auto_run })
    }

    fn to_json(&self) -> Value {
        json!({
            "cliPath": self.cli_path,
            "permissionMode": self.permission_mode.as_str(),
            "model": self.model,
            "confine": self.confine,
            "autoRun": self.auto_run,
        })
    }
}

/// A model name as the CLI takes it: an alias (`sonnet`, `opus[1m]`) or a full id. Starting with a letter or digit
/// keeps it from ever being read as another option.
pub fn valid_model(model: &str) -> bool {
    let mut chars = model.chars();
    model.len() <= 100
        && chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-' | '/' | '[' | ']'))
}

fn clean_path(input: &str) -> Result<String> {
    let mut path = input.trim();
    for quote in ['"', '\''] {
        if let Some(inner) = path.strip_prefix(quote).and_then(|p| p.strip_suffix(quote)) {
            path = inner.trim();
        }
    }
    if path.len() > 1024 || path.chars().any(char::is_control) {
        return Err(Error::invalid("That isn't a path to a program"));
    }
    Ok(path.to_string())
}

/// The saved settings, with the defaults for anything missing or unusable.
pub fn load(root: &Path) -> ClaudeSettings {
    let defaults = ClaudeSettings::default();
    let Some(value) = fs::read_to_string(settings_path(root)).ok().and_then(|text| serde_json::from_str::<Value>(&text).ok()) else {
        return defaults;
    };
    let text = |key: &str| value.get(key).and_then(Value::as_str);
    ClaudeSettings {
        cli_path: text("cliPath").and_then(|p| clean_path(p).ok()).unwrap_or(defaults.cli_path),
        permission_mode: text("permissionMode").and_then(PermissionMode::parse).unwrap_or(defaults.permission_mode),
        model: text("model").map(str::trim).filter(|m| valid_model(m)).map(str::to_string).unwrap_or(defaults.model),
        confine: value.get("confine").and_then(Value::as_bool).unwrap_or(defaults.confine),
        auto_run: value.get("autoRun").and_then(Value::as_bool).unwrap_or(defaults.auto_run),
    }
}

pub fn save(root: &Path, settings: &ClaudeSettings) -> Result<()> {
    let file = settings_path(root);
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    }
    let text = serde_json::to_string_pretty(&settings.to_json()).unwrap_or_default();
    fs::write(&file, text).map_err(|e| Error::io(&file, e))?;
    settings::ensure_gitignore(root)
}
