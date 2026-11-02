//! Finding the `claude` program, and the command line it is run with.
//!
//! The prompt itself never goes on the command line: it is written to the program's standard input (see
//! `runner`), so no quoting question arises for it and Windows' command-line length limit doesn't apply. What does
//! go on the command line is a fixed set of flags plus two values that are validated first (the permission mode, an
//! enum, and the model name, which `config::valid_model` limits to letters, digits and a few separators).

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use serde_json::json;

use super::config::ClaudeSettings;
use crate::error::{Error, Result};
use crate::settings::LEDGER_DIR;

// ---- finding the program ----------------------------------------------------------------------------

/// The file names `claude` goes by. On Windows the native installer makes `claude.exe`, and an npm install makes a
/// `claude.cmd` shim; on macOS and Linux it is plain `claude`.
pub fn candidate_names(windows: bool) -> &'static [&'static str] {
    if windows { &["claude.exe", "claude.cmd"] } else { &["claude"] }
}

/// Where to look, apart from an explicit path: the PATH, and the places the installers use that a desktop app
/// started from the Dock or the Start menu may not have on its PATH.
#[derive(Debug, Clone, Default)]
pub struct Search {
    pub path_var: Option<OsString>,
    pub fallback_dirs: Vec<PathBuf>,
    pub windows: bool,
}

impl Search {
    /// This machine's PATH and the usual install folders.
    pub fn host() -> Search {
        let mut fallback_dirs = Vec::new();
        let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
        let home = if cfg!(windows) { var("USERPROFILE").or_else(|| var("HOME")) } else { var("HOME") };
        if let Some(home) = home {
            fallback_dirs.push(home.join(".local").join("bin"));
            fallback_dirs.push(home.join(".claude").join("local"));
            fallback_dirs.push(home.join(".npm-global").join("bin"));
        }
        if cfg!(windows) {
            if let Some(appdata) = var("APPDATA") {
                fallback_dirs.push(appdata.join("npm"));
            }
        } else {
            fallback_dirs.push(PathBuf::from("/opt/homebrew/bin"));
            fallback_dirs.push(PathBuf::from("/usr/local/bin"));
        }
        Search { path_var: std::env::var_os("PATH"), fallback_dirs, windows: cfg!(windows) }
    }

    pub fn path_dirs(&self) -> Vec<PathBuf> {
        // A relative PATH entry ("." or an empty one) would mean "wherever the app happens to be", and a program
        // planted there would run with the user's rights. Only absolute folders count.
        self.path_var.iter().flat_map(std::env::split_paths).filter(|dir| dir.is_absolute()).collect()
    }
}

fn is_program(path: &Path) -> bool {
    let Ok(meta) = path.metadata() else { return false };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn find_in(dirs: &[PathBuf], names: &[String]) -> Option<PathBuf> {
    dirs.iter().flat_map(|dir| names.iter().map(move |name| dir.join(name))).find(|p| is_program(p))
}

fn default_names(windows: bool) -> Vec<String> {
    candidate_names(windows).iter().map(|n| n.to_string()).collect()
}

pub fn not_found() -> Error {
    Error::not_found(
        "Claude Code wasn't found. Install it (claude.com/claude-code), then restart Ledger so it is on the PATH, or enter the full path of the claude program under Settings, Claude Code.",
    )
}

/// The program to run: the path from Settings if there is one, else `claude` from the PATH, else from the usual
/// install folders.
pub fn resolve(configured: &str, search: &Search) -> Result<PathBuf> {
    let configured = configured.trim();
    if configured.is_empty() {
        let mut dirs = search.path_dirs();
        dirs.extend(search.fallback_dirs.iter().cloned());
        return find_in(&dirs, &default_names(search.windows)).ok_or_else(not_found);
    }

    let as_path = Path::new(configured);
    let named_only = as_path.components().count() == 1 && !as_path.is_absolute();
    if named_only {
        // A bare name such as "claude-code": look it up like `claude`.
        let names = with_extensions(configured, search.windows);
        let mut dirs = search.path_dirs();
        dirs.extend(search.fallback_dirs.iter().cloned());
        return find_in(&dirs, &names).ok_or_else(|| {
            Error::not_found(format!("\"{configured}\" (set under Settings, Claude Code) wasn't found on the PATH. Enter its full path instead."))
        });
    }
    if as_path.is_dir() {
        // The folder it is installed in.
        return find_in(&[as_path.to_path_buf()], &default_names(search.windows)).ok_or_else(|| {
            Error::not_found(format!("There is no claude program in {configured} (the path set under Settings, Claude Code)."))
        });
    }
    for name in with_extensions(configured, search.windows) {
        let candidate = PathBuf::from(&name);
        if is_program(&candidate) {
            return Ok(candidate);
        }
    }
    Err(Error::not_found(format!("There is no program at {configured} (the path set under Settings, Claude Code).")))
}

/// `name` as given, and on Windows, with the extensions a program can have when none was typed.
fn with_extensions(name: &str, windows: bool) -> Vec<String> {
    let mut names = vec![name.to_string()];
    if windows && Path::new(name).extension().is_none() {
        names.push(format!("{name}.exe"));
        names.push(format!("{name}.cmd"));
    }
    names
}

// ---- the command line -------------------------------------------------------------------------------

/// The arguments for a run: print mode with streaming JSON output (which needs `--verbose`), the chosen permission
/// mode, and optionally the model and the guard rails. The prompt is not among them.
///
/// The guard rails are `--settings <file>` (the deny rules and limits of `guard_settings`), `--setting-sources user`
/// (the project folder's own `.claude/settings*.json`, which could hold hooks, are not read: the vault is synced
/// from elsewhere, and a hook there would run with the user's rights) and `--strict-mcp-config` (with no
/// `--mcp-config` it means no MCP servers at all, so a `.mcp.json` in the vault can't start a program either).
pub fn run_args(settings: &ClaudeSettings, guard_file: Option<&Path>) -> Vec<OsString> {
    let mut args: Vec<OsString> = ["-p", "--output-format", "stream-json", "--verbose", "--permission-mode"].map(OsString::from).into();
    args.push(settings.permission_mode.as_str().into());
    if !settings.model.is_empty() {
        args.push("--model".into());
        args.push(settings.model.as_str().into());
    }
    if let Some(file) = guard_file {
        args.push("--setting-sources".into());
        args.push("user".into());
        args.push("--settings".into());
        args.push(file.as_os_str().to_os_string());
        args.push("--strict-mcp-config".into());
    }
    args
}

pub fn version_args() -> Vec<OsString> {
    vec![OsString::from("--version")]
}

/// For showing, never for running: the program and arguments on one line, quoted where a person would need quotes.
pub fn display_command(program: &Path, args: &[OsString]) -> String {
    std::iter::once(program.as_os_str()).chain(args.iter().map(OsString::as_os_str)).map(display_arg).collect::<Vec<_>>().join(" ")
}

fn display_arg(arg: &OsStr) -> String {
    let text = arg.to_string_lossy();
    if text.is_empty() || text.contains(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '&' | '%' | '(' | ')' | '[' | ']')) {
        format!("\"{}\"", text.replace('"', "\\\""))
    } else {
        text.into_owned()
    }
}

// ---- the guard rails --------------------------------------------------------------------------------

/// A path as a Claude Code permission rule writes an absolute one: `//` and then the path with `/` separators,
/// and on Windows `C:\Users\me` as `//c/Users/me`. `None` for a path that can't be written that way (a network
/// share, or one with characters that mean something in a pattern).
pub fn rule_path(path: &str) -> Option<String> {
    let path = path.strip_prefix(r"\\?\").unwrap_or(path);
    if path.contains(['*', '?', '[', ']', '!', '{', '}']) || path.starts_with(r"\\") || path.starts_with("//") {
        return None;
    }
    let bytes = path.as_bytes();
    if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/') {
        let rest = path[3..].replace('\\', "/");
        return Some(format!("//{}/{}", (bytes[0] as char).to_ascii_lowercase(), rest.trim_end_matches('/')));
    }
    if path.starts_with('/') {
        return Some(format!("/{}", path.trim_end_matches('/')));
    }
    None
}

/// The settings file passed with `--settings` when the guard rails are on: the vault's `.ledger/` folder and every
/// `.env` are off limits to the file tools, and reads and writes outside the project folder are refused (this also
/// makes the always-allowed read-only shell commands such as `cat` ask first, which a non-interactive run can't
/// answer, so they fail). It is rewritten before every run and shown in the confirmation, so nothing is hidden.
pub fn guard_settings(vault_root: &Path) -> String {
    let mut deny = Vec::new();
    if let Some(ledger) = rule_path(&vault_root.join(LEDGER_DIR).display().to_string()) {
        deny.push(format!("Read({ledger}/**)"));
        deny.push(format!("Edit({ledger}/**)"));
    }
    deny.push("Read(**/.env)".to_string());
    deny.push("Read(**/.env.*)".to_string());
    let value = json!({
        "permissions": {
            "deny": deny,
            "blockReadsOutsideWorkingDirectories": true,
            "blockWritesOutsideWorkingDirectories": true,
        }
    });
    serde_json::to_string_pretty(&value).unwrap_or_default()
}
