//! Keeping the vault and a GitHub branch the same, without needing git installed.
//!
//! Each round is a three-way comparison per file between *base* (what both sides agreed on at the last sync,
//! kept in `.ledger/sync-state.json`), *local* (the vault on disk) and *remote* (the branch's tree):
//!
//! | local vs base | remote vs base | what happens                                           |
//! |---------------|----------------|--------------------------------------------------------|
//! | same          | changed        | the remote version is downloaded (or the file removed)  |
//! | changed       | same           | the local version is pushed (or the file removed there) |
//! | changed       | changed        | see below                                               |
//!
//! When both sides changed a file the edit is never lost: if one side deleted it, the side that edited it wins;
//! if both edited it, the local version wins and the remote one is kept in `.ledger/conflicts/`.
//!
//! The remote is a trait so the algorithm can be tested against an in-memory fake; `github.rs` is the real one.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

use crate::error::{Error, Result};
use crate::ids;

const STATE_FILE: &str = ".ledger/sync-state.json";
const CONFLICTS_DIR: &str = ".ledger/conflicts";
const MAX_ATTEMPTS: usize = 3;
const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;
const DOWNLOAD_WORKERS: usize = 6;

// ---- the remote -------------------------------------------------------------------------------------

/// A commit on the synced branch and the tree it points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    pub commit: String,
    pub tree: String,
}

pub enum Change {
    Put { path: String, bytes: Vec<u8> },
    Delete { path: String },
}

pub trait Remote: Sync {
    /// The branch's current commit, or `None` if the branch does not exist yet.
    fn head(&self) -> Result<Option<Head>>;
    /// Every file in the commit's tree: path → git blob sha.
    fn tree(&self, head: &Head) -> Result<BTreeMap<String, String>>;
    fn blob(&self, sha: &str) -> Result<Vec<u8>>;
    /// Commits the changes on top of `parent` (or as the branch's first commit) and moves the branch.
    /// `Error::Rejected` if the branch is no longer at `parent`.
    fn push(&self, parent: Option<&Head>, changes: &[Change], message: &str) -> Result<Head>;
}

/// Something that may write to the vault's files on this machine. The app's version holds the vault lock and
/// reloads the in-memory database afterwards; each op carries the sha it expects the file to have right now,
/// and an op whose file has changed since is skipped (and reported) instead of overwriting the newer edit.
pub trait Applier {
    fn apply(&self, ops: Vec<LocalOp>) -> Result<Vec<String>>;
}

pub enum LocalOp {
    Write { path: String, bytes: Vec<u8>, expect: Option<String> },
    Delete { path: String, expect: Option<String> },
}

impl LocalOp {
    pub fn path(&self) -> &str {
        match self {
            LocalOp::Write { path, .. } | LocalOp::Delete { path, .. } => path,
        }
    }

    pub fn expect(&self) -> Option<&str> {
        match self {
            LocalOp::Write { expect, .. } | LocalOp::Delete { expect, .. } => expect.as_deref(),
        }
    }
}

// ---- state and results ------------------------------------------------------------------------------

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct State {
    pub repo: String,
    pub branch: String,
    /// The commit both sides last agreed on.
    pub head: Option<String>,
    /// path → blob sha of every file in that agreement.
    pub files: BTreeMap<String, String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub pulled: usize,
    pub pushed: usize,
    pub removed_here: usize,
    pub removed_there: usize,
    pub conflicts: Vec<String>,
    /// Files left alone because they changed while the sync was running; the next round deals with them.
    pub skipped: usize,
}

impl Outcome {
    pub fn changed_local(&self) -> bool {
        self.pulled + self.removed_here > 0
    }

    pub fn summary(&self) -> String {
        let mut parts = vec![];
        if self.pulled + self.removed_here > 0 {
            parts.push(format!("pulled {}", self.pulled + self.removed_here));
        }
        if self.pushed + self.removed_there > 0 {
            parts.push(format!("pushed {}", self.pushed + self.removed_there));
        }
        if !self.conflicts.is_empty() {
            parts.push(format!("{} conflict(s) resolved in favour of this machine", self.conflicts.len()));
        }
        if parts.is_empty() { "Up to date".to_string() } else { capitalize(&parts.join(", ")) }
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map_or_else(String::new, |c| c.to_uppercase().collect::<String>() + chars.as_str())
}

pub fn load_state(root: &Path) -> Option<State> {
    serde_json::from_str(&fs::read_to_string(root.join(STATE_FILE)).ok()?).ok()
}

fn save_state(root: &Path, state: &State) -> Result<()> {
    let file = root.join(STATE_FILE);
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    }
    fs::write(&file, serde_json::to_string(state).unwrap_or_default()).map_err(|e| Error::io(&file, e))
}

// ---- files ------------------------------------------------------------------------------------------

/// The sha git gives a file's contents, so local files can be compared with the remote tree without downloading.
pub fn blob_sha(bytes: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(format!("blob {}\0", bytes.len()).as_bytes());
    hasher.update(bytes);
    hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Which paths are part of the sync: everything except what the vault's `.gitignore` lists and the app's own
/// bookkeeping. `.env` is excluded here too, whatever `.gitignore` says: the token must never be uploaded.
pub struct Ignore {
    gitignore: Gitignore,
}

impl Ignore {
    pub fn load(root: &Path) -> Ignore {
        let mut builder = GitignoreBuilder::new(root);
        let _ = builder.add(root.join(".gitignore"));
        Ignore { gitignore: builder.build().unwrap_or_else(|_| Gitignore::empty()) }
    }

    pub fn is_ignored(&self, rel: &str, is_dir: bool) -> bool {
        let always = rel == ".env" || rel == ".git" || rel.starts_with(".git/") || rel == ".ledger" || rel.starts_with(".ledger/");
        always || self.gitignore.matched_path_or_any_parents(rel, is_dir).is_ignore()
    }
}

pub struct LocalFile {
    pub sha: String,
    pub bytes: Vec<u8>,
}

/// Every syncable file in the vault, read into memory.
pub fn scan(root: &Path, ignore: &Ignore) -> Result<BTreeMap<String, LocalFile>> {
    fn walk(root: &Path, rel: &str, ignore: &Ignore, out: &mut BTreeMap<String, LocalFile>) -> Result<()> {
        let dir = if rel.is_empty() { root.to_path_buf() } else { root.join(rel) };
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(Error::io(&dir, e)),
        };
        for entry in entries.flatten() {
            let Some(name) = entry.file_name().to_str().map(str::to_string) else { continue };
            let child = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                if !ignore.is_ignored(&child, true) {
                    walk(root, &child, ignore, out)?;
                }
            } else if kind.is_file() && !ignore.is_ignored(&child, false) && !name.ends_with(".tmp") {
                if entry.metadata().map_or(true, |m| m.len() > MAX_FILE_BYTES) {
                    continue;
                }
                let bytes = fs::read(entry.path()).map_err(|e| Error::io(&entry.path(), e))?;
                out.insert(child, LocalFile { sha: blob_sha(&bytes), bytes });
            }
        }
        Ok(())
    }

    let mut out = BTreeMap::new();
    walk(root, "", ignore, &mut out)?;
    Ok(out)
}

/// The sha of a file as it is on disk right now, `None` if it does not exist.
pub fn local_sha(root: &Path, rel: &str) -> Option<String> {
    fs::read(root.join(rel)).ok().map(|bytes| blob_sha(&bytes))
}

// ---- the round --------------------------------------------------------------------------------------

#[derive(Default)]
struct Plan {
    /// Paths where both sides already agree, with the shared sha.
    synced: BTreeMap<String, String>,
    download: BTreeMap<String, String>,
    remove_local: BTreeSet<String>,
    put: BTreeSet<String>,
    remove_remote: BTreeSet<String>,
    /// Both sides edited: the remote blob that would be lost if it were simply overwritten.
    conflicts: BTreeMap<String, String>,
}

fn plan(base: &BTreeMap<String, String>, local: &BTreeMap<String, LocalFile>, remote: &BTreeMap<String, String>) -> Plan {
    let paths: BTreeSet<&String> = base.keys().chain(local.keys()).chain(remote.keys()).collect();
    let mut plan = Plan::default();

    for path in paths {
        let b = base.get(path);
        let l = local.get(path).map(|f| &f.sha);
        let r = remote.get(path);

        if l == r {
            if let Some(sha) = r {
                plan.synced.insert(path.clone(), sha.clone());
            }
        } else if l == b {
            // Untouched here: whatever the remote did wins.
            match r {
                Some(sha) => {
                    plan.download.insert(path.clone(), sha.clone());
                }
                None => {
                    plan.remove_local.insert(path.clone());
                }
            }
        } else if r == b {
            // Untouched there: what happened here goes up.
            match l {
                Some(_) => {
                    plan.put.insert(path.clone());
                }
                None => {
                    plan.remove_remote.insert(path.clone());
                }
            }
        } else {
            // Both sides changed it, differently. An edit beats a delete; two edits: this machine wins.
            match (l, r) {
                (None, Some(sha)) => {
                    plan.download.insert(path.clone(), sha.clone());
                }
                (Some(_), None) => {
                    plan.put.insert(path.clone());
                }
                (Some(_), Some(sha)) => {
                    plan.conflicts.insert(path.clone(), sha.clone());
                    plan.put.insert(path.clone());
                }
                (None, None) => {}
            }
        }
    }
    plan
}

/// One full reconcile of the vault at `root` with the remote branch. Retries from scratch if the remote moved
/// while the push was being prepared.
pub fn run(root: &Path, repo: &str, branch: &str, remote: &dyn Remote, applier: &dyn Applier) -> Result<Outcome> {
    let mut state = load_state(root).filter(|s| s.repo == repo && s.branch == branch).unwrap_or_else(|| State {
        repo: repo.to_string(),
        branch: branch.to_string(),
        ..State::default()
    });

    let mut totals = Outcome::default();
    for attempt in 0..MAX_ATTEMPTS {
        let head = remote.head()?;

        // A branch that was there at the last sync and is gone now is not a request to delete everything here.
        if head.is_none() && state.head.is_some() {
            state.head = None;
            state.files.clear();
        }

        let ignore = Ignore::load(root);
        let remote_files: BTreeMap<String, String> = match &head {
            None => BTreeMap::new(),
            Some(h) if state.head.as_deref() == Some(h.commit.as_str()) => state.files.clone(),
            Some(h) => remote.tree(h)?.into_iter().filter(|(path, _)| !ignore.is_ignored(path, false)).collect(),
        };
        let local = scan(root, &ignore)?;
        let plan = plan(&state.files, &local, &remote_files);

        // Download what is needed first, so a network failure leaves the vault untouched.
        let mut wanted: Vec<String> = plan.download.values().chain(plan.conflicts.values()).cloned().collect();
        wanted.sort();
        wanted.dedup();
        let blobs = fetch_all(remote, &wanted)?;

        if !plan.conflicts.is_empty() {
            let stamp = ids::now().replace(':', "-");
            for (path, sha) in &plan.conflicts {
                let file = root.join(CONFLICTS_DIR).join(&stamp).join(path);
                if let Some(dir) = file.parent() {
                    fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
                }
                fs::write(&file, &blobs[sha]).map_err(|e| Error::io(&file, e))?;
            }
        }

        let mut ops: Vec<LocalOp> = vec![];
        for (path, sha) in &plan.download {
            ops.push(LocalOp::Write {
                path: path.clone(),
                bytes: blobs[sha].clone(),
                expect: local.get(path).map(|f| f.sha.clone()),
            });
        }
        for path in &plan.remove_local {
            ops.push(LocalOp::Delete { path: path.clone(), expect: local.get(path).map(|f| f.sha.clone()) });
        }
        let skipped: BTreeSet<String> = if ops.is_empty() { BTreeSet::new() } else { applier.apply(ops)?.into_iter().collect() };

        // Then push what this machine has that the remote lacks, as one commit.
        let mut changes: Vec<Change> = plan
            .put
            .iter()
            .map(|path| Change::Put { path: path.clone(), bytes: local[path].bytes.clone() })
            .collect();
        changes.extend(plan.remove_remote.iter().map(|path| Change::Delete { path: path.clone() }));

        let mut new_head = head.as_ref().map(|h| h.commit.clone());
        if !changes.is_empty() {
            let message = format!("Ledger sync: {} updated, {} removed", plan.put.len(), plan.remove_remote.len());
            match remote.push(head.as_ref(), &changes, &message) {
                Ok(pushed) => new_head = Some(pushed.commit),
                Err(Error::Rejected) if attempt + 1 < MAX_ATTEMPTS => {
                    // Whatever this round applied locally already matches the remote, so the next one starts clean.
                    accumulate(&mut totals, &plan, &skipped, false);
                    continue;
                }
                Err(e) => return Err(e),
            }
        }

        // What both sides now agree on.
        let mut files = plan.synced.clone();
        for (path, sha) in &plan.download {
            if skipped.contains(path) {
                if let Some(old) = state.files.get(path) {
                    files.insert(path.clone(), old.clone());
                }
            } else {
                files.insert(path.clone(), sha.clone());
            }
        }
        for path in &plan.remove_local {
            if skipped.contains(path)
                && let Some(old) = state.files.get(path)
            {
                files.insert(path.clone(), old.clone());
            }
        }
        for path in &plan.put {
            files.insert(path.clone(), local[path].sha.clone());
        }
        // If a file was left alone, `files` no longer describes exactly what the head holds, so the next round
        // must read the remote's tree instead of assuming it is unchanged.
        state.head = if skipped.is_empty() { new_head } else { None };
        state.files = files;
        save_state(root, &state)?;

        accumulate(&mut totals, &plan, &skipped, true);
        return Ok(totals);
    }
    Err(Error::sync("GitHub kept changing while syncing; it will be retried shortly"))
}

fn accumulate(totals: &mut Outcome, plan: &Plan, skipped: &BTreeSet<String>, pushed: bool) {
    totals.pulled += plan.download.keys().filter(|p| !skipped.contains(*p)).count();
    totals.removed_here += plan.remove_local.iter().filter(|p| !skipped.contains(*p)).count();
    totals.skipped += skipped.len();
    if pushed {
        totals.pushed += plan.put.len();
        totals.removed_there += plan.remove_remote.len();
        totals.conflicts.extend(plan.conflicts.keys().cloned());
    }
}

/// Downloads blobs a few at a time.
fn fetch_all(remote: &dyn Remote, shas: &[String]) -> Result<HashMap<String, Vec<u8>>> {
    let next = AtomicUsize::new(0);
    let results: Mutex<HashMap<String, Vec<u8>>> = Mutex::new(HashMap::new());
    let failure: Mutex<Option<Error>> = Mutex::new(None);

    std::thread::scope(|scope| {
        for _ in 0..DOWNLOAD_WORKERS.min(shas.len()) {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(sha) = shas.get(i) else { return };
                    if failure.lock().unwrap().is_some() {
                        return;
                    }
                    match remote.blob(sha) {
                        Ok(bytes) => {
                            results.lock().unwrap().insert(sha.clone(), bytes);
                        }
                        Err(e) => {
                            failure.lock().unwrap().get_or_insert(e);
                            return;
                        }
                    }
                }
            });
        }
    });

    match failure.into_inner().unwrap() {
        Some(e) => Err(e),
        None => Ok(results.into_inner().unwrap()),
    }
}

// ---- the plain applier ------------------------------------------------------------------------------

/// Writes straight to disk. The app wraps this with the vault lock and a reload.
#[cfg(test)]
pub struct DiskApplier {
    pub root: PathBuf,
}

#[cfg(test)]
impl Applier for DiskApplier {
    fn apply(&self, ops: Vec<LocalOp>) -> Result<Vec<String>> {
        Ok(apply_to_disk(&self.root, ops)?.0)
    }
}

/// Applies the ops whose files are still as expected. Returns the skipped paths and whether anything was written.
pub fn apply_to_disk(root: &Path, ops: Vec<LocalOp>) -> Result<(Vec<String>, bool)> {
    let mut skipped = vec![];
    let mut wrote = false;
    for op in ops {
        if local_sha(root, op.path()).as_deref() != op.expect() {
            skipped.push(op.path().to_string());
            continue;
        }
        let file = root.join(op.path());
        match op {
            LocalOp::Write { bytes, .. } => {
                if let Some(dir) = file.parent() {
                    fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
                }
                fs::write(&file, bytes).map_err(|e| Error::io(&file, e))?;
            }
            LocalOp::Delete { .. } => {
                fs::remove_file(&file).map_err(|e| Error::io(&file, e))?;
                prune_empty(root, &file);
            }
        }
        wrote = true;
    }
    Ok((skipped, wrote))
}

fn prune_empty(root: &Path, file: &Path) {
    let mut dir = file.parent();
    while let Some(current) = dir {
        if current == root || !current.starts_with(root) || fs::remove_dir(current).is_err() {
            break;
        }
        dir = current.parent();
    }
}
