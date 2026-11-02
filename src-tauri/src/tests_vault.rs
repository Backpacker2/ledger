//! The vault: what the files look like, that they round-trip, that they are written sparingly, and that a
//! vault written by hand loads without complaint.

use std::fs;
use std::path::Path;

use rusqlite::Connection;
use tempfile::tempdir;

use crate::frontmatter::{self, MetaExt};
use crate::models::{Bump, Criterion, StoryDraft, StoryUpdate};
use crate::posts::AddOptions;
use crate::settings::{self, SyncEnv};
use crate::vault::{self, Vault};
use crate::{db, document_types, documents, planboard, posts, stories, threads};

fn write(root: &Path, rel: &str, text: &str) {
    let file = root.join(rel);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, text).unwrap();
}

fn read(root: &Path, rel: &str) -> String {
    fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn open(root: &Path) -> (Vault, Connection) {
    let mut vault = Vault::open(root.to_path_buf()).unwrap();
    let conn = vault.load().unwrap();
    (vault, conn)
}

/// A vault with a bit of everything in it.
fn populate(conn: &Connection) {
    let interview = document_types::create(conn, "Interview", Some("after a call"), Some(&["Who?".into(), "Why: really?".into()])).unwrap().slug;
    let alpha = threads::create(conn, "Alpha: the first").unwrap().slug;
    let beta = threads::create(conn, "Beta").unwrap().slug;
    threads::link(conn, &alpha, &beta).unwrap();

    let first = posts::add(conn, &alpha, "A note with **bold**\n\nand a second paragraph.", AddOptions::default()).unwrap();
    posts::add(conn, &alpha, "A reply", AddOptions { reply_to_id: Some(first.id), ..Default::default() }).unwrap();

    let doc = documents::create(conn, &interview, "Call with Ana", Some(&alpha)).unwrap();
    documents::update(conn, &doc.id, None, Some("# Heading\n\nSome *body*.")).unwrap();
    documents::publish(conn, &doc.id, Bump::Minor, None).unwrap();
    documents::update(conn, &doc.id, None, Some("# Heading\n\nA later body.")).unwrap();
    documents::publish(conn, &doc.id, Bump::Major, None).unwrap();
    documents::create(conn, &interview, "Loose draft", None).unwrap();

    let story = StoryDraft {
        id: "MM01".into(),
        summary: "Clarify pricing".into(),
        role: "customer".into(),
        want: "see prices".into(),
        reason: "I can decide".into(),
        criteria: vec![Criterion { text: "A page lists plans".into(), done: true }, Criterion { text: "Prices in euro".into(), done: false }],
        thread_slugs: vec![alpha, beta],
    };
    stories::create(conn, story).unwrap();
    stories::update(conn, "MM01", StoryUpdate { column_id: Some("drafting".into()), ..Default::default() }).unwrap();
    planboard::add_column(conn, "In review").unwrap();
}

/// Everything the screens would show, as one string.
fn snapshot(conn: &Connection) -> String {
    let mut out = String::new();
    for t in threads::list(conn).unwrap() {
        out += &format!("{:?}\n", threads::get(conn, &t.slug).unwrap());
    }
    out += &format!("{:?}\n{:?}\n", planboard::get(conn).unwrap(), document_types::list(conn).unwrap());
    for d in documents::list_all(conn).unwrap() {
        out += &format!("{:?}\n", documents::get(conn, &d.id).unwrap());
        for v in documents::versions(conn, &d.id).unwrap() {
            out += &format!("{:?}\n", documents::get_version(conn, &d.id, &v).unwrap());
        }
    }
    out
}

// ---- what the files look like -----------------------------------------------------------------------

#[test]
fn the_vault_is_folders_of_markdown_with_frontmatter() {
    let dir = tempdir().unwrap();
    let (mut vault, conn) = open(dir.path());
    populate(&conn);
    vault.persist(&conn).unwrap();
    let root = dir.path();

    let thread = read(root, "threads/alpha-the-first/thread.md");
    assert!(thread.starts_with("---\n"), "{thread}");
    assert!(thread.contains("title: 'Alpha: the first'") || thread.contains("title: \"Alpha: the first\""), "{thread}");
    assert!(thread.contains("linkedThreadSlugs:") && thread.contains("- beta"), "{thread}");

    // Posts: one file each, named by id, body below the frontmatter.
    let posts_dir = root.join("threads/alpha-the-first/posts");
    let files: Vec<_> = fs::read_dir(&posts_dir).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    assert_eq!(files.len(), 6, "{files:?}"); // two notes, two seeded questions and two "version published" notes
    let note = files.iter().map(|f| read(root, &format!("threads/alpha-the-first/posts/{f}"))).find(|t| t.contains("**bold**")).unwrap();
    assert!(note.contains("createdAt:") && note.ends_with("and a second paragraph.\n"), "{note}");
    let question = files.iter().map(|f| read(root, &format!("threads/alpha-the-first/posts/{f}"))).find(|t| t.contains("Guiding question")).unwrap();
    assert!(question.contains("guidingQuestion: true"), "{question}");

    assert!(read(root, "document-types/interview.md").contains("name: Interview"));
    assert!(read(root, "document-types/interview.md").contains("- Who?"));

    let doc = fs::read_dir(root.join("documents")).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect::<Vec<_>>();
    assert!(doc.iter().any(|f| f.ends_with("@v0.1.md")) && doc.iter().any(|f| f.ends_with("@v1.0.md")), "{doc:?}");
    let latest = doc.iter().find(|f| f.ends_with("@v1.0.md")).unwrap();
    let text = read(root, &format!("documents/{latest}"));
    assert!(text.contains("version: '1.0'") || text.contains("version: \"1.0\""), "versions must stay text:\n{text}");
    assert!(text.contains("A later body."));

    let story = read(root, "user-stories/MM01.md");
    assert!(story.contains("columnId: drafting") && story.contains("text: A page lists plans") && story.contains("done: true"), "{story}");
    assert!(story.contains("- alpha-the-first") && story.contains("- beta"), "{story}");

    let board = read(root, "planboard.md");
    assert!(board.contains("name: Backlog") && board.contains("name: In review") && board.contains("backlogColumnId: backlog"), "{board}");

    // Local-only files are kept out of GitHub.
    let ignore = read(root, ".gitignore");
    assert!(ignore.lines().any(|l| l == ".env") && ignore.lines().any(|l| l == ".ledger/"), "{ignore}");
}

#[test]
fn what_is_written_loads_back_identically() {
    let dir = tempdir().unwrap();
    let (mut vault, conn) = open(dir.path());
    populate(&conn);
    vault.persist(&conn).unwrap();

    let (mut again, reloaded) = open(dir.path());
    assert_eq!(vault::export(&conn).unwrap(), vault::export(&reloaded).unwrap());
    assert_eq!(snapshot(&conn), snapshot(&reloaded));
    // Loading a vault that is already in order writes nothing.
    assert_eq!(again.persist(&reloaded).unwrap(), 0);
}

#[test]
fn posts_made_in_the_same_instant_keep_their_order_after_a_reload() {
    let dir = tempdir().unwrap();
    let (mut vault, conn) = open(dir.path());
    let questions: Vec<String> = (1..=12).map(|n| format!("Question {n}?")).collect();
    let t = document_types::create(&conn, "Many", None, Some(&questions)).unwrap().slug;
    let thread = threads::create(&conn, "T").unwrap().slug;
    documents::create(&conn, &t, "Doc", Some(&thread)).unwrap();
    vault.persist(&conn).unwrap();

    let (_, reloaded) = open(dir.path());
    let bodies: Vec<_> = posts::list(&reloaded, &thread).unwrap().into_iter().map(|p| p.body).collect();
    let expected: Vec<_> = questions.iter().map(|q| format!("**Guiding question (Many):** {q}")).collect();
    assert_eq!(bodies, expected);
}

// ---- writing as little as possible ------------------------------------------------------------------

#[test]
fn persisting_touches_only_what_changed() {
    let dir = tempdir().unwrap();
    let (mut vault, conn) = open(dir.path());
    populate(&conn);
    assert!(vault.persist(&conn).unwrap() >= 15, "everything populate made is new");
    assert_eq!(vault.persist(&conn).unwrap(), 0);

    let thread = threads::list(&conn).unwrap().into_iter().find(|t| t.slug == "beta").unwrap().slug;
    let post = posts::add(&conn, &thread, "one more", AddOptions::default()).unwrap();
    assert_eq!(vault.persist(&conn).unwrap(), 1);
    assert_eq!(vault.persist(&conn).unwrap(), 0);

    posts::update(&conn, &thread, &post.id, "edited").unwrap();
    assert_eq!(vault.persist(&conn).unwrap(), 1);
    assert!(read(dir.path(), &format!("threads/beta/posts/{}.md", post.id)).contains("edited"));
}

#[test]
fn deleting_things_deletes_their_files_and_leaves_no_empty_folders() {
    let dir = tempdir().unwrap();
    let (mut vault, conn) = open(dir.path());
    populate(&conn);
    vault.persist(&conn).unwrap();
    assert!(dir.path().join("threads/beta/thread.md").exists());

    threads::delete(&conn, "beta").unwrap();
    vault.persist(&conn).unwrap();
    assert!(!dir.path().join("threads/beta").exists());

    stories::delete(&conn, "MM01").unwrap();
    vault.persist(&conn).unwrap();
    assert!(!dir.path().join("user-stories/MM01.md").exists());

    // Deleting a draft keeps its published versions.
    let doc = documents::list_all(&conn).unwrap().into_iter().find(|d| d.title == "Call with Ana").unwrap();
    documents::delete(&conn, &doc.id).unwrap();
    vault.persist(&conn).unwrap();
    assert!(!dir.path().join(format!("documents/{}.md", doc.id)).exists());
    assert!(dir.path().join(format!("documents/{}@v1.0.md", doc.id)).exists());
}

#[test]
fn files_the_app_does_not_manage_are_left_alone() {
    let dir = tempdir().unwrap();
    write(dir.path(), "notes/my own note.md", "mine");
    write(dir.path(), "threads/alpha/notes.txt", "not a post");
    write(dir.path(), "README.md", "# my vault");
    let (mut vault, conn) = open(dir.path());
    threads::create(&conn, "New").unwrap();
    vault.persist(&conn).unwrap();
    threads::delete(&conn, "new").unwrap();
    vault.persist(&conn).unwrap();

    assert_eq!(read(dir.path(), "notes/my own note.md"), "mine");
    assert_eq!(read(dir.path(), "threads/alpha/notes.txt"), "not a post");
    assert_eq!(read(dir.path(), "README.md"), "# my vault");
}

// ---- a vault written by hand ------------------------------------------------------------------------

#[test]
fn a_hand_written_vault_loads_and_is_repaired() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // A thread folder with no thread.md; a post with no frontmatter and one with numbers where text goes.
    write(root, "threads/ideas/posts/first.md", "Just a plain markdown file.");
    write(root, "threads/ideas/posts/second.md", "---\nid: 7\nparentId: first\ncreatedAt: 2026-01-02T03:04:05.000Z\n---\n\nA reply to the plain file.\n");
    // Frontmatter that is not a mapping is not frontmatter: the file is all body.
    write(root, "threads/ideas/posts/odd.md", "---\n- a list\n- not a mapping\n---\nbody text");
    // A thread that links to one that does not exist, and to itself.
    write(root, "threads/real/thread.md", "---\ntitle: Real thread\nlinkedThreadSlugs:\n  - ideas\n  - ghost\n  - real\n---\n");

    // A document whose type is not defined, whose thread is gone, and with an unquoted version.
    write(root, "documents/doc1.md", "---\ntitle: Orphan\ndocumentTypeSlug: mystery\nthreadSlug: ghost\nversion: 1.5\n---\n\nBody.\n");
    write(root, "documents/doc1@v1.5.md", "---\ntitle: Orphan\ndocumentTypeSlug: mystery\n---\nOld body");
    write(root, "documents/doc1@vnonsense.md", "ignored");

    // A story with a column and a thread that do not exist, and criteria written in a loose style.
    write(
        root,
        "user-stories/HAND-1.md",
        "---\nsummary: By hand\ncolumnId: nowhere\nthreadSlugs: [ideas, ghost]\ncriteria:\n  - text: first\n    done: true\n  - text: second\n  - done: true\n---\n",
    );
    // Columns and the backlog written by hand.
    write(root, "planboard.md", "---\ncolumns:\n  - name: Todo\n  - id: doing\n    name: Doing\nbacklogColumnId: doing\n---\n");

    let (_, conn) = open(root);

    let ideas = threads::get(&conn, "ideas").unwrap();
    assert_eq!(ideas.title, "ideas");
    let bodies: Vec<_> = ideas.posts.iter().map(|p| (p.id.as_str(), p.body.as_str(), p.parent_id.as_deref())).collect();
    assert!(bodies.contains(&("first", "Just a plain markdown file.", None)), "{bodies:?}");
    assert!(bodies.contains(&("7", "A reply to the plain file.", Some("first"))), "{bodies:?}");
    assert!(ideas.posts.iter().any(|p| p.id == "odd" && p.body.contains("not a mapping")), "{bodies:?}");

    // Links count when either side lists them; links to nothing and to oneself are dropped.
    let real = threads::get(&conn, "real").unwrap();
    assert_eq!(real.linked_threads.len(), 1);
    assert_eq!(real.linked_threads[0].slug, "ideas");
    assert_eq!(threads::get(&conn, "ideas").unwrap().linked_threads[0].slug, "real");

    let doc = documents::get(&conn, "doc1").unwrap();
    assert_eq!((doc.document_type_slug.as_str(), doc.document_type_name.as_str()), ("mystery", "mystery"));
    assert!(doc.thread_slug.is_none());
    assert_eq!(doc.version.as_deref(), Some("1.5"));
    assert_eq!(doc.versions, ["1.5"]);
    assert_eq!(documents::get_version(&conn, "doc1", "1.5").unwrap().body, "Old body");

    let story = stories::get(&conn, "HAND-1").unwrap();
    assert_eq!(story.column_id, "doing"); // "nowhere" does not exist, so the story lands in the backlog column
    assert_eq!(story.threads.len(), 1);
    assert_eq!(story.criteria.iter().map(|c| (c.text.as_str(), c.done)).collect::<Vec<_>>(), [("first", true), ("second", false)]);

    let board = planboard::get(&conn).unwrap();
    assert_eq!(board.columns.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["todo", "doing"]);
    assert_eq!(board.backlog_column_id, "doing");

    // The repairs are written back, and a second load agrees with the first.
    assert!(root.join("threads/ideas/thread.md").exists());
    assert!(root.join("document-types/mystery.md").exists());
    let (_, again) = open(root);
    assert_eq!(snapshot(&conn), snapshot(&again));
}

#[test]
fn a_story_id_that_differs_only_in_case_is_not_loaded_twice() {
    let dir = tempdir().unwrap();
    write(dir.path(), "user-stories/AB-1.md", "---\nsummary: upper\n---\n");
    write(dir.path(), "user-stories/ab-1.md", "---\nsummary: lower\n---\n");
    let (_, conn) = open(dir.path());
    assert_eq!(stories::list(&conn).unwrap().len(), 1);
}

#[test]
fn edits_made_outside_the_app_are_noticed() {
    let dir = tempdir().unwrap();
    let (mut vault, conn) = open(dir.path());
    threads::create(&conn, "Watched").unwrap();
    vault.persist(&conn).unwrap();
    assert!(!vault.changed_on_disk());

    write(dir.path(), "threads/watched/posts/new.md", "Added in another editor.");
    assert!(vault.changed_on_disk());
    let conn = vault.load().unwrap();
    assert!(!vault.changed_on_disk());
    assert_eq!(posts::list(&conn, "watched").unwrap()[0].body, "Added in another editor.");

    fs::remove_file(dir.path().join("threads/watched/posts/new.md")).unwrap();
    assert!(vault.changed_on_disk());
}

#[test]
fn a_large_vault_persists_quickly() {
    let dir = tempdir().unwrap();
    let (mut vault, conn) = open(dir.path());
    let thread = threads::create(&conn, "Big").unwrap().slug;
    for n in 0..2000 {
        posts::add(&conn, &thread, &format!("Post number {n} with a little text in it."), AddOptions::default()).unwrap();
    }
    let started = std::time::Instant::now();
    vault.persist(&conn).unwrap();
    println!("first persist of 2000 posts: {:?}", started.elapsed());

    posts::add(&conn, &thread, "one more", AddOptions::default()).unwrap();
    let started = std::time::Instant::now();
    assert_eq!(vault.persist(&conn).unwrap(), 1);
    let elapsed = started.elapsed();
    println!("persist after one edit among 2000 posts: {elapsed:?}");
    assert!(elapsed.as_millis() < 1500, "{elapsed:?}");

    let started = std::time::Instant::now();
    let (_, reloaded) = open(dir.path());
    println!("load of 2001 posts: {:?}", started.elapsed());
    assert_eq!(posts::list(&reloaded, &thread).unwrap().len(), 2001);
}

// ---- frontmatter ------------------------------------------------------------------------------------

#[test]
fn frontmatter_round_trips_awkward_text() {
    let mut meta = serde_yaml_ng::Mapping::new();
    frontmatter::set(&mut meta, "title", "A: title # with \"quotes\" and 'apostrophes'");
    frontmatter::set(&mut meta, "version", "1.0");
    frontmatter::set(&mut meta, "flag", "true");
    frontmatter::set(&mut meta, "empty-looking", "null");
    frontmatter::set_list(&mut meta, "list", &["a, b".into(), "- c".into()]);
    frontmatter::set_opt(&mut meta, "absent", Some(""));
    let body = "Line one\n\n  indented\n\n---\n\nafter a rule";

    let parsed = frontmatter::parse(&frontmatter::render(&meta, body)).unwrap();
    assert_eq!(parsed.body, body);
    assert_eq!(parsed.meta.text("title").unwrap(), "A: title # with \"quotes\" and 'apostrophes'");
    assert_eq!(parsed.meta.text("version").unwrap(), "1.0");
    assert_eq!(parsed.meta.text("flag").unwrap(), "true");
    assert_eq!(parsed.meta.text("empty-looking").unwrap(), "null");
    assert_eq!(parsed.meta.list("list"), ["a, b", "- c"]);
    assert!(parsed.meta.text("absent").is_none());
}

#[test]
fn frontmatter_reading_is_lenient() {
    // Windows line endings, a byte-order mark, a number where text goes, a loose boolean.
    let text = "\u{feff}---\r\nid: 42\r\nversion: 1.5\r\nguidingQuestion: True\r\n---\r\n\r\nBody\r\n";
    let parsed = frontmatter::parse(text).unwrap();
    assert_eq!(parsed.meta.text("id").unwrap(), "42");
    assert_eq!(parsed.meta.text("version").unwrap(), "1.5");
    assert!(parsed.meta.flag("guidingQuestion"));
    assert_eq!(parsed.body, "Body");

    assert!(frontmatter::parse("no frontmatter here").is_none());
    assert!(frontmatter::parse("---\nnever closed: true\n").is_none());
    assert_eq!(frontmatter::parse("---\n---\nonly body").unwrap().body, "only body");
    assert_eq!(frontmatter::parse("---\nid: 1\n---\n").unwrap().body, "");
}

// ---- settings ---------------------------------------------------------------------------------------

#[test]
fn env_settings_are_read_written_and_kept_out_of_git() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    assert_eq!(settings::read_env(root), SyncEnv::default());
    assert!(settings::read_env(root).complete().is_none());

    fs::write(root.join(".env"), "# mine\nOTHER=keep me\nGITHUB_REPO = \"me/notes\"\nexport GITHUB_BRANCH='ledger'\nGITHUB_TOKEN=\n").unwrap();
    let env = settings::read_env(root);
    assert_eq!((env.repo.as_str(), env.branch.as_str(), env.token.as_str()), ("me/notes", "ledger", ""));
    assert!(env.complete().is_none(), "a missing token means sync is off");

    settings::write_env(root, &SyncEnv { repo: "me/notes".into(), branch: "main".into(), token: "github_pat_abc".into() }).unwrap();
    let text = read(root, ".env");
    assert!(text.contains("# mine") && text.contains("OTHER=keep me"), "{text}");
    assert_eq!(text.matches("GITHUB_BRANCH").count(), 1, "{text}");
    let config = settings::read_env(root).complete().unwrap();
    assert_eq!((config.repo.as_str(), config.branch.as_str(), config.token.as_str()), ("me/notes", "main", "github_pat_abc"));

    assert!(read(root, ".gitignore").lines().any(|l| l == ".env"));
}

#[test]
fn an_existing_gitignore_is_extended_not_replaced() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join(".gitignore"), "drafts/\n*.bak").unwrap();
    settings::ensure_gitignore(dir.path()).unwrap();
    settings::ensure_gitignore(dir.path()).unwrap();
    let text = read(dir.path(), ".gitignore");
    assert_eq!(text, "drafts/\n*.bak\n.env\n.ledger/\n");
}

#[test]
fn repositories_and_branches_are_normalised() {
    for good in ["me/notes", " me/notes ", "https://github.com/me/notes", "https://github.com/me/notes.git", "git@github.com:me/notes.git", "github.com/me/notes/"] {
        assert_eq!(settings::normalize_repo(good).unwrap(), "me/notes", "{good}");
    }
    for bad in ["", "notes", "me/", "/notes", "me/notes/extra", "me/no tes", "https://example.com/me/notes/x"] {
        assert!(settings::normalize_repo(bad).is_err(), "{bad:?}");
    }
    assert_eq!(settings::normalize_branch(" refs/heads/ledger ").unwrap(), "ledger");
    assert_eq!(settings::normalize_branch("feature/notes").unwrap(), "feature/notes");
    for bad in ["", "a b", "a..b", "/a"] {
        assert!(settings::normalize_branch(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn the_vault_location_is_remembered_in_the_app_config() {
    let config = tempdir().unwrap();
    let docs = tempdir().unwrap();

    let (path, locked) = settings::resolve_vault(config.path(), Some(docs.path().to_path_buf()));
    if std::env::var_os("LEDGER_VAULT").is_none() {
        assert_eq!((path, locked), (docs.path().join("Ledger"), false));
        let chosen = config.path().join("elsewhere");
        settings::update_config(config.path(), |c| c.vault_path = Some(chosen.display().to_string())).unwrap();
        settings::update_config(config.path(), |c| c.screenshot_hotkey = Some("Alt+F9".into())).unwrap();
        assert_eq!(settings::load_config(config.path()).vault_path.as_deref(), Some(chosen.to_str().unwrap()), "other settings survive an update");
        assert_eq!(settings::resolve_vault(config.path(), None).0, chosen);
    }
}

#[test]
fn moving_the_vault_copies_it_and_loads_the_new_place() {
    let config = tempdir().unwrap();
    let old = tempdir().unwrap();
    let new = tempdir().unwrap();
    let target = new.path().join("moved");

    let mut core = crate::state::Core::open_at(old.path().to_path_buf(), config.path().to_path_buf(), false).unwrap();
    threads::create(&core.conn, "Travelling").unwrap();
    core.vault.persist(&core.conn).unwrap();
    settings::write_env(old.path(), &SyncEnv { repo: "me/notes".into(), branch: "main".into(), token: "secret-token".into() }).unwrap();

    assert!(core.set_vault(Path::new("relative/path"), true).is_err());
    assert!(core.set_vault(&old.path().join("inside"), true).is_err());

    core.set_vault(&target, true).unwrap();
    assert_eq!(core.vault.root(), target);
    assert_eq!(threads::list(&core.conn).unwrap()[0].slug, "travelling");
    assert!(target.join("threads/travelling/thread.md").exists());
    assert_eq!(core.sync.as_ref().unwrap().repo, "me/notes", "the new vault's own .env decides sync");
    assert!(old.path().join("threads/travelling/thread.md").exists(), "the old vault is left where it was");

    // Without copying, an empty folder starts a fresh vault.
    let fresh = new.path().join("fresh");
    core.set_vault(&fresh, false).unwrap();
    assert!(threads::list(&core.conn).unwrap().is_empty());
    assert!(core.sync.is_none());
    assert_eq!(settings::load_config(config.path()).vault_path.unwrap(), fresh.display().to_string());

    // The planboard of a fresh vault is the default one.
    assert_eq!(db::query_all(&core.conn, "SELECT name FROM planboard_columns ORDER BY position", [], |r| r.get::<_, String>(0)).unwrap(), ["Backlog", "Drafting", "Documented"]);
}
