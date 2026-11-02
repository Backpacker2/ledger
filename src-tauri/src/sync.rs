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
//!
//! With encryption on, the remote is wrapped in `encrypted_remote.rs`, and what GitHub stores no longer hashes like
//! the file on disk. So the state keeps two shas per path, each compared with its own side: the plain sha with
//! the vault, the sha of the stored blob with the remote's tree. The table above is unchanged.
//!
//! Encryption never converts a branch that already holds plain files: their history would stay on GitHub, and it
//! would be too easy to believe the vault was encrypted when it wasn't. It starts on a branch that doesn't exist
//! yet (created as a fresh root commit with nothing behind it), or carries on with one that was encrypted from
//! the start. Anything else is refused, with what to do instead.

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

use crate::crypto;
use crate::error::{Error, Result};
use crate::ids;

const STATE_FILE: &str = ".ledger/sync-state.json";
const CONFLICTS_DIR: &str = ".ledger/conflicts";
const MAX_ATTEMPTS: usize = 3;
pub const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;
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
    /// The contents of the file at `path` that is stored as the blob `sha`. (The path matters to a remote that
    /// encrypts: it is part of what is authenticated.)
    fn blob(&self, path: &str, sha: &str) -> Result<Vec<u8>>;
    /// Commits the changes on top of `parent` (or as the branch's first commit) and moves the branch.
    /// `Error::Rejected` if the branch is no longer at `parent`.
    fn push(&self, parent: Option<&Head>, changes: &[Change], message: &str) -> Result<Head>;

    /// The id of the key this remote encrypts with. `None` for a remote that stores files as they are.
    fn key_id(&self) -> Option<u8> {
        None
    }
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
    /// path → blob sha of every file in that agreement, as the vault holds it. This is what local files are
    /// compared with.
    pub files: BTreeMap<String, String>,
    /// path → blob sha of what GitHub stores for the file, compared with the remote's tree. Only kept while files
    /// are encrypted, since then it can't be worked out from their contents. Without encryption it is `files`.
    ///
    /// Both this and `encrypted` are new: a state file from before them loads as an unencrypted agreement, which
    /// is exactly what it was.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub remote: BTreeMap<String, String>,
    /// The id of the key the remote's files are encrypted with, once they all are.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted: Option<u8>,
}

impl State {
    /// What the remote held for each path at the last agreement.
    fn remote_files(&self) -> &BTreeMap<String, String> {
        if self.encrypted.is_some() { &self.remote } else { &self.files }
    }

    /// Whether this machine has been syncing this branch before.
    fn has_history(&self) -> bool {
        self.head.is_some() || !self.files.is_empty()
    }
}

/// Whether this vault has been syncing this repository and branch without encryption: the branch then holds its
/// plain files, and keeps them in its history.
pub fn has_plain_history(root: &Path, repo: &str, branch: &str) -> bool {
    load_state(root).is_some_and(|s| s.repo == repo && s.branch == branch && s.encrypted.is_none() && s.has_history())
}

/// Why encryption can't start on a branch that already holds plain files, and what to do instead.
pub fn fresh_start_needed(repo: &str, branch: &str) -> String {
    format!(
        "{repo} on {branch} already holds this vault's files unencrypted, and its history would keep them. Encryption starts on a new repository, or on a branch that doesn't exist yet (Ledger creates it with nothing behind it): change the repository or the branch under Sync to GitHub. Nothing was changed."
    )
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

/// What both sides agreed on at the last sync, as each of them sees it. Without encryption the two are the same.
struct Base<'a> {
    /// path → sha of the file's contents: compared with the vault.
    local: &'a BTreeMap<String, String>,
    /// path → sha of the stored blob: compared with the remote's tree.
    remote: &'a BTreeMap<String, String>,
}

#[derive(Default)]
struct Plan {
    /// Paths where both sides already agree, with the sha here and the sha there.
    synced: BTreeMap<String, (String, String)>,
    download: BTreeMap<String, String>,
    remove_local: BTreeSet<String>,
    put: BTreeSet<String>,
    remove_remote: BTreeSet<String>,
    /// Both sides edited: the remote blob that would be lost if it were simply overwritten.
    conflicts: BTreeMap<String, String>,
}

/// With `encrypted`, the shas on the two sides can't be compared with each other, so each is compared with its own
/// side's base. A file both sides changed may still have ended up the same; that only shows once the remote copy
/// has been downloaded, which is done for conflicts anyway (see `run`).
fn plan(base: Base, local: &BTreeMap<String, LocalFile>, remote: &BTreeMap<String, String>, encrypted: bool) -> Plan {
    let paths: BTreeSet<&String> = base.local.keys().chain(base.remote.keys()).chain(local.keys()).chain(remote.keys()).collect();
    let mut plan = Plan::default();

    for path in paths {
        let (bl, br) = (base.local.get(path), base.remote.get(path));
        let l = local.get(path).map(|f| &f.sha);
        let r = remote.get(path);

        if l.is_none() && r.is_none() {
            continue;
        }
        let agree = if encrypted { l == bl && r == br } else { l == r };
        if agree {
            if let (Some(l), Some(r)) = (l, r) {
                plan.synced.insert(path.clone(), (l.clone(), r.clone()));
            }
        } else if l == bl {
            // Untouched here: whatever the remote did wins.
            match r {
                Some(sha) => {
                    plan.download.insert(path.clone(), sha.clone());
                }
                None => {
                    plan.remove_local.insert(path.clone());
                }
            }
        } else if r == br {
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

/// Refuses to go on when what was synced before doesn't match how this machine is set up now.
fn check_encryption(state: &State, key_id: Option<u8>) -> Result<()> {
    match (state.encrypted, key_id) {
        (Some(was), None) => Err(Error::sync(format!(
            "This vault was synced with encryption on (key {was:02x}), and encryption is off now. Turn it back on in Settings to keep syncing. Nothing was changed."
        ))),
        (Some(was), Some(now)) if was != now => Err(Error::sync(format!(
            "This vault was synced with key {was:02x}, but the key on this machine is {now:02x}. Import the key that was used (Settings). Nothing was changed."
        ))),
        _ => Ok(()),
    }
}

/// One full reconcile of the vault at `root` with the remote branch. Retries from scratch if the remote moved
/// while the push was being prepared.
pub fn run(root: &Path, repo: &str, branch: &str, remote: &dyn Remote, applier: &dyn Applier) -> Result<Outcome> {
    let mut state = load_state(root).filter(|s| s.repo == repo && s.branch == branch).unwrap_or_else(|| State {
        repo: repo.to_string(),
        branch: branch.to_string(),
        ..State::default()
    });

    let key_id = remote.key_id();
    let encrypted = key_id.is_some();
    check_encryption(&state, key_id)?;

    let mut totals = Outcome::default();
    for attempt in 0..MAX_ATTEMPTS {
        let head = remote.head()?;

        // A branch that was there at the last sync and is gone now is not a request to delete everything here.
        if head.is_none() && state.head.is_some() {
            state.head = None;
            state.files.clear();
            state.remote.clear();
        }

        // A branch that already holds this vault's plain files can't become an encrypted one: those files, and
        // their history, would stay on GitHub, and the vault would only look encrypted. Nothing is read or sent.
        if encrypted && state.encrypted.is_none() && state.has_history() {
            return Err(Error::sync(fresh_start_needed(repo, branch)));
        }

        let ignore = Ignore::load(root);
        let remote_files: BTreeMap<String, String> = match &head {
            None => BTreeMap::new(),
            Some(h) if state.head.as_deref() == Some(h.commit.as_str()) => state.remote_files().clone(),
            Some(h) => remote.tree(h)?.into_iter().filter(|(path, _)| !ignore.is_ignored(path, false)).collect(),
        };
        let local = scan(root, &ignore)?;
        let mut plan = plan(Base { local: &state.files, remote: state.remote_files() }, &local, &remote_files, encrypted);

        // Download what is needed first, so that a network failure, or a file that won't decrypt, leaves the
        // vault untouched: nothing below runs until every file has been fetched and opened.
        let wanted: BTreeMap<String, String> = plan.download.iter().chain(&plan.conflicts).map(|(p, s)| (p.clone(), s.clone())).collect();
        let blobs = fetch_all(remote, &wanted)?;

        if !encrypted && let Some(path) = wanted.keys().find(|path| crypto::is_encrypted(&blobs[*path])) {
            return Err(Error::sync(format!(
                "{path} on GitHub is encrypted, but encryption is off here. Turn encryption on and import the key (Settings), then sync again. Nothing was changed."
            )));
        }

        // Both sides may have changed a file to the same thing; with encryption that only shows now.
        let converged: Vec<String> = plan.conflicts.keys().filter(|path| blob_sha(&blobs[*path]) == local[*path].sha).cloned().collect();
        for path in converged {
            if let Some(stored) = plan.conflicts.remove(&path) {
                plan.put.remove(&path);
                plan.synced.insert(path.clone(), (local[&path].sha.clone(), stored));
            }
        }

        if !plan.conflicts.is_empty() {
            let stamp = ids::now().replace(':', "-");
            for path in plan.conflicts.keys() {
                let file = root.join(CONFLICTS_DIR).join(&stamp).join(path);
                if let Some(dir) = file.parent() {
                    fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
                }
                fs::write(&file, &blobs[path]).map_err(|e| Error::io(&file, e))?;
            }
        }

        let mut ops: Vec<LocalOp> = vec![];
        for path in plan.download.keys() {
            ops.push(LocalOp::Write {
                path: path.clone(),
                bytes: blobs[path].clone(),
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
        // What GitHub stores for the files just pushed. Encryption makes that unknowable from here, so ask.
        let mut uploaded: BTreeMap<String, String> = BTreeMap::new();
        if !changes.is_empty() {
            let message = format!("Ledger sync: {} updated, {} removed", plan.put.len(), plan.remove_remote.len());
            match remote.push(head.as_ref(), &changes, &message) {
                Ok(pushed) => {
                    if encrypted {
                        // If this fails the push has still happened; the next round finds both sides changed to the
                        // same thing and settles it without pushing again.
                        uploaded = remote.tree(&pushed)?;
                    }
                    new_head = Some(pushed.commit);
                }
                Err(Error::Rejected) if attempt + 1 < MAX_ATTEMPTS => {
                    // Whatever this round applied locally already matches the remote, so the next one starts clean.
                    accumulate(&mut totals, &plan, &skipped, false);
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
        let stored_sha = |path: &String, plain: &String| -> Result<String> {
            if !encrypted {
                return Ok(plain.clone());
            }
            uploaded.get(path).cloned().ok_or_else(|| Error::sync("GitHub did not list a file that was just uploaded"))
        };

        // What both sides now agree on, as each of them sees it.
        let mut files: BTreeMap<String, String> = BTreeMap::new();
        let mut shas: BTreeMap<String, String> = BTreeMap::new();
        for (path, (plain, there)) in &plan.synced {
            files.insert(path.clone(), plain.clone());
            shas.insert(path.clone(), there.clone());
        }
        for (path, there) in &plan.download {
            if skipped.contains(path) {
                if let Some(old) = state.files.get(path) {
                    files.insert(path.clone(), old.clone());
                }
                if let Some(old) = state.remote_files().get(path) {
                    shas.insert(path.clone(), old.clone());
                }
            } else {
                files.insert(path.clone(), if encrypted { blob_sha(&blobs[path]) } else { there.clone() });
                shas.insert(path.clone(), there.clone());
            }
        }
        for path in &plan.remove_local {
            if skipped.contains(path) {
                if let Some(old) = state.files.get(path) {
                    files.insert(path.clone(), old.clone());
                }
                if let Some(old) = state.remote_files().get(path) {
                    shas.insert(path.clone(), old.clone());
                }
            }
        }
        for path in &plan.put {
            files.insert(path.clone(), local[path].sha.clone());
            shas.insert(path.clone(), stored_sha(path, &local[path].sha)?);
        }

        // If a file was left alone, `files` no longer describes exactly what the head holds, so the next round
        // must read the remote's tree instead of assuming it is unchanged.
        state.head = if skipped.is_empty() { new_head } else { None };
        // Once the remote holds only sealed files, this vault's state says so (and with which key).
        if encrypted {
            state.encrypted = key_id;
        }
        state.files = files;
        state.remote = if state.encrypted.is_some() { shas } else { BTreeMap::new() };
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

/// Downloads blobs a few at a time: path → the file's contents (decrypted, if the remote encrypts).
fn fetch_all(remote: &dyn Remote, wanted: &BTreeMap<String, String>) -> Result<HashMap<String, Vec<u8>>> {
    // Identical files are fetched once, unless they are sealed: each of those is opened for its own path, so
    // that a blob copied to another path is noticed instead of passing for the file that belongs there.
    let share = remote.key_id().is_none();
    let mut first: HashMap<&str, &str> = HashMap::new();
    let mut jobs: Vec<(&str, &str)> = vec![];
    for (path, sha) in wanted {
        if share && first.contains_key(sha.as_str()) {
            continue;
        }
        first.insert(sha.as_str(), path.as_str());
        jobs.push((path.as_str(), sha.as_str()));
    }

    let next = AtomicUsize::new(0);
    let results: Mutex<HashMap<String, Vec<u8>>> = Mutex::new(HashMap::new());
    let failure: Mutex<Option<Error>> = Mutex::new(None);

    std::thread::scope(|scope| {
        for _ in 0..DOWNLOAD_WORKERS.min(jobs.len()) {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&(path, sha)) = jobs.get(i) else { return };
                    if failure.lock().unwrap().is_some() {
                        return;
                    }
                    match remote.blob(path, sha) {
                        Ok(bytes) => {
                            results.lock().unwrap().insert(path.to_string(), bytes);
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

    if let Some(e) = failure.into_inner().unwrap() {
        return Err(e);
    }
    let mut results = results.into_inner().unwrap();
    if share {
        for (path, sha) in wanted {
            if !results.contains_key(path) {
                let bytes = results[first[sha.as_str()]].clone();
                results.insert(path.clone(), bytes);
            }
        }
    }
    Ok(results)
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
