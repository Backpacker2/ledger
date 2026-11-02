//! The sync algorithm against an in-memory stand-in for a GitHub branch.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Mutex;

use tempfile::tempdir;

use crate::crypto::{self, CryptoError, Key};
use crate::encrypted_remote::EncryptedRemote;
use crate::error::{Error, Result};
use crate::keystore::{EnvFileKeyStore, KeyStore};
use crate::sync::{self, Change, DiskApplier, Head, Outcome, Remote, blob_sha};

/// A branch as a list of commits, each a full snapshot of the files.
#[derive(Default)]
struct Fake {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    commits: Vec<BTreeMap<String, Vec<u8>>>,
    /// Another machine pushes this just before our next push lands, which therefore bounces once.
    race: Option<(String, String)>,
    pushes: usize,
    /// The next `tree` call fails (a connection lost right after a push, say).
    tree_fails: bool,
    blob_requests: usize,
}

impl Inner {
    fn head_files(&self) -> BTreeMap<String, Vec<u8>> {
        self.commits.last().cloned().unwrap_or_default()
    }

    fn commit(&mut self, changes: impl IntoIterator<Item = (String, Option<Vec<u8>>)>) {
        let mut files = self.head_files();
        for (path, bytes) in changes {
            match bytes {
                Some(bytes) => files.insert(path, bytes),
                None => files.remove(&path),
            };
        }
        self.commits.push(files);
    }
}

impl Fake {
    /// Something another machine did.
    fn elsewhere(&self, changes: &[(&str, Option<&str>)]) {
        self.inner.lock().unwrap().commit(changes.iter().map(|(p, t)| (p.to_string(), t.map(|t| t.as_bytes().to_vec()))));
    }

    fn files(&self) -> BTreeMap<String, String> {
        self.inner.lock().unwrap().head_files().into_iter().map(|(p, b)| (p, String::from_utf8(b).unwrap())).collect()
    }

    fn commits(&self) -> usize {
        self.inner.lock().unwrap().commits.len()
    }

    fn pushes(&self) -> usize {
        self.inner.lock().unwrap().pushes
    }

    fn blob_requests(&self) -> usize {
        self.inner.lock().unwrap().blob_requests
    }
}

fn head_of(index: usize) -> Head {
    Head { commit: format!("commit-{index}"), tree: format!("tree-{index}") }
}

fn index_of(head: &Head) -> usize {
    head.commit.strip_prefix("commit-").unwrap().parse().unwrap()
}

impl Remote for Fake {
    fn head(&self) -> Result<Option<Head>> {
        let inner = self.inner.lock().unwrap();
        Ok(inner.commits.len().checked_sub(1).map(head_of))
    }

    fn tree(&self, head: &Head) -> Result<BTreeMap<String, String>> {
        let mut inner = self.inner.lock().unwrap();
        if std::mem::take(&mut inner.tree_fails) {
            return Err(Error::sync("connection lost"));
        }
        Ok(inner.commits[index_of(head)].iter().map(|(p, b)| (p.clone(), blob_sha(b))).collect())
    }

    fn blob(&self, _path: &str, sha: &str) -> Result<Vec<u8>> {
        let mut inner = self.inner.lock().unwrap();
        inner.blob_requests += 1;
        inner
            .commits
            .iter()
            .flat_map(|files| files.values())
            .find(|bytes| blob_sha(bytes) == sha)
            .cloned()
            .ok_or_else(|| Error::sync("no such blob"))
    }

    fn push(&self, parent: Option<&Head>, changes: &[Change], _message: &str) -> Result<Head> {
        let mut inner = self.inner.lock().unwrap();
        if let Some((path, text)) = inner.race.take() {
            inner.commit([(path, Some(text.into_bytes()))]);
            return Err(Error::Rejected);
        }
        let current = inner.commits.len().checked_sub(1);
        if parent.map(index_of) != current {
            return Err(Error::Rejected);
        }
        inner.pushes += 1;
        inner.commit(changes.iter().map(|c| match c {
            Change::Put { path, bytes } => (path.clone(), Some(bytes.clone())),
            Change::Delete { path } => (path.clone(), None),
        }));
        Ok(head_of(inner.commits.len() - 1))
    }
}

fn write(root: &Path, rel: &str, text: &str) {
    let file = root.join(rel);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, text).unwrap();
}

fn read(root: &Path, rel: &str) -> Option<String> {
    fs::read_to_string(root.join(rel)).ok()
}

fn sync(root: &Path, remote: &Fake) -> Outcome {
    sync::run(root, "me/notes", "main", remote, &DiskApplier { root: root.to_path_buf() }).unwrap()
}

fn paths(files: &BTreeMap<String, String>) -> Vec<&str> {
    files.keys().map(String::as_str).collect()
}

#[test]
fn the_first_sync_pushes_everything_and_the_second_has_nothing_to_do() {
    let dir = tempdir().unwrap();
    write(dir.path(), "threads/a/thread.md", "---\ntitle: A\n---\n");
    write(dir.path(), "threads/a/posts/1.md", "hello");
    let remote = Fake::default();

    let first = sync(dir.path(), &remote);
    assert_eq!((first.pushed, first.pulled), (2, 0));
    assert_eq!(remote.files()["threads/a/posts/1.md"], "hello");
    assert_eq!(remote.commits(), 1);

    let second = sync(dir.path(), &remote);
    assert_eq!(second, Outcome::default());
    assert_eq!(second.summary(), "Up to date");
    assert_eq!(remote.commits(), 1, "nothing to push, nothing pushed");
}

#[test]
fn a_new_machine_pulls_everything() {
    let remote = Fake::default();
    remote.elsewhere(&[("threads/a/thread.md", Some("---\ntitle: A\n---\n")), ("threads/a/posts/1.md", Some("hello")), ("README.md", Some("# notes"))]);

    let dir = tempdir().unwrap();
    let outcome = sync(dir.path(), &remote);
    assert_eq!((outcome.pulled, outcome.pushed), (3, 0));
    assert_eq!(read(dir.path(), "threads/a/posts/1.md").unwrap(), "hello");
    assert_eq!(read(dir.path(), "README.md").unwrap(), "# notes");
    assert_eq!(remote.pushes(), 0);
}

#[test]
fn edits_on_both_sides_to_different_files_are_both_kept() {
    let dir = tempdir().unwrap();
    write(dir.path(), "a.md", "a1");
    write(dir.path(), "b.md", "b1");
    let remote = Fake::default();
    sync(dir.path(), &remote);

    write(dir.path(), "a.md", "a2 (here)");
    write(dir.path(), "new-here.md", "new");
    remote.elsewhere(&[("b.md", Some("b2 (there)")), ("new-there.md", Some("new"))]);

    let outcome = sync(dir.path(), &remote);
    assert_eq!((outcome.pulled, outcome.pushed), (2, 2));
    assert!(outcome.conflicts.is_empty());

    for files in [remote.files(), BTreeMap::from_iter(["a.md", "b.md", "new-here.md", "new-there.md"].map(|p| (p.to_string(), read(dir.path(), p).unwrap())))] {
        assert_eq!(files["a.md"], "a2 (here)");
        assert_eq!(files["b.md"], "b2 (there)");
        assert_eq!(paths(&files), ["a.md", "b.md", "new-here.md", "new-there.md"]);
    }
    assert_eq!(sync(dir.path(), &remote), Outcome::default());
}

#[test]
fn the_same_file_edited_on_both_sides_keeps_this_machines_version_and_saves_the_other() {
    let dir = tempdir().unwrap();
    write(dir.path(), "plan.md", "v1");
    let remote = Fake::default();
    sync(dir.path(), &remote);

    write(dir.path(), "plan.md", "v2 from here");
    remote.elsewhere(&[("plan.md", Some("v2 from there"))]);

    let outcome = sync(dir.path(), &remote);
    assert_eq!(outcome.conflicts, ["plan.md"]);
    assert_eq!(remote.files()["plan.md"], "v2 from here");
    assert_eq!(read(dir.path(), "plan.md").unwrap(), "v2 from here");

    // Nothing is lost: the other version is kept under .ledger/conflicts/.
    let conflicts = dir.path().join(".ledger/conflicts");
    let saved: Vec<_> = fs::read_dir(&conflicts).unwrap().map(|e| e.unwrap().path().join("plan.md")).collect();
    assert_eq!(saved.len(), 1);
    assert_eq!(fs::read_to_string(&saved[0]).unwrap(), "v2 from there");
    assert!(outcome.summary().contains("1 conflict"));
}

#[test]
fn deletions_travel_in_both_directions() {
    let dir = tempdir().unwrap();
    write(dir.path(), "threads/t/posts/keep.md", "keep");
    write(dir.path(), "threads/t/posts/gone-here.md", "x");
    write(dir.path(), "threads/t/posts/gone-there.md", "y");
    let remote = Fake::default();
    sync(dir.path(), &remote);

    fs::remove_file(dir.path().join("threads/t/posts/gone-here.md")).unwrap();
    remote.elsewhere(&[("threads/t/posts/gone-there.md", None)]);

    let outcome = sync(dir.path(), &remote);
    assert_eq!((outcome.removed_there, outcome.removed_here), (1, 1));
    assert_eq!(paths(&remote.files()), ["threads/t/posts/keep.md"]);
    assert!(read(dir.path(), "threads/t/posts/gone-there.md").is_none());
    assert!(read(dir.path(), "threads/t/posts/keep.md").is_some());
}

#[test]
fn an_edit_beats_a_delete_whichever_side_made_it() {
    let dir = tempdir().unwrap();
    write(dir.path(), "edited-here.md", "1");
    write(dir.path(), "edited-there.md", "1");
    let remote = Fake::default();
    sync(dir.path(), &remote);

    write(dir.path(), "edited-here.md", "2");
    remote.elsewhere(&[("edited-here.md", None)]);
    fs::remove_file(dir.path().join("edited-there.md")).unwrap();
    remote.elsewhere(&[("edited-there.md", Some("2"))]);

    sync(dir.path(), &remote);
    assert_eq!(remote.files()["edited-here.md"], "2");
    assert_eq!(read(dir.path(), "edited-there.md").unwrap(), "2");
}

#[test]
fn local_only_files_never_leave_and_remote_copies_of_them_are_left_alone() {
    let dir = tempdir().unwrap();
    write(dir.path(), ".gitignore", ".env\n.ledger/\n.obsidian/\nprivate/\n");
    write(dir.path(), ".env", "GITHUB_TOKEN=secret");
    write(dir.path(), ".obsidian/app.json", "{}");
    write(dir.path(), "private/diary.md", "dear diary");
    write(dir.path(), "public.md", "hello");
    let remote = Fake::default();
    // A repository that happens to contain something this vault ignores.
    remote.elsewhere(&[("private/old.md", Some("from long ago")), (".env", Some("not a secret, but not ours"))]);

    sync(dir.path(), &remote);
    sync(dir.path(), &remote);

    let files = remote.files();
    assert_eq!(paths(&files), [".env", ".gitignore", "private/old.md", "public.md"]);
    assert_eq!(files[".env"], "not a secret, but not ours", "the local .env is never uploaded, whatever is up there");
    assert!(!files.values().any(|v| v.contains("secret") && v.contains("TOKEN")));
    assert!(read(dir.path(), "private/old.md").is_none(), "ignored paths are not downloaded either");
    assert_eq!(read(dir.path(), ".env").unwrap(), "GITHUB_TOKEN=secret");
}

#[test]
fn the_token_is_never_uploaded_even_if_gitignore_forgets_it() {
    let dir = tempdir().unwrap();
    write(dir.path(), ".gitignore", "# nothing useful in here\n");
    write(dir.path(), ".env", "GITHUB_TOKEN=secret");
    write(dir.path(), "note.md", "hi");
    let remote = Fake::default();
    sync(dir.path(), &remote);
    assert_eq!(paths(&remote.files()), [".gitignore", "note.md"]);
}

#[test]
fn a_push_that_loses_a_race_is_retried_on_top_of_the_new_state() {
    let dir = tempdir().unwrap();
    write(dir.path(), "mine.md", "mine");
    let remote = Fake::default();
    sync(dir.path(), &remote);

    write(dir.path(), "mine.md", "mine, edited");
    remote.inner.lock().unwrap().race = Some(("theirs.md".into(), "theirs".into()));

    let outcome = sync(dir.path(), &remote);
    assert_eq!(remote.files()["mine.md"], "mine, edited");
    assert_eq!(remote.files()["theirs.md"], "theirs");
    assert_eq!(read(dir.path(), "theirs.md").unwrap(), "theirs");
    assert_eq!((outcome.pushed, outcome.pulled), (1, 1));
}

#[test]
fn a_branch_that_vanished_does_not_delete_the_vault() {
    let dir = tempdir().unwrap();
    write(dir.path(), "a.md", "a");
    write(dir.path(), "b.md", "b");
    let remote = Fake::default();
    sync(dir.path(), &remote);

    // The branch is deleted and recreated empty on GitHub.
    let fresh = Fake::default();
    let outcome = sync(dir.path(), &fresh);
    assert_eq!(outcome.removed_here, 0);
    assert_eq!(read(dir.path(), "a.md").unwrap(), "a");
    assert_eq!(paths(&fresh.files()), ["a.md", "b.md"], "the vault is pushed back up");
}

#[test]
fn switching_repository_starts_a_fresh_comparison() {
    let dir = tempdir().unwrap();
    write(dir.path(), "a.md", "a");
    let first = Fake::default();
    sync(dir.path(), &first);

    // Another repository already has a different a.md: that is a conflict, resolved in favour of this machine.
    let other = Fake::default();
    other.elsewhere(&[("a.md", Some("someone else's")), ("c.md", Some("c"))]);
    let outcome = sync::run(dir.path(), "me/other", "main", &other, &DiskApplier { root: dir.path().to_path_buf() }).unwrap();
    assert_eq!(outcome.conflicts, ["a.md"]);
    assert_eq!(other.files()["a.md"], "a");
    assert_eq!(read(dir.path(), "c.md").unwrap(), "c");
}

#[test]
fn a_file_edited_while_the_sync_runs_is_not_overwritten() {
    struct Racy<'a>(&'a Path);
    impl sync::Applier for Racy<'_> {
        fn apply(&self, ops: Vec<sync::LocalOp>) -> Result<Vec<String>> {
            // The user saves another edit between the sync looking at the file and writing to it.
            write(self.0, "a.md", "typed meanwhile");
            Ok(sync::apply_to_disk(self.0, ops)?.0)
        }
    }

    let dir = tempdir().unwrap();
    write(dir.path(), "a.md", "1");
    let remote = Fake::default();
    sync(dir.path(), &remote);
    remote.elsewhere(&[("a.md", Some("2 from there"))]);

    let outcome = sync::run(dir.path(), "me/notes", "main", &remote, &Racy(dir.path())).unwrap();
    assert_eq!(outcome.skipped, 1);
    assert_eq!(read(dir.path(), "a.md").unwrap(), "typed meanwhile");

    // The next round sees both sides changed: this machine's edit wins and the other is saved.
    let next = sync(dir.path(), &remote);
    assert_eq!(next.conflicts, ["a.md"]);
    assert_eq!(remote.files()["a.md"], "typed meanwhile");
}

#[test]
fn binary_and_nested_files_survive() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("assets/deep")).unwrap();
    fs::write(dir.path().join("assets/deep/pic.bin"), [0u8, 159, 146, 150, 255]).unwrap();
    let remote = Fake::default();
    sync(dir.path(), &remote);

    let elsewhere = tempdir().unwrap();
    sync(elsewhere.path(), &remote);
    assert_eq!(fs::read(elsewhere.path().join("assets/deep/pic.bin")).unwrap(), [0u8, 159, 146, 150, 255]);
}

#[test]
fn blob_shas_match_gits() {
    // `printf 'hello\n' | git hash-object --stdin`, and the empty blob.
    assert_eq!(blob_sha(b"hello\n"), "ce013625030ba8dba906f756967f9e9ca394464a");
    assert_eq!(blob_sha(b""), "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391");
}

// ---- with encryption --------------------------------------------------------------------------------
//
// "Machines" are vault folders that share one fake remote. Encrypting machines share a key.

/// A remote that is only borrowed, so a test can wrap the fake and still look at it afterwards.
impl<T: Remote> Remote for &T {
    fn head(&self) -> Result<Option<Head>> {
        (**self).head()
    }

    fn tree(&self, head: &Head) -> Result<BTreeMap<String, String>> {
        (**self).tree(head)
    }

    fn blob(&self, path: &str, sha: &str) -> Result<Vec<u8>> {
        (**self).blob(path, sha)
    }

    fn push(&self, parent: Option<&Head>, changes: &[Change], message: &str) -> Result<Head> {
        (**self).push(parent, changes, message)
    }

    fn key_id(&self) -> Option<u8> {
        (**self).key_id()
    }
}

fn key() -> Key {
    Key::from_bytes([7; 32])
}

fn other_key() -> Key {
    (0u8..=255).map(|b| Key::from_bytes([b; 32])).find(|k| k.id() != key().id()).unwrap()
}

fn seal(path: &str, text: &str) -> Vec<u8> {
    crypto::armor(&crypto::encrypt(&key(), path, text.as_bytes()).unwrap())
}

impl Fake {
    /// What another machine that has the key pushes.
    fn elsewhere_sealed(&self, changes: &[(&str, Option<&str>)]) {
        self.inner.lock().unwrap().commit(changes.iter().map(|(p, t)| (p.to_string(), t.map(|t| seal(p, t)))));
    }

    /// The files as stored.
    fn raw(&self) -> BTreeMap<String, Vec<u8>> {
        self.inner.lock().unwrap().head_files()
    }

    /// The files as the key opens them. Panics if any of them isn't sealed.
    fn opened(&self) -> BTreeMap<String, String> {
        let opened = |path: &String, stored: &Vec<u8>| {
            String::from_utf8(crypto::decrypt(&key(), path, &crypto::unarmor(stored).unwrap()).unwrap()).unwrap()
        };
        self.raw().iter().map(|(path, stored)| (path.clone(), opened(path, stored))).collect()
    }

    /// Changes a stored file in place, as someone with write access to the repository could.
    fn tamper(&self, path: &str, change: impl FnOnce(&mut Vec<u8>)) {
        let mut inner = self.inner.lock().unwrap();
        change(inner.commits.last_mut().unwrap().get_mut(path).unwrap());
    }

    fn swap(&self, a: &str, b: &str) {
        let mut inner = self.inner.lock().unwrap();
        let files = inner.commits.last_mut().unwrap();
        let (x, y) = (files[a].clone(), files[b].clone());
        files.insert(a.to_string(), y);
        files.insert(b.to_string(), x);
    }
}

fn try_sync_with(root: &Path, remote: &Fake, key: Key) -> Result<Outcome> {
    sync::run(root, "me/notes", "main", &EncryptedRemote::new(remote, key), &DiskApplier { root: root.to_path_buf() })
}

fn sync_sealed(root: &Path, remote: &Fake) -> Outcome {
    try_sync_with(root, remote, key()).unwrap()
}

/// A sealed sync to another branch of the same repository (each branch is a `Fake` of its own).
fn try_sync_sealed_on(root: &Path, branch: &str, remote: &Fake) -> Result<Outcome> {
    sync::run(root, "me/notes", branch, &EncryptedRemote::new(remote, key()), &DiskApplier { root: root.to_path_buf() })
}

fn try_sync_plain(root: &Path, remote: &Fake) -> Result<Outcome> {
    sync::run(root, "me/notes", "main", remote, &DiskApplier { root: root.to_path_buf() })
}

/// The path and cause of a sync that stopped because a file wouldn't open.
fn refused(result: Result<Outcome>) -> (String, CryptoError) {
    match result {
        Err(Error::Crypto { path, cause }) => (path, cause),
        Err(e) => panic!("stopped for another reason: {e}"),
        Ok(outcome) => panic!("synced anyway: {outcome:?}"),
    }
}

fn is_sealed(remote: &Fake) -> bool {
    remote.raw().values().all(|bytes| crypto::is_encrypted(bytes))
}

/// Writes into the vault between the sync reading a file and writing to it.
struct EditsMeanwhile<'a> {
    root: &'a Path,
    path: &'a str,
    text: &'a str,
}

impl sync::Applier for EditsMeanwhile<'_> {
    fn apply(&self, ops: Vec<sync::LocalOp>) -> Result<Vec<String>> {
        write(self.root, self.path, self.text);
        Ok(sync::apply_to_disk(self.root, ops)?.0)
    }
}

#[test]
fn a_sealed_sync_uploads_no_plaintext_and_another_machine_reads_it() {
    let (a, b) = (tempdir().unwrap(), tempdir().unwrap());
    write(a.path(), "threads/t/posts/1.md", "my private thoughts");
    write(a.path(), "README.md", "# notes");
    let remote = Fake::default();

    let first = sync_sealed(a.path(), &remote);
    assert_eq!((first.pushed, first.pulled), (2, 0));
    assert_eq!(remote.commits(), 1);
    assert!(is_sealed(&remote));
    for bytes in remote.raw().values() {
        assert!(!String::from_utf8_lossy(bytes).contains("private") && !String::from_utf8_lossy(bytes).contains("# notes"));
    }
    assert_eq!(paths(&remote.opened()), ["README.md", "threads/t/posts/1.md"], "paths stay readable");
    assert_eq!(remote.opened()["threads/t/posts/1.md"], "my private thoughts");
    assert_eq!(read(a.path(), "threads/t/posts/1.md").unwrap(), "my private thoughts", "the vault on disk is untouched");

    // Each side is compared with its own sha: the vault with the file's, the remote with the stored blob's.
    let state = sync::load_state(a.path()).unwrap();
    assert_eq!(state.encrypted, Some(key().id()));
    assert_eq!(state.files["README.md"], blob_sha(b"# notes"));
    assert_eq!(state.remote["README.md"], blob_sha(&remote.raw()["README.md"]));
    assert_ne!(state.files["README.md"], state.remote["README.md"]);

    assert_eq!(sync_sealed(a.path(), &remote), Outcome::default());
    assert_eq!(remote.commits(), 1);

    let pulled = sync_sealed(b.path(), &remote);
    assert_eq!((pulled.pulled, pulled.pushed), (2, 0));
    assert_eq!(read(b.path(), "threads/t/posts/1.md").unwrap(), "my private thoughts");
    assert_eq!(sync_sealed(b.path(), &remote), Outcome::default());
    assert_eq!((remote.commits(), remote.pushes()), (1, 1));
}

#[test]
fn the_file_that_holds_the_key_is_never_uploaded() {
    let dir = tempdir().unwrap();
    write(dir.path(), "a.md", "a note");
    EnvFileKeyStore.save(dir.path(), &key()).unwrap();
    assert!(read(dir.path(), ".ledger/.env").unwrap().contains("LEDGER_ENCRYPTION_KEY="), "the key is where it should be");
    let recovery = key().to_recovery().to_string();
    let remote = Fake::default();

    // Sealed or not, nothing under `.ledger/` goes up, and the key is in nothing that does.
    sync_sealed(dir.path(), &remote);
    let opened = remote.opened();
    assert!(opened.contains_key("a.md") && opened.keys().all(|p| !p.starts_with(".ledger") && p != ".env"), "{:?}", opened.keys());
    assert!(opened.values().all(|text| !text.contains(&recovery)));

    // The same without encryption, from a vault that was never sealed.
    let unsealed = tempdir().unwrap();
    write(unsealed.path(), "a.md", "a note");
    EnvFileKeyStore.save(unsealed.path(), &key()).unwrap();
    let plain = Fake::default();
    try_sync_plain(unsealed.path(), &plain).unwrap();
    let files = plain.inner.lock().unwrap().head_files();
    assert!(files.keys().all(|p| !p.starts_with(".ledger") && p != ".env"), "{:?}", files.keys());
    assert!(files.values().all(|bytes| !String::from_utf8_lossy(bytes).contains(&recovery)));
}

#[test]
fn only_the_files_that_changed_are_sealed_and_pushed_again() {
    let dir = tempdir().unwrap();
    for name in ["a.md", "b.md", "c.md"] {
        write(dir.path(), name, &format!("{name} v1"));
    }
    let remote = Fake::default();
    sync_sealed(dir.path(), &remote);
    let before = remote.raw();

    write(dir.path(), "b.md", "b.md v2");
    let outcome = sync_sealed(dir.path(), &remote);
    assert_eq!((outcome.pushed, outcome.pulled), (1, 0));
    let after = remote.raw();
    assert_eq!(after["a.md"], before["a.md"], "an unchanged file is not sealed again (a new nonce would be churn)");
    assert_eq!(after["c.md"], before["c.md"]);
    assert_ne!(after["b.md"], before["b.md"]);
    assert_eq!(remote.opened()["b.md"], "b.md v2");

    // Saving a file again with the same contents changes nothing either.
    write(dir.path(), "a.md", "a.md v1");
    assert_eq!(sync_sealed(dir.path(), &remote), Outcome::default());
    assert_eq!((remote.commits(), remote.raw()), (2, after));
}

#[test]
fn edits_on_two_machines_to_different_files_are_both_kept_when_sealed() {
    let (a, b) = (tempdir().unwrap(), tempdir().unwrap());
    write(a.path(), "a.md", "a1");
    write(a.path(), "b.md", "b1");
    let remote = Fake::default();
    sync_sealed(a.path(), &remote);
    sync_sealed(b.path(), &remote);

    write(a.path(), "a.md", "a2 (machine A)");
    write(b.path(), "b.md", "b2 (machine B)");
    write(b.path(), "new.md", "new on B");
    sync_sealed(a.path(), &remote);
    let on_b = sync_sealed(b.path(), &remote);
    assert_eq!((on_b.pulled, on_b.pushed), (1, 2));
    assert!(on_b.conflicts.is_empty());
    let on_a = sync_sealed(a.path(), &remote);
    assert_eq!((on_a.pulled, on_a.pushed), (2, 0));

    for root in [a.path(), b.path()] {
        assert_eq!(read(root, "a.md").unwrap(), "a2 (machine A)");
        assert_eq!(read(root, "b.md").unwrap(), "b2 (machine B)");
        assert_eq!(read(root, "new.md").unwrap(), "new on B");
    }
    assert_eq!(remote.opened()["b.md"], "b2 (machine B)");
    assert!(is_sealed(&remote));
    assert_eq!(sync_sealed(b.path(), &remote), Outcome::default());
}

#[test]
fn a_file_edited_on_two_machines_is_still_a_conflict_when_sealed() {
    let (a, b) = (tempdir().unwrap(), tempdir().unwrap());
    write(a.path(), "plan.md", "v1");
    let remote = Fake::default();
    sync_sealed(a.path(), &remote);
    sync_sealed(b.path(), &remote);

    write(a.path(), "plan.md", "v2 from A");
    write(b.path(), "plan.md", "v2 from B");
    sync_sealed(a.path(), &remote);
    let outcome = sync_sealed(b.path(), &remote);

    assert_eq!(outcome.conflicts, ["plan.md"]);
    assert_eq!(remote.opened()["plan.md"], "v2 from B");
    assert_eq!(read(b.path(), "plan.md").unwrap(), "v2 from B");
    // The other version was opened before being set aside, so the saved copy is readable.
    let saved: Vec<_> = fs::read_dir(b.path().join(".ledger/conflicts")).unwrap().map(|e| e.unwrap().path().join("plan.md")).collect();
    assert_eq!(saved.len(), 1);
    assert_eq!(fs::read_to_string(&saved[0]).unwrap(), "v2 from A");

    sync_sealed(a.path(), &remote);
    assert_eq!(read(a.path(), "plan.md").unwrap(), "v2 from B");
}

#[test]
fn the_same_edit_on_two_machines_is_not_a_conflict() {
    let (a, b) = (tempdir().unwrap(), tempdir().unwrap());
    write(a.path(), "plan.md", "v1");
    let remote = Fake::default();
    sync_sealed(a.path(), &remote);
    sync_sealed(b.path(), &remote);

    write(a.path(), "plan.md", "same on both");
    write(b.path(), "plan.md", "same on both");
    sync_sealed(a.path(), &remote);
    let stored = remote.raw();

    // Only opening the other copy shows they match; nothing is saved and nothing is pushed.
    let outcome = sync_sealed(b.path(), &remote);
    assert_eq!(outcome, Outcome::default());
    assert!(!b.path().join(".ledger/conflicts").exists());
    assert_eq!((remote.commits(), remote.raw()), (2, stored));
    assert_eq!(sync_sealed(b.path(), &remote), Outcome::default());
}

#[test]
fn deletions_and_the_rule_that_an_edit_beats_a_delete_hold_when_sealed() {
    let (a, b) = (tempdir().unwrap(), tempdir().unwrap());
    for name in ["keep.md", "gone-on-a.md", "gone-on-b.md", "edited-on-b.md"] {
        write(a.path(), name, "1");
    }
    let remote = Fake::default();
    sync_sealed(a.path(), &remote);
    sync_sealed(b.path(), &remote);

    fs::remove_file(a.path().join("gone-on-a.md")).unwrap();
    fs::remove_file(a.path().join("edited-on-b.md")).unwrap();
    sync_sealed(a.path(), &remote);
    fs::remove_file(b.path().join("gone-on-b.md")).unwrap();
    write(b.path(), "edited-on-b.md", "2");
    let outcome = sync_sealed(b.path(), &remote);
    assert_eq!((outcome.removed_here, outcome.removed_there), (1, 1));

    sync_sealed(a.path(), &remote);
    assert_eq!(paths(&remote.opened()), ["edited-on-b.md", "keep.md"]);
    assert_eq!(read(a.path(), "edited-on-b.md").unwrap(), "2", "the edit survived the delete");
    assert!(read(b.path(), "gone-on-a.md").is_none() && read(a.path(), "gone-on-b.md").is_none());
}

#[test]
fn a_push_that_loses_a_race_is_retried_when_sealed() {
    let dir = tempdir().unwrap();
    write(dir.path(), "mine.md", "mine");
    let remote = Fake::default();
    sync_sealed(dir.path(), &remote);

    write(dir.path(), "mine.md", "mine, edited");
    remote.inner.lock().unwrap().race = Some(("theirs.md".into(), String::from_utf8(seal("theirs.md", "theirs")).unwrap()));
    let outcome = sync_sealed(dir.path(), &remote);
    assert_eq!((outcome.pushed, outcome.pulled), (1, 1));
    assert_eq!(remote.opened()["mine.md"], "mine, edited");
    assert_eq!(read(dir.path(), "theirs.md").unwrap(), "theirs");
}

#[test]
fn a_file_edited_while_a_sealed_sync_runs_is_not_overwritten() {
    let dir = tempdir().unwrap();
    write(dir.path(), "a.md", "1");
    let remote = Fake::default();
    sync_sealed(dir.path(), &remote);
    remote.elsewhere_sealed(&[("a.md", Some("2 from there"))]);

    let applier = EditsMeanwhile { root: dir.path(), path: "a.md", text: "typed meanwhile" };
    let outcome = sync::run(dir.path(), "me/notes", "main", &EncryptedRemote::new(&remote, key()), &applier).unwrap();
    assert_eq!(outcome.skipped, 1);
    assert_eq!(read(dir.path(), "a.md").unwrap(), "typed meanwhile");

    let next = sync_sealed(dir.path(), &remote);
    assert_eq!(next.conflicts, ["a.md"]);
    assert_eq!(remote.opened()["a.md"], "typed meanwhile");
}

#[test]
fn the_push_is_not_lost_if_the_connection_drops_right_after_it() {
    let dir = tempdir().unwrap();
    write(dir.path(), "a.md", "a");
    write(dir.path(), "b.md", "b");
    let remote = Fake::default();

    // The first thing a first push asks for afterwards is the tree, to learn what GitHub stored.
    remote.inner.lock().unwrap().tree_fails = true;
    assert!(try_sync_with(dir.path(), &remote, key()).is_err());
    assert_eq!(remote.commits(), 1, "the push itself went through");
    assert!(sync::load_state(dir.path()).is_none(), "but this machine doesn't know that yet");

    // Next round finds both sides holding the same files, and settles without a conflict or a second push.
    let outcome = sync_sealed(dir.path(), &remote);
    assert_eq!((outcome.pushed, outcome.pulled, outcome.conflicts.len()), (0, 0, 0));
    assert!(!dir.path().join(".ledger/conflicts").exists());
    assert_eq!((remote.commits(), remote.pushes()), (1, 1));
    assert_eq!(sync_sealed(dir.path(), &remote), Outcome::default());
}

// ---- keys that don't fit, and files that don't open ---------------------------------------------------

#[test]
fn the_wrong_key_stops_the_sync_and_changes_nothing_here() {
    let (a, b) = (tempdir().unwrap(), tempdir().unwrap());
    write(a.path(), "a.md", "from A");
    write(a.path(), "b.md", "also from A");
    let remote = Fake::default();
    sync_sealed(a.path(), &remote);
    let stored = remote.raw();

    write(b.path(), "mine.md", "keep me");
    write(b.path(), "a.md", "my own a.md");
    let result = try_sync_with(b.path(), &remote, other_key());
    let message = result.as_ref().err().unwrap().to_string();
    let (_, cause) = refused(result);
    assert!(matches!(cause, CryptoError::WrongKey { found, expected } if found == key().id() && expected == other_key().id()), "{cause:?}");
    assert!(message.contains("different key") && message.contains("nothing on this machine was changed"), "{message}");

    assert_eq!(read(b.path(), "mine.md").unwrap(), "keep me");
    assert_eq!(read(b.path(), "a.md").unwrap(), "my own a.md");
    assert!(read(b.path(), "b.md").is_none());
    assert!(!b.path().join(".ledger").exists(), "no state, no conflict files");
    assert_eq!((remote.raw(), remote.commits()), (stored, 1), "and nothing was pushed");
}

#[test]
fn one_file_that_will_not_decrypt_stops_the_whole_pull_before_anything_is_written() {
    let remote = Fake::default();
    remote.elsewhere_sealed(&[("a.md", Some("a")), ("b.md", Some("b")), ("c.md", Some("c")), ("d.md", Some("d"))]);
    remote.tamper("c.md", |stored| {
        let mut sealed = crypto::unarmor(stored).unwrap();
        let last = sealed.len() - 1;
        sealed[last] ^= 1;
        *stored = crypto::armor(&sealed);
    });
    let stored = remote.raw();

    let dir = tempdir().unwrap();
    write(dir.path(), "z.md", "local only");
    let (path, cause) = refused(try_sync_with(dir.path(), &remote, key()));
    assert_eq!((path.as_str(), cause), ("c.md", CryptoError::Failed));

    // The ones before and after it were fine, and are still not on disk.
    for name in ["a.md", "b.md", "d.md"] {
        assert!(read(dir.path(), name).is_none(), "{name}");
    }
    assert_eq!(read(dir.path(), "z.md").unwrap(), "local only");
    assert!(!dir.path().join(".ledger").exists());
    assert_eq!((remote.raw(), remote.commits()), (stored, 1));
}

#[test]
fn a_failure_to_decrypt_never_deletes_or_overwrites_what_is_here() {
    let (a, b) = (tempdir().unwrap(), tempdir().unwrap());
    write(a.path(), "keep.md", "keep v1");
    write(a.path(), "edit.md", "edit v1");
    write(a.path(), "drop.md", "drop v1");
    let remote = Fake::default();
    sync_sealed(a.path(), &remote);
    sync_sealed(b.path(), &remote);

    // Elsewhere: one file edited (but damaged in transit), one deleted. Both would change B.
    remote.elsewhere_sealed(&[("edit.md", Some("edit v2")), ("drop.md", None)]);
    remote.tamper("edit.md", |stored| stored.truncate(stored.len() - 8));

    let (path, _) = refused(try_sync_with(b.path(), &remote, key()));
    assert_eq!(path, "edit.md");
    assert_eq!(read(b.path(), "edit.md").unwrap(), "edit v1");
    assert_eq!(read(b.path(), "drop.md").unwrap(), "drop v1", "the delete is not carried out either");
    assert_eq!(read(b.path(), "keep.md").unwrap(), "keep v1");
}

#[test]
fn a_blob_moved_to_another_path_is_refused() {
    let remote = Fake::default();
    remote.elsewhere_sealed(&[("projects/a/readme.md", Some("about a")), ("projects/b/readme.md", Some("about b"))]);
    remote.swap("projects/a/readme.md", "projects/b/readme.md");

    let dir = tempdir().unwrap();
    let (_, cause) = refused(try_sync_with(dir.path(), &remote, key()));
    assert_eq!(cause, CryptoError::Failed);
    assert!(read(dir.path(), "projects/a/readme.md").is_none() && read(dir.path(), "projects/b/readme.md").is_none());
}

#[test]
fn plain_files_on_a_sealed_remote_are_refused_not_imported() {
    // A machine that has never synced here, with encryption on, meets a repository of plain files.
    let remote = Fake::default();
    remote.elsewhere(&[("a.md", Some("plain, written by something that doesn't encrypt"))]);
    let stored = remote.raw();
    let dir = tempdir().unwrap();
    write(dir.path(), "mine.md", "mine");

    let result = try_sync_with(dir.path(), &remote, key());
    let message = result.as_ref().err().unwrap().to_string();
    assert_eq!(refused(result), ("a.md".to_string(), CryptoError::NotEncrypted));
    assert!(message.contains("isn't encrypted"), "{message}");
    assert!(message.contains("new repository") && message.contains("doesn't exist yet"), "it says what to do instead: {message}");
    assert!(read(dir.path(), "a.md").is_none());
    assert_eq!(read(dir.path(), "mine.md").unwrap(), "mine");
    assert_eq!((remote.raw(), remote.commits()), (stored, 1), "and this machine's files were not pushed into the mixed repository");
    assert!(sync::load_state(dir.path()).is_none());
}

#[test]
fn a_plain_file_appearing_later_in_a_sealed_remote_is_refused_too() {
    let dir = tempdir().unwrap();
    write(dir.path(), "a.md", "a");
    let remote = Fake::default();
    sync_sealed(dir.path(), &remote);

    // A new plain file, and a plain replacement of a file this machine has.
    remote.elsewhere(&[("sneaky.md", Some("not sealed"))]);
    assert_eq!(refused(try_sync_with(dir.path(), &remote, key())).1, CryptoError::NotEncrypted);
    assert!(read(dir.path(), "sneaky.md").is_none());

    remote.elsewhere(&[("a.md", Some("replaced by plain text"))]);
    assert_eq!(refused(try_sync_with(dir.path(), &remote, key())).1, CryptoError::NotEncrypted);
    assert_eq!(read(dir.path(), "a.md").unwrap(), "a");
}

#[test]
fn a_sealed_remote_is_refused_by_a_machine_without_encryption() {
    let (a, plain) = (tempdir().unwrap(), tempdir().unwrap());
    write(a.path(), "a.md", "secret a");
    let remote = Fake::default();
    sync_sealed(a.path(), &remote);
    let stored = remote.raw();

    // Never synced here, encryption off: the stored text must not be written into the vault as if it were the notes.
    write(plain.path(), "mine.md", "mine");
    let message = try_sync_plain(plain.path(), &remote).unwrap_err().to_string();
    assert!(message.contains("a.md") && message.contains("encrypted") && message.contains("Turn encryption on"), "{message}");
    assert!(read(plain.path(), "a.md").is_none());
    assert_eq!((remote.raw(), remote.commits()), (stored.clone(), 1));

    // And when the vault was synced sealed before, turning encryption off stops the sync instead of pushing plain files.
    write(a.path(), "b.md", "plain b");
    let message = try_sync_plain(a.path(), &remote).unwrap_err().to_string();
    assert!(message.contains("encryption on") && message.contains("Turn it back on"), "{message}");
    assert_eq!((remote.raw(), remote.commits()), (stored, 1));
    assert_eq!(sync_sealed(a.path(), &remote).pushed, 1, "and turning it back on carries on as before");
}

#[test]
fn a_vault_synced_with_one_key_refuses_another() {
    let dir = tempdir().unwrap();
    write(dir.path(), "a.md", "a");
    let remote = Fake::default();
    sync_sealed(dir.path(), &remote);
    let stored = remote.raw();

    write(dir.path(), "b.md", "b");
    let message = try_sync_with(dir.path(), &remote, other_key()).unwrap_err().to_string();
    assert!(message.contains(&format!("{:02x}", key().id())) && message.contains(&format!("{:02x}", other_key().id())), "{message}");
    assert_eq!(remote.raw(), stored);
    assert_eq!(sync_sealed(dir.path(), &remote).pushed, 1, "the right key still works");
}

// ---- encryption starts fresh, never over plain history -----------------------------------------------

#[test]
fn encryption_is_refused_for_a_branch_this_vault_synced_without_it() {
    let dir = tempdir().unwrap();
    for name in ["a.md", "b.md"] {
        write(dir.path(), name, &format!("{name} in plain text"));
    }
    let remote = Fake::default();
    sync(dir.path(), &remote);
    let sent = (remote.raw(), remote.commits());
    let before = sync::load_state(dir.path()).unwrap();
    assert!(sync::has_plain_history(dir.path(), "me/notes", "main"));
    assert!(!sync::has_plain_history(dir.path(), "me/notes", "another-branch"));

    write(dir.path(), "c.md", "typed after");
    let message = try_sync_with(dir.path(), &remote, key()).unwrap_err().to_string();
    assert!(message.contains("unencrypted") && message.contains("new repository") && message.contains("doesn't exist yet"), "{message}");

    // Nothing is sent, nothing here is touched, nothing is recorded.
    assert_eq!((remote.raw(), remote.commits()), sent);
    assert_eq!(read(dir.path(), "a.md").unwrap(), "a.md in plain text");
    let after = sync::load_state(dir.path()).unwrap();
    assert_eq!((after.files, after.encrypted, after.head), (before.files, None, before.head));

    // Without encryption, sync carries on as it always did.
    assert_eq!(sync(dir.path(), &remote).pushed, 1);
}

#[test]
fn a_new_branch_starts_encrypted_with_nothing_plain_behind_it() {
    let dir = tempdir().unwrap();
    for name in ["a.md", "b.md", "c.md"] {
        write(dir.path(), name, &format!("{name} in plain text"));
    }
    let plain_branch = Fake::default();
    sync(dir.path(), &plain_branch);

    // The same vault, pointed at a branch that doesn't exist yet.
    let fresh = Fake::default();
    let outcome = try_sync_sealed_on(dir.path(), "encrypted", &fresh).unwrap();
    assert_eq!((outcome.pushed, outcome.pulled), (3, 0));
    assert_eq!(fresh.commits(), 1, "a root commit with only sealed files in it, and nothing before it");
    assert!(is_sealed(&fresh));
    assert_eq!(fresh.opened()["b.md"], "b.md in plain text");
    assert_eq!(read(dir.path(), "b.md").unwrap(), "b.md in plain text", "the vault on disk is untouched");
    let state = sync::load_state(dir.path()).unwrap();
    assert_eq!((state.branch.as_str(), state.encrypted), ("encrypted", Some(key().id())));

    // The old branch is left as it was: getting rid of it is for its owner, on GitHub.
    assert_eq!((plain_branch.files()["a.md"].as_str(), plain_branch.commits()), ("a.md in plain text", 1));

    // From here on it is an ordinary encrypted branch.
    assert_eq!(try_sync_sealed_on(dir.path(), "encrypted", &fresh).unwrap(), Outcome::default());
    assert!(sync::has_plain_history(dir.path(), "me/notes", "encrypted") == false);
}

#[test]
fn if_the_plain_branch_was_deleted_on_github_encryption_starts_over_there() {
    let dir = tempdir().unwrap();
    write(dir.path(), "a.md", "a in plain text");
    sync(dir.path(), &Fake::default());

    // Same repository and branch, but the branch is gone: nothing plain is left behind to protect.
    let gone = Fake::default();
    let outcome = try_sync_with(dir.path(), &gone, key()).unwrap();
    assert_eq!((outcome.pushed, outcome.pulled), (1, 0));
    assert!(is_sealed(&gone));
    assert_eq!(gone.commits(), 1);
    assert_eq!(read(dir.path(), "a.md").unwrap(), "a in plain text");
}

#[test]
fn a_sync_state_from_before_encryption_still_loads_and_still_works() {
    let dir = tempdir().unwrap();
    write(dir.path(), "a.md", "a1");
    let remote = Fake::default();
    remote.elsewhere(&[("a.md", Some("a1"))]);
    let sha = blob_sha(b"a1");
    // Exactly what an older version wrote: no `remote`, no `encrypted`.
    let old = format!(r#"{{"repo":"me/notes","branch":"main","head":"commit-0","files":{{"a.md":"{sha}"}}}}"#);
    write(dir.path(), ".ledger/sync-state.json", &old);

    let state = sync::load_state(dir.path()).unwrap();
    assert_eq!((state.encrypted, state.remote.len(), state.files.len()), (None, 0, 1));
    assert_eq!(sync(dir.path(), &remote), Outcome::default(), "an agreement is an agreement");

    // Without encryption the file keeps its old shape, so older builds could still read it.
    let text = read(dir.path(), ".ledger/sync-state.json").unwrap();
    assert!(!text.contains("remote") && !text.contains("encrypted"), "{text}");

    // Over the old state encryption is refused, as over any branch that holds plain files...
    write(dir.path(), ".ledger/sync-state.json", &old);
    let message = try_sync_with(dir.path(), &remote, key()).unwrap_err().to_string();
    assert!(message.contains("new repository"), "{message}");
    assert!(!is_sealed(&remote));

    // ...and starts over on a branch that doesn't exist yet.
    let fresh = Fake::default();
    assert_eq!(try_sync_sealed_on(dir.path(), "sealed", &fresh).unwrap().pushed, 1);
    assert!(is_sealed(&fresh));
    assert_eq!(fresh.opened()["a.md"], "a1");
}

#[test]
fn without_encryption_everything_is_as_it_was() {
    let dir = tempdir().unwrap();
    write(dir.path(), "a.md", "plain");
    let remote = Fake::default();
    sync(dir.path(), &remote);

    assert_eq!(remote.files()["a.md"], "plain", "plain files go up as they are");
    let state = sync::load_state(dir.path()).unwrap();
    assert_eq!((state.encrypted, state.remote.len()), (None, 0));
    assert_eq!(state.files["a.md"], blob_sha(b"plain"));

    // Identical files are still downloaded once.
    let elsewhere = tempdir().unwrap();
    remote.elsewhere(&[("x.md", Some("same")), ("y.md", Some("same"))]);
    let requests = remote.blob_requests();
    let outcome = sync(elsewhere.path(), &remote);
    assert_eq!((outcome.pulled, outcome.pushed), (3, 0));
    assert_eq!(read(elsewhere.path(), "y.md").unwrap(), "same");
    assert_eq!(remote.blob_requests() - requests, 2, "three files, two different contents");
}
