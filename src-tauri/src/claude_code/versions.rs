//! What a run did to the project's deliverables, and so what is to be published for it.
//!
//! The deliverables are looked at before the run starts ([`Baseline::capture`]) and again when it is over
//! ([`Baseline::changes`]), so it doesn't matter how the run wrote a file. What a change means for the versions:
//!
//! * a file that wasn't there is new: it is published as the first version and that version is linked to the prompt;
//! * a file that was there, and is the same as its latest version, was edited: the edit is published as the next
//!   minor version and that is linked;
//! * a file that was there with edits no version holds (it was never published, or was edited after the latest
//!   version) first has those edits published as a version, from what was read before the run; then the run's edit is
//!   published as the next minor and that one is linked. The version made to keep the earlier edits is not linked.
//!
//! A file that has never been published and holds nothing but a title (what "Create deliverable" starts one with) is
//! a new deliverable that has yet to be written, so it counts as new rather than having a stub published.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::Path;

use super::prompt::list_files;
use crate::deliverables::{parse_version, split_version};

/// A file this big is not followed: a run writes text, and holding one to keep earlier edits would cost too much.
const MAX_TRACKED_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Before {
    /// The same as its latest published version: nothing is lost if the run changes it.
    Published,
    /// Edits that no version holds, or never published at all. Kept so they can be published first.
    Unpublished(Vec<u8>),
    /// Never published, and nothing or only a title: a deliverable still to be written.
    Placeholder,
}

#[derive(Debug, Clone)]
struct Seen {
    hash: u64,
    before: Before,
}

/// The live deliverables as they were before the run.
#[derive(Debug, Clone, Default)]
pub struct Baseline {
    seen: BTreeMap<String, Seen>,
    /// Too big to follow (or unreadable): left alone.
    skipped: BTreeSet<String>,
}

/// What the run did to one deliverable, by its live path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// A file that wasn't there, or was only a placeholder.
    New(String),
    /// A file that was there and the run changed. `held` is what it said before when no version had that.
    Edited { path: String, held: Option<Vec<u8>> },
}

impl Change {
    pub fn path(&self) -> &str {
        match self {
            Change::New(path) | Change::Edited { path, .. } => path,
        }
    }

    /// The earlier edits to publish first, if the file had any that no version held.
    pub fn held(&self) -> Option<&[u8]> {
        match self {
            Change::Edited { held, .. } => held.as_deref(),
            Change::New(_) => None,
        }
    }
}

fn read_capped(file: &Path) -> Option<Vec<u8>> {
    let meta = fs::metadata(file).ok()?;
    if !meta.is_file() || meta.len() > MAX_TRACKED_BYTES {
        return None;
    }
    fs::read(file).ok()
}

fn hash(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// Nothing at all, or a single heading line.
fn is_placeholder(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes);
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    match (lines.next(), lines.next()) {
        (None, _) => true,
        (Some(only), None) => only.starts_with('#'),
        _ => false,
    }
}

/// For each live path with published versions, the path of the latest of them.
fn latest_versions(files: &BTreeSet<String>) -> HashMap<String, String> {
    let mut latest: HashMap<String, ((u32, u32), String)> = HashMap::new();
    for path in files {
        if let (base, Some(version)) = split_version(path)
            && let Some(number) = parse_version(&version)
            && latest.get(&base).is_none_or(|(known, _)| number > *known)
        {
            latest.insert(base, (number, path.clone()));
        }
    }
    latest.into_iter().map(|(base, (_, path))| (base, path)).collect()
}

impl Baseline {
    /// Reads the deliverables folder: which files there are, and for each whether a version holds what it says.
    pub fn capture(dir: &Path) -> Baseline {
        let files = list_files(dir);
        let latest = latest_versions(&files);
        let mut baseline = Baseline::default();
        for path in files.iter().filter(|p| split_version(p).1.is_none()) {
            let Some(bytes) = read_capped(&dir.join(path)) else {
                baseline.skipped.insert(path.clone());
                continue;
            };
            let before = match latest.get(path) {
                Some(snapshot) if read_capped(&dir.join(snapshot)).is_some_and(|published| published == bytes) => Before::Published,
                Some(_) => Before::Unpublished(bytes.clone()),
                None if is_placeholder(&bytes) => Before::Placeholder,
                None => Before::Unpublished(bytes.clone()),
            };
            baseline.seen.insert(path.clone(), Seen { hash: hash(&bytes), before });
        }
        baseline
    }

    /// What the run changed, by path.
    pub fn changes(&self, dir: &Path) -> Vec<Change> {
        let mut changes = Vec::new();
        for path in list_files(dir).into_iter().filter(|p| split_version(p).1.is_none()) {
            let Some(seen) = self.seen.get(&path) else {
                if !self.skipped.contains(&path) {
                    changes.push(Change::New(path));
                }
                continue;
            };
            let Some(now) = read_capped(&dir.join(&path)) else { continue };
            if hash(&now) == seen.hash {
                continue;
            }
            changes.push(match &seen.before {
                Before::Placeholder => Change::New(path),
                Before::Published => Change::Edited { path, held: None },
                Before::Unpublished(earlier) => Change::Edited { path, held: Some(earlier.clone()) },
            });
        }
        changes
    }
}
