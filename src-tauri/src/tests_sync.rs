//! The sync algorithm against an in-memory stand-in for a GitHub branch.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Mutex;

use tempfile::tempdir;

use crate::error::{Error, Result};
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
        let inner = self.inner.lock().unwrap();
        Ok(inner.commits[index_of(head)].iter().map(|(p, b)| (p.clone(), blob_sha(b))).collect())
    }

    fn blob(&self, sha: &str) -> Result<Vec<u8>> {
        let inner = self.inner.lock().unwrap();
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
