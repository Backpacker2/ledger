//! Keeping secrets out of what a run shows and keeps. The run's output and the note kept about it are built from
//! what a model printed and what a prompt said, and neither can be trusted not to contain a token: so everything is passed
//! through here first. It removes the values in the vault's own `.ledger/.env` (the GitHub token, the encryption
//! key), anything shaped like a well-known access token, `Bearer` tokens, and the value of `NAME=value` where the
//! name says it is a secret. It is a net, not a guarantee: the real protection is that the run is never given the
//! vault's secrets to read (see `cli::guard_settings`).

use std::fs;
use std::path::Path;

use crate::settings::secrets_path;

pub const MASK: &str = "[redacted]";

/// Token prefixes, and how many token characters must follow one for it to count.
const PREFIXES: &[(&str, usize)] = &[
    ("github_pat_", 20),
    ("ghp_", 20),
    ("gho_", 20),
    ("ghu_", 20),
    ("ghs_", 20),
    ("ghr_", 20),
    ("sk-ant-", 16),
    ("sk-", 24),
    ("xoxb-", 16),
    ("xoxp-", 16),
    ("xoxa-", 16),
    ("AKIA", 16),
    ("ledger-key-1:", 12),
];

/// Names (upper-cased) that mark `NAME=value` as holding a secret.
const SECRET_NAMES: &[&str] = &["TOKEN", "SECRET", "PASSWORD", "PASSWD", "API_KEY", "APIKEY", "PRIVATE_KEY", "ACCESS_KEY"];

#[derive(Debug, Clone, Default)]
pub struct Redactor {
    secrets: Vec<String>,
}

impl Redactor {
    /// Knows these exact values, as well as the shapes above. Values too short to be a secret are ignored: masking
    /// every "true" or "main" would make the output unreadable.
    pub fn new(secrets: impl IntoIterator<Item = String>) -> Redactor {
        let mut secrets: Vec<String> = secrets.into_iter().filter(|s| s.chars().count() >= 8).collect();
        secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
        secrets.dedup();
        Redactor { secrets }
    }

    /// Knows every value in the vault's `.ledger/.env`.
    pub fn for_vault(root: &Path) -> Redactor {
        let text = fs::read_to_string(secrets_path(root)).unwrap_or_default();
        Redactor::new(text.lines().filter_map(env_value))
    }

    pub fn apply(&self, text: &str) -> String {
        let mut out = text.to_string();
        for secret in &self.secrets {
            out = out.replace(secret.as_str(), MASK);
        }
        scrub_assignments(&scrub_tokens(&out))
    }
}

/// The value of a `NAME=value` line of an env file.
fn env_value(line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (_, value) = line.strip_prefix("export ").unwrap_or(line).split_once('=')?;
    let value = value.trim();
    let value = ['"', '\''].iter().find_map(|q| value.strip_prefix(*q).and_then(|v| v.strip_suffix(*q))).unwrap_or(value);
    Some(value.to_string())
}

fn is_token_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'+' | b'/' | b'=' | b'.')
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Replaces tokens that start with a known prefix, and `Bearer` credentials.
fn scrub_tokens(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let (mut i, mut copied) = (0, 0);
    while i < bytes.len() {
        // A token starts at a word boundary: the "sk-" inside "task-name" is not one.
        let boundary = i == 0 || !is_word_byte(bytes[i - 1]);
        let mut matched = None;
        if boundary {
            let rest = &bytes[i..];
            if rest.len() >= 7 && rest[..7].eq_ignore_ascii_case(b"bearer ") {
                matched = Some((7, 16));
            } else {
                matched = PREFIXES.iter().find(|(prefix, _)| rest.starts_with(prefix.as_bytes())).map(|(prefix, min)| (prefix.len(), *min));
            }
        }
        if let Some((prefix_len, min)) = matched {
            let body = bytes[i + prefix_len..].iter().take_while(|b| is_token_char(**b)).count();
            if body >= min {
                out.push_str(&text[copied..i]);
                // A "Bearer " stays readable; only what follows it is hidden.
                let keep = if bytes[i..].len() >= 7 && bytes[i..i + 7].eq_ignore_ascii_case(b"bearer ") { prefix_len } else { 0 };
                out.push_str(&text[i..i + keep]);
                out.push_str(MASK);
                i += prefix_len + body;
                copied = i;
                continue;
            }
        }
        i += 1;
    }
    out.push_str(&text[copied..]);
    out
}

/// Replaces the value in `NAME=value` when NAME is a secret's name (GITHUB_TOKEN, DB_PASSWORD, SOME_API_KEY, ...).
fn scrub_assignments(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    for (eq, _) in text.match_indices('=') {
        if eq < copied {
            continue;
        }
        let name_start = bytes[..eq].iter().rposition(|b| !is_word_byte(*b)).map_or(0, |p| p + 1);
        let name = &text[name_start..eq];
        let upper = name.to_ascii_uppercase();
        let secret_name = name.len() >= 3 && (SECRET_NAMES.iter().any(|n| upper.contains(n)) || upper.ends_with("_KEY"));
        if !secret_name {
            continue;
        }
        let mut value_start = eq + 1;
        if matches!(bytes.get(value_start), Some(b'"' | b'\'')) {
            value_start += 1;
        }
        let value_len = bytes[value_start..].iter().take_while(|b| !b.is_ascii_whitespace() && !matches!(**b, b'"' | b'\'')).count();
        if value_len >= 4 && &text[value_start..value_start + value_len] != MASK {
            out.push_str(&text[copied..value_start]);
            out.push_str(MASK);
            copied = value_start + value_len;
        }
    }
    out.push_str(&text[copied..]);
    out
}
