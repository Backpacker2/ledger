//! Markdown files with a YAML frontmatter block, the one file format of the vault:
//!
//! ```text
//! ---
//! id: 1790847517702-ab12cd34
//! createdAt: 2026-10-16T09:18:22.123Z
//! ---
//!
//! The markdown body.
//! ```
//!
//! Reading is lenient (people edit these files by hand, in Obsidian or anywhere else): a number or boolean
//! where text was expected still reads as text, and missing keys read as empty. Writing is canonical.

use serde_yaml_ng::{Mapping, Value};

pub struct Parsed {
    pub meta: Mapping,
    pub body: String,
}

/// Splits a file into its frontmatter and body. `None` when the file has no frontmatter block (or one that
/// isn't a YAML mapping), so callers can fall back to treating the whole file as the body.
pub fn parse(text: &str) -> Option<Parsed> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let rest = text.strip_prefix("---\r\n").or_else(|| text.strip_prefix("---\n"))?;

    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']) == "---" {
            let meta = match serde_yaml_ng::from_str::<Value>(&rest[..offset]).ok()? {
                Value::Mapping(map) => map,
                Value::Null => Mapping::new(),
                _ => return None,
            };
            let body = &rest[offset + line.len()..];
            let body = body.strip_prefix("\r\n").or_else(|| body.strip_prefix('\n')).unwrap_or(body);
            return Some(Parsed { meta, body: body.trim_end().to_string() });
        }
        offset += line.len();
    }
    None
}

/// The canonical text of a file: keys in the order they were set, one blank line, the body, a final newline.
pub fn render(meta: &Mapping, body: &str) -> String {
    let yaml = serde_yaml_ng::to_string(&Value::Mapping(meta.clone())).unwrap_or_default();
    let mut out = String::with_capacity(yaml.len() + body.len() + 16);
    out.push_str("---\n");
    out.push_str(&yaml);
    if !yaml.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("---\n");
    let body = body.trim_end();
    if !body.is_empty() {
        out.push('\n');
        out.push_str(body);
        out.push('\n');
    }
    out
}

// ---- reading ----------------------------------------------------------------------------------------

fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

pub trait MetaExt {
    /// A scalar as text; empty text counts as absent.
    fn text(&self, key: &str) -> Option<String>;
    fn flag(&self, key: &str) -> bool;
    /// A list of scalars; a single scalar reads as a list of one.
    fn list(&self, key: &str) -> Vec<String>;
    /// A list of mappings (acceptance criteria, planboard columns).
    fn maps(&self, key: &str) -> Vec<&Mapping>;
}

impl MetaExt for Mapping {
    fn text(&self, key: &str) -> Option<String> {
        self.get(key).and_then(scalar).filter(|s| !s.is_empty())
    }

    fn flag(&self, key: &str) -> bool {
        match self.get(key) {
            Some(Value::Bool(b)) => *b,
            Some(Value::String(s)) => s.eq_ignore_ascii_case("true"),
            _ => false,
        }
    }

    fn list(&self, key: &str) -> Vec<String> {
        match self.get(key) {
            Some(Value::Sequence(items)) => items.iter().filter_map(scalar).filter(|s| !s.is_empty()).collect(),
            Some(single) => scalar(single).filter(|s| !s.is_empty()).into_iter().collect(),
            None => vec![],
        }
    }

    fn maps(&self, key: &str) -> Vec<&Mapping> {
        match self.get(key) {
            Some(Value::Sequence(items)) => items.iter().filter_map(Value::as_mapping).collect(),
            _ => vec![],
        }
    }
}

// ---- writing ----------------------------------------------------------------------------------------

pub fn set(meta: &mut Mapping, key: &str, value: impl Into<Value>) {
    meta.insert(Value::String(key.to_string()), value.into());
}

/// Empty and absent values are left out so files stay short.
pub fn set_opt(meta: &mut Mapping, key: &str, value: Option<&str>) {
    if let Some(value) = value.filter(|v| !v.is_empty()) {
        set(meta, key, value);
    }
}

pub fn set_flag(meta: &mut Mapping, key: &str, on: bool) {
    if on {
        set(meta, key, true);
    }
}

pub fn set_list(meta: &mut Mapping, key: &str, items: &[String]) {
    if !items.is_empty() {
        set(meta, key, Value::Sequence(items.iter().map(|s| Value::String(s.clone())).collect()));
    }
}
