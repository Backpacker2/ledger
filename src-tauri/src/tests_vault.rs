//! The vault: what the files look like, that they round-trip, that they are written sparingly, that a vault
//! written by hand loads without complaint, and where the settings and the secret live.

use std::fs;
use std::path::Path;

use rusqlite::Connection;
use tempfile::tempdir;

use crate::frontmatter::{self, MetaExt};
use crate::models::{Bump, Criterion, StoryDraft, TaskUpdate};
use crate::posts::AddOptions;
use crate::provenance::{self, Origin};
use crate::settings::{self, SyncEnv};
use crate::vault::{self, Vault};
use crate::{board, db, deliverables, feedback, posts, projects, prompts, stories, templates, threads, uploads};

fn write(root: &Path, rel: &str, text: &str) {
    let file = root.join(rel);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, text).unwrap();
}

fn read(root: &Path, rel: &str) -> String {
    fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn names(root: &Path, rel: &str) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(root.join(rel)).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    names.sort();
    names
}

fn open(root: &Path) -> (Vault, Connection) {
    let mut vault = Vault::open(root.to_path_buf()).unwrap();
    let conn = vault.load().unwrap();
    (vault, conn)
}

/// A vault with a bit of everything in it. Returns the project and thread slugs.
fn populate(conn: &Connection, root: &Path) -> (String, String) {
    templates::create(conn, "", "Interview", "after a call", &["Who?".into(), "Why: really?".into()], "# Interview\n\n## Who\n").unwrap();
    let project = projects::create(conn, "Acme").unwrap().slug;
    templates::create(conn, &project, "House style", "", &[], "Short sentences.").unwrap();
    projects::save_readme(conn, &project, "# Acme\n\nRead the call notes first.\n").unwrap();

    let alpha = threads::create(conn, &project, "Alpha: the first").unwrap().slug;
    let beta = threads::create(conn, &project, "Beta").unwrap().slug;
    threads::link(conn, &project, &alpha, &beta).unwrap();

    let first = posts::add(conn, &project, &alpha, "A note with **bold**\n\nand a second paragraph.", AddOptions::default()).unwrap();
    posts::add(conn, &project, &alpha, "A reply", AddOptions { reply_to_id: Some(first.id), ..Default::default() }).unwrap();

    let prompt = prompts::create(conn, &project, &alpha, "Call summary", "Write up the call with Ana.", Some("interview")).unwrap();
    let plan = deliverables::create(conn, &project, "Call summary", "# Call summary\n\nAna wants SSO.\n").unwrap();
    prompts::link_deliverable(conn, &project, &alpha, &prompt.id, &plan.path).unwrap();
    deliverables::publish(conn, root, &project, &plan.path, Bump::Major, None).unwrap();
    feedback::add(conn, &project, &beta, &plan.path, "Add the date of the call.").unwrap();

    uploads::add_bytes(conn, root, &project, &alpha, "site plan.png", b"\x89PNG....", "The site plan").unwrap();

    stories::create(
        conn,
        &project,
        StoryDraft {
            id: "MM01".into(),
            summary: "Clarify pricing".into(),
            role: "customer".into(),
            want: "see prices".into(),
            reason: "I can decide".into(),
            notes: "Raised on the first call.".into(),
            criteria: vec![Criterion { text: "A page lists plans".into(), done: true }, Criterion { text: "Prices in euro".into(), done: false }],
            thread_slugs: vec![alpha.clone(), beta],
        },
    )
    .unwrap();
    let task = board::create_task(conn, &project, "Draw the pricing page", "In figma.", Some("MM01"), Some(&alpha)).unwrap();
    board::update_task(conn, &project, &task.id, TaskUpdate { column_id: Some("doing".into()), ..Default::default() }).unwrap();
    board::add_column(conn, &project, "In review").unwrap();
    (project, alpha)
}

/// Everything the screens would show, as one string.
fn snapshot(conn: &Connection, root: &Path) -> String {
    let mut out = String::new();
    for p in projects::list(conn).unwrap() {
        out += &format!("{p:?}\n{:?}\n", projects::get(conn, &p.slug).unwrap());
        for t in &p.threads {
            out += &format!("{:?}\n", threads::get(conn, root, &p.slug, &t.slug).unwrap());
        }
        // (a file's modified time is the disk's to say, so it is left out)
        for d in deliverables::list(conn, &p.slug).unwrap() {
            for path in std::iter::once(d.path.clone()).chain(d.versions.iter().map(|v| deliverables::snapshot_path(&d.path, v))) {
                let v = deliverables::get(conn, &p.slug, &path).unwrap();
                out += &format!("{} {} {:?} {:?} {:?} {:?} {:?}\n", v.path, v.kind, v.version, v.versions, v.body, v.prompts, v.feedback);
            }
        }
        out += &format!("{:?}\n{:?}\n{:?}\n", stories::list(conn, &p.slug).unwrap(), board::get(conn, &p.slug).unwrap(), templates::for_project(conn, &p.slug).unwrap());
    }
    out + &format!("{:?}\n", templates::list(conn, "").unwrap())
}

// ---- what the files look like -----------------------------------------------------------------------

#[test]
fn the_vault_has_the_agreed_folder_structure() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let (mut vault, conn) = open(root);
    let (p, t) = populate(&conn, root);
    vault.persist(&conn).unwrap();
    let project = format!("projects/{p}");
    let thread = format!("{project}/threads/{t}");

    // The project: its definition, its reading guide, and the folders hanging off it.
    assert_eq!(names(root, &project), ["README.md", "attachments", "deliverables", "project.md", "tasks", "templates", "threads", "userstories", "versions"]);
    let definition = read(root, &format!("{project}/project.md"));
    assert!(definition.contains("name: Acme") && definition.contains("- id: backlog") && definition.contains("name: In review"), "{definition}");
    assert_eq!(read(root, &format!("{project}/README.md")), "# Acme\n\nRead the call notes first.\n");

    // A thread, and what is kept in it.
    assert_eq!(names(root, &thread), ["posts", "prompts", "thread.md", "uploads"]);
    let meta = read(root, &format!("{thread}/thread.md"));
    assert!(meta.contains("title: 'Alpha: the first'") || meta.contains("title: \"Alpha: the first\""), "{meta}");
    assert!(meta.contains("linkedThreadSlugs:") && meta.contains("- beta"), "{meta}");

    let posts = names(root, &format!("{thread}/posts"));
    assert_eq!(posts.len(), 5, "{posts:?}"); // two notes, two seeded questions, and the note that a version was published
    let note = posts.iter().map(|f| read(root, &format!("{thread}/posts/{f}"))).find(|x| x.contains("**bold**")).unwrap();
    assert!(note.contains("createdAt:") && note.ends_with("and a second paragraph.\n"), "{note}");

    // A prompt, with the deliverables it produced.
    let prompt_files = names(root, &format!("{thread}/prompts"));
    assert_eq!(prompt_files.len(), 1);
    let prompt = read(root, &format!("{thread}/prompts/{}", prompt_files[0]));
    assert!(prompt.contains("title: Call summary") && prompt.contains("template: interview") && prompt.contains("- call-summary.md"), "{prompt}");
    assert!(prompt.ends_with("Write up the call with Ana.\n"));

    // Feedback lives in the thread it was written in and names the deliverable.
    let beta = format!("{project}/threads/beta/feedback");
    let feedback_file = names(root, &beta);
    let text = read(root, &format!("{beta}/{}", feedback_file[0]));
    assert!(text.contains("deliverable: call-summary.md") && text.contains("status: open") && text.ends_with("Add the date of the call.\n"), "{text}");

    // An upload: a manifest in the thread, the file itself in the project's attachments.
    let manifest_name = &names(root, &format!("{thread}/uploads"))[0];
    let manifest = read(root, &format!("{thread}/uploads/{manifest_name}"));
    assert!(manifest.contains("uploadedAt:") && manifest.contains("file: attachments/site plan.png") && manifest.ends_with("The site plan\n"), "{manifest}");
    assert_eq!(fs::read(root.join(&project).join("attachments/site plan.png")).unwrap(), b"\x89PNG....");

    // Deliverables are written verbatim; a published version is tagged @v1.0.
    assert_eq!(names(root, &format!("{project}/deliverables")), ["call-summary.md", "call-summary@v1.0.md"]);
    assert_eq!(read(root, &format!("{project}/deliverables/call-summary.md")), "# Call summary\n\nAna wants SSO.\n");

    // How the published version came to be: one file for it, next to the project's other notes.
    let record_files = names(root, &format!("{project}/versions"));
    assert_eq!(record_files.len(), 1, "{record_files:?}");
    let record = read(root, &format!("{project}/versions/{}", record_files[0]));
    assert!(
        record.contains("deliverable: call-summary.md") && record.contains("version: '1.0'") && record.contains("thread: alpha-the-first") && record.contains("by: human") && record.contains("sha256: "),
        "{record}"
    );
    assert!(!record.contains("prompt:") && !record.contains("run:") && !record.contains("mmm:"), "what isn't known is left out: {record}");

    // Stories and tasks, and the two kinds of template.
    let story = read(root, &format!("{project}/userstories/MM01.md"));
    assert!(story.contains("text: A page lists plans") && story.contains("done: true") && story.contains("- alpha-the-first") && story.ends_with("Raised on the first call.\n"), "{story}");
    let task_name = &names(root, &format!("{project}/tasks"))[0];
    let task = read(root, &format!("{project}/tasks/{task_name}"));
    assert!(task.contains("title: Draw the pricing page") && task.contains("columnId: doing") && task.contains("story: MM01") && task.contains("thread: alpha-the-first") && task.ends_with("In figma.\n"), "{task}");
    assert_eq!(names(root, &format!("{project}/templates")), ["house-style.md"]);
    let global = read(root, "templates/interview.md");
    assert!(global.contains("name: Interview") && global.contains("trigger: after a call") && global.contains("- Who?") && global.ends_with("## Who\n"), "{global}");

    // Settings and the secret are kept apart from the notes, and out of git.
    let ignore = read(root, ".gitignore");
    assert!(ignore.lines().any(|l| l == ".ledger/") && ignore.lines().any(|l| l == ".env"), "{ignore}");
}

#[test]
fn what_is_written_loads_back_identically() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let (mut vault, conn) = open(root);
    populate(&conn, root);
    vault.persist(&conn).unwrap();

    let (mut again, reloaded) = open(root);
    assert_eq!(vault::export(&conn).unwrap(), vault::export(&reloaded).unwrap());
    assert_eq!(snapshot(&conn, root), snapshot(&reloaded, root));
    // Loading a vault that is already in order writes nothing.
    assert_eq!(again.persist(&reloaded).unwrap(), 0);
}

#[test]
fn things_made_in_the_same_instant_keep_their_order_after_a_reload() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let (mut vault, conn) = open(root);
    let questions: Vec<String> = (1..=12).map(|n| format!("Question {n}?")).collect();
    templates::create(&conn, "", "Many", "", &questions, "").unwrap();
    let p = projects::create(&conn, "P").unwrap().slug;
    let t = threads::create(&conn, &p, "T").unwrap().slug;
    prompts::create(&conn, &p, &t, "", "Answer them all", Some("many")).unwrap();
    vault.persist(&conn).unwrap();

    let (_, reloaded) = open(root);
    let bodies: Vec<_> = posts::list(&reloaded, &p, &t).unwrap().into_iter().map(|x| x.body).collect();
    let expected: Vec<_> = questions.iter().map(|q| format!("**Guiding question (Many):** {q}")).collect();
    assert_eq!(bodies, expected);
    assert_eq!(prompts::list(&reloaded, &p, &t).unwrap().len(), 1);
}

// ---- writing as little as possible ------------------------------------------------------------------

#[test]
fn persisting_touches_only_what_changed() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let (mut vault, conn) = open(root);
    let (p, t) = populate(&conn, root);
    assert!(vault.persist(&conn).unwrap() >= 15, "everything populate made is new");
    assert_eq!(vault.persist(&conn).unwrap(), 0);

    let post = posts::add(&conn, &p, &t, "one more", AddOptions::default()).unwrap();
    assert_eq!(vault.persist(&conn).unwrap(), 1);
    assert_eq!(vault.persist(&conn).unwrap(), 0);

    posts::update(&conn, &p, &t, &post.id, "edited").unwrap();
    assert_eq!(vault.persist(&conn).unwrap(), 1);
    assert!(read(root, &format!("projects/{p}/threads/{t}/posts/{}.md", post.id)).contains("edited"));

    projects::save_readme(&conn, &p, "# New guide\n").unwrap();
    deliverables::save(&conn, &p, "call-summary.md", "# Call summary\n\nEdited.\n").unwrap();
    assert_eq!(vault.persist(&conn).unwrap(), 3, "the reading guide, the project's updated time, and the deliverable");
    assert_eq!(read(root, &format!("projects/{p}/deliverables/call-summary.md")), "# Call summary\n\nEdited.\n");
}

#[test]
fn deleting_things_deletes_their_files_and_leaves_no_empty_folders() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let (mut vault, conn) = open(root);
    let (p, _) = populate(&conn, root);
    vault.persist(&conn).unwrap();
    assert!(root.join(format!("projects/{p}/threads/beta/thread.md")).exists());

    threads::delete(&conn, root, &p, "beta").unwrap();
    vault.persist(&conn).unwrap();
    assert!(!root.join(format!("projects/{p}/threads/beta")).exists(), "feedback and links went with the thread");

    stories::delete(&conn, &p, "MM01").unwrap();
    vault.persist(&conn).unwrap();
    assert!(!root.join(format!("projects/{p}/userstories")).exists());

    // Deleting the current file keeps its published versions.
    deliverables::delete(&conn, root, &p, "call-summary.md").unwrap();
    vault.persist(&conn).unwrap();
    assert_eq!(names(root, &format!("projects/{p}/deliverables")), ["call-summary@v1.0.md"]);
}

#[test]
fn deleting_a_project_removes_its_folder_with_the_files_the_database_does_not_own() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let (mut vault, conn) = open(root);
    let (p, _) = populate(&conn, root);
    let other = projects::create(&conn, "Other").unwrap().slug;
    let folder = deliverables::deliverables_dir(root, &p);
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("chart.png"), [1u8]).unwrap();
    vault.persist(&conn).unwrap();

    projects::delete(&conn, &p).unwrap();
    vault.persist(&conn).unwrap();
    assert!(root.join(format!("projects/{p}/attachments/site plan.png")).exists(), "attachments are not the database's to delete");
    vault.remove_project_dir(&p).unwrap();
    assert!(!root.join(format!("projects/{p}")).exists());
    assert!(root.join(format!("projects/{other}/project.md")).exists());
    assert!(root.join("templates/interview.md").exists(), "vault-wide templates stay");
}

#[test]
fn files_the_app_does_not_manage_are_left_alone() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    write(root, "notes/my own note.md", "mine");
    write(root, "README.md", "# my vault");
    write(root, "projects/acme/notes.md", "loose note");
    write(root, "projects/acme/attachments/readme.md", "an uploaded markdown file, not a note");
    let (mut vault, conn) = open(root);
    assert!(!vault.changed_on_disk());

    let t = threads::create(&conn, "acme", "New").unwrap().slug;
    vault.persist(&conn).unwrap();
    threads::delete(&conn, root, "acme", &t).unwrap();
    vault.persist(&conn).unwrap();

    assert_eq!(read(root, "notes/my own note.md"), "mine");
    assert_eq!(read(root, "README.md"), "# my vault");
    assert_eq!(read(root, "projects/acme/notes.md"), "loose note");
    assert_eq!(read(root, "projects/acme/attachments/readme.md"), "an uploaded markdown file, not a note");
    assert!(threads::list(&conn, "acme").unwrap().is_empty(), "an attachment is never read as a note");

    // Attachments aren't watched; a deliverable is.
    write(root, "projects/acme/attachments/new.png", "x");
    assert!(!vault.changed_on_disk());
    write(root, "projects/acme/deliverables/report.pdf", "%PDF");
    assert!(vault.changed_on_disk());
}

// ---- a vault written by hand ------------------------------------------------------------------------

#[test]
fn a_hand_written_vault_loads_and_is_repaired() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // A project with no project.md, whose threads have no thread.md; a post with no frontmatter and one with
    // numbers where text goes.
    write(root, "projects/ideas/threads/first/posts/a.md", "Just a plain markdown file.");
    write(root, "projects/ideas/threads/first/posts/b.md", "---\nid: 7\nparentId: a\ncreatedAt: 2026-01-02T03:04:05.000Z\n---\n\nA reply to the plain file.\n");
    write(root, "projects/ideas/threads/first/posts/odd.md", "---\n- a list\n- not a mapping\n---\nbody text");
    write(root, "projects/ideas/threads/second/thread.md", "---\ntitle: Second thread\nlinkedThreadSlugs:\n  - first\n  - ghost\n  - second\n---\n");

    // A prompt from a template that does not exist, listing a deliverable that is there and one that is not.
    write(root, "projects/ideas/threads/first/prompts/p1.md", "---\ntitle: Brief\ntemplate: ghost\ndeliverables:\n  - x.md\n  - missing.pdf\n---\n\nWrite the brief.\n");
    write(root, "projects/ideas/threads/first/prompts/p2.md", "---\ntitle: empty\n---\n");
    // Feedback needs a deliverable; the status is lenient.
    write(root, "projects/ideas/threads/first/feedback/f1.md", "---\nstatus: open\n---\nFeedback on nothing.");
    write(root, "projects/ideas/threads/first/feedback/f2.md", "---\ndeliverable: x.md\nstatus: weird\n---\n\nToo long.\n");
    write(root, "projects/ideas/threads/first/uploads/u1.md", "---\nfile: attachments/a.png\nuploadedAt: 2026-02-03T04:05:06.000Z\n---\n\nA diagram.\n");
    write(root, "projects/ideas/threads/first/uploads/u2.md", "---\nuploadedAt: 2026-02-03T04:05:06.000Z\n---\nno file, so no upload");

    // Deliverables of every kind, nested ones included; a markdown one keeps its own frontmatter.
    write(root, "projects/ideas/deliverables/x.md", "---\ntitle: X\nauthor: me\n---\n\n# The brief\n");
    write(root, "projects/ideas/deliverables/x@v1.0.md", "# The brief, v1\n");
    write(root, "projects/ideas/deliverables/sub/data.csv", "a,b\n1,2\n");
    write(root, "projects/ideas/deliverables/diagram.png", "not really a png");

    // A story and tasks pointing at things that don't exist.
    write(root, "projects/ideas/userstories/HAND-1.md", "---\nsummary: By hand\nthreadSlugs: [first, ghost]\ncriteria:\n  - text: first\n    done: true\n  - text: second\n  - done: true\n---\n");
    write(root, "projects/ideas/tasks/t1.md", "---\ntitle: Do it\ncolumnId: nowhere\nstory: ghost\nthread: ghost\n---\nWith notes.");
    write(root, "projects/ideas/tasks/t2.md", "---\ncolumnId: doing\n---\nno title, so no task");
    write(root, "projects/ideas/project.md", "---\nname: Ideas\ncolumns:\n  - name: Todo\n  - id: doing\n    name: Doing\nbacklogColumnId: doing\n---\nAbout the project.\n");

    // A project that is nothing but a reading guide, and templates in both scopes.
    write(root, "projects/bare/README.md", "# Bare\n\nJust a guide.");
    write(root, "templates/memo.md", "---\nname: Memo\nquestions: Who is it for?\n---\n\n# Memo\n");
    write(root, "projects/ideas/templates/memo.md", "---\nname: Ideas memo\n---\nProject memo.");

    let (_, conn) = open(root);

    let listed: Vec<_> = projects::list(&conn).unwrap().into_iter().map(|p| (p.slug, p.name)).collect();
    assert_eq!(listed, [("bare".to_string(), "bare".to_string()), ("ideas".to_string(), "Ideas".to_string())]);
    assert_eq!(projects::get(&conn, "bare").unwrap().readme, "# Bare\n\nJust a guide.");
    assert_eq!(projects::get(&conn, "ideas").unwrap().description, "About the project.");
    assert!(projects::get(&conn, "ideas").unwrap().readme.starts_with("# Ideas"), "a missing README is started");

    let first = threads::get(&conn, root, "ideas", "first").unwrap();
    assert_eq!(first.title, "first");
    let bodies: Vec<_> = first.posts.iter().map(|p| (p.id.as_str(), p.body.as_str(), p.parent_id.as_deref())).collect();
    assert!(bodies.contains(&("a", "Just a plain markdown file.", None)), "{bodies:?}");
    assert!(bodies.contains(&("7", "A reply to the plain file.", Some("a"))), "{bodies:?}");
    assert!(first.posts.iter().any(|p| p.id == "odd" && p.body.contains("not a mapping")));
    let second = threads::get(&conn, root, "ideas", "second").unwrap();
    assert_eq!((second.title.as_str(), second.linked_threads.len(), first.linked_threads.len()), ("Second thread", 1, 1), "links count from either side and dangling ones are dropped");

    assert_eq!(first.prompts.len(), 1, "an empty prompt is not a prompt");
    let prompt = &first.prompts[0];
    assert_eq!((prompt.title.as_str(), prompt.template_id.as_deref(), prompt.template_name.as_deref()), ("Brief", Some("ghost"), None));
    assert_eq!(prompt.deliverables.iter().map(|d| (d.path.as_str(), d.exists)).collect::<Vec<_>>(), [("x.md", true), ("missing.pdf", false)]);
    assert_eq!(first.feedback.len(), 1);
    assert_eq!((first.feedback[0].deliverable_path.as_str(), first.feedback[0].status.as_str()), ("x.md", "open"));
    assert_eq!(first.uploads.len(), 1);
    assert_eq!((first.uploads[0].name.as_str(), first.uploads[0].description.as_str(), first.uploads[0].exists, first.uploads[0].image), ("a.png", "A diagram.", false, true));

    let listed: Vec<_> = deliverables::list(&conn, "ideas").unwrap().into_iter().map(|d| (d.path, d.kind, d.versions)).collect();
    assert_eq!(
        listed,
        [
            ("sub/data.csv".to_string(), "other".to_string(), vec![]),
            ("diagram.png".to_string(), "image".to_string(), vec![]),
            ("x.md".to_string(), "markdown".to_string(), vec!["1.0".to_string()]),
        ]
    );
    let x = deliverables::get(&conn, "ideas", "x.md").unwrap();
    assert_eq!(x.body.as_deref(), Some("---\ntitle: X\nauthor: me\n---\n\n# The brief\n"), "markdown deliverables are kept verbatim");
    let html = x.html.unwrap();
    assert!(html.contains("<h1>The brief</h1>") && !html.contains("author: me"), "the page shows the document, not its frontmatter");
    assert!(deliverables::get(&conn, "ideas", "diagram.png").unwrap().body.is_none());

    let story = stories::get(&conn, "ideas", "HAND-1").unwrap();
    assert_eq!(story.threads.len(), 1);
    assert_eq!(story.criteria.iter().map(|c| (c.text.as_str(), c.done)).collect::<Vec<_>>(), [("first", true), ("second", false)]);

    let board = board::get(&conn, "ideas").unwrap();
    assert_eq!(board.columns.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["todo", "doing"]);
    assert_eq!(board.backlog_column_id, "doing");
    assert_eq!(board.tasks.len(), 1, "a task without a title is skipped");
    let task = &board.tasks[0];
    assert_eq!((task.title.as_str(), task.column_id.as_str(), task.story_id.clone(), task.thread_slug.clone(), task.body.as_str()), ("Do it", "doing", None, None, "With notes."));

    assert_eq!(templates::get(&conn, "", "memo").unwrap().questions, ["Who is it for?"]);
    assert_eq!(templates::find(&conn, "ideas", "memo").unwrap().name, "Ideas memo");
    assert!(templates::find(&conn, "bare", "memo").unwrap().scope.is_empty());

    // The repairs are written back, and a second load agrees with the first.
    assert!(root.join("projects/ideas/threads/first/thread.md").exists());
    assert!(root.join("projects/bare/project.md").exists());
    assert!(root.join("projects/ideas/README.md").exists());
    let (mut again_vault, again) = open(root);
    assert_eq!(snapshot(&conn, root), snapshot(&again, root));
    assert_eq!(again_vault.persist(&again).unwrap(), 0);
    assert_eq!(read(root, "projects/ideas/deliverables/x.md"), "---\ntitle: X\nauthor: me\n---\n\n# The brief\n", "not rewritten");
}

#[test]
fn a_story_id_that_differs_only_in_case_is_not_loaded_twice() {
    let dir = tempdir().unwrap();
    write(dir.path(), "projects/p/userstories/AB-1.md", "---\nsummary: upper\n---\n");
    write(dir.path(), "projects/p/userstories/ab-1.md", "---\nsummary: lower\n---\n");
    let (_, conn) = open(dir.path());
    assert_eq!(stories::list(&conn, "p").unwrap().len(), 1);
}

#[test]
fn edits_made_outside_the_app_are_noticed() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let (mut vault, conn) = open(root);
    projects::create(&conn, "Watched").unwrap();
    threads::create(&conn, "watched", "T").unwrap();
    vault.persist(&conn).unwrap();
    assert!(!vault.changed_on_disk());

    write(root, "projects/watched/threads/t/posts/new.md", "Added in another editor.");
    assert!(vault.changed_on_disk());
    let conn = vault.load().unwrap();
    assert!(!vault.changed_on_disk());
    assert_eq!(posts::list(&conn, "watched", "t").unwrap()[0].body, "Added in another editor.");

    fs::remove_file(root.join("projects/watched/threads/t/posts/new.md")).unwrap();
    assert!(vault.changed_on_disk());
}

#[test]
fn a_large_vault_persists_quickly() {
    let dir = tempdir().unwrap();
    let (mut vault, conn) = open(dir.path());
    let p = projects::create(&conn, "Big").unwrap().slug;
    let t = threads::create(&conn, &p, "Big").unwrap().slug;
    for n in 0..2000 {
        posts::add(&conn, &p, &t, &format!("Post number {n} with a little text in it."), AddOptions::default()).unwrap();
    }
    let started = std::time::Instant::now();
    vault.persist(&conn).unwrap();
    println!("first persist of 2000 posts: {:?}", started.elapsed());

    posts::add(&conn, &p, &t, "one more", AddOptions::default()).unwrap();
    let started = std::time::Instant::now();
    assert_eq!(vault.persist(&conn).unwrap(), 1);
    let elapsed = started.elapsed();
    println!("persist after one edit among 2000 posts: {elapsed:?}");
    assert!(elapsed.as_millis() < 1500, "{elapsed:?}");

    let started = std::time::Instant::now();
    let (_, reloaded) = open(dir.path());
    println!("load of 2001 posts: {:?}", started.elapsed());
    assert_eq!(posts::list(&reloaded, &p, &t).unwrap().len(), 2001);
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

// ---- settings and secrets ---------------------------------------------------------------------------

#[test]
fn settings_live_in_the_ledger_folder_and_the_token_in_its_env_file() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    assert_eq!(settings::read_env(root), SyncEnv::default());
    assert!(settings::read_env(root).complete().is_none());

    settings::update_settings(root, |s| s.screenshot_hotkey = Some("Alt+F9".into())).unwrap();
    settings::write_env(root, &SyncEnv { repo: "me/notes".into(), branch: "main".into(), token: "github_pat_abc".into() }).unwrap();

    // Repository and branch are settings; the token is the secret and nothing else is in its file.
    let json = read(root, ".ledger/settings.json");
    assert!(json.contains("\"repo\": \"me/notes\"") && json.contains("\"branch\": \"main\"") && json.contains("Alt+F9"), "{json}");
    assert!(!json.contains("github_pat_abc"), "the token never goes in settings.json");
    let secrets = read(root, ".ledger/.env");
    assert!(secrets.contains("GITHUB_TOKEN=github_pat_abc") && !secrets.contains("me/notes"), "{secrets}");

    let config = settings::read_env(root).complete().unwrap();
    assert_eq!((config.repo.as_str(), config.branch.as_str(), config.token.as_str()), ("me/notes", "main", "github_pat_abc"));
    assert_eq!(settings::read_settings(root).screenshot_hotkey.as_deref(), Some("Alt+F9"), "saving sync settings keeps the others");

    // Neither file is ever pushed.
    assert!(read(root, ".gitignore").lines().any(|l| l == ".ledger/"));
    let ignore = crate::sync::Ignore::load(root);
    assert!(ignore.is_ignored(".ledger/settings.json", false) && ignore.is_ignored(".ledger/.env", false));

    // Turning sync off removes the token and leaves the rest.
    settings::write_env(root, &SyncEnv { repo: "me/notes".into(), branch: "main".into(), token: String::new() }).unwrap();
    assert!(settings::read_env(root).complete().is_none());
    assert_eq!(settings::read_env(root).repo, "me/notes");
}

#[test]
fn an_env_file_keeps_lines_that_are_not_the_token() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    write(root, ".ledger/.env", "# mine\nOTHER=keep me\nexport GITHUB_TOKEN = \"old-token\"\n");
    assert_eq!(settings::read_env(root).token, "old-token");

    settings::write_env(root, &SyncEnv { repo: "me/notes".into(), branch: "main".into(), token: "new-token".into() }).unwrap();
    let text = read(root, ".ledger/.env");
    assert!(text.contains("# mine") && text.contains("OTHER=keep me"), "{text}");
    assert_eq!(text.matches("GITHUB_TOKEN").count(), 1, "{text}");
    assert_eq!(settings::read_env(root).token, "new-token");
}

#[test]
fn an_existing_gitignore_is_extended_not_replaced() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join(".gitignore"), "drafts/\n*.bak").unwrap();
    settings::ensure_gitignore(dir.path()).unwrap();
    settings::ensure_gitignore(dir.path()).unwrap();
    assert_eq!(read(dir.path(), ".gitignore"), "drafts/\n*.bak\n.ledger/\n.env\n");
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
    projects::create(&core.conn, "Travelling").unwrap();
    core.vault.persist(&core.conn).unwrap();
    settings::write_env(old.path(), &SyncEnv { repo: "me/notes".into(), branch: "main".into(), token: "secret-token".into() }).unwrap();

    assert!(core.set_vault(Path::new("relative/path"), true).is_err());
    assert!(core.set_vault(&old.path().join("inside"), true).is_err());

    core.set_vault(&target, true).unwrap();
    assert_eq!(core.vault.root(), target);
    assert_eq!(projects::list(&core.conn).unwrap()[0].slug, "travelling");
    assert!(target.join("projects/travelling/project.md").exists());
    assert_eq!(core.sync.as_ref().unwrap().repo, "me/notes", "the new vault's own .ledger folder decides sync");
    assert!(old.path().join("projects/travelling/project.md").exists(), "the old vault is left where it was");

    // Without copying, an empty folder starts a fresh vault.
    let fresh = new.path().join("fresh");
    core.set_vault(&fresh, false).unwrap();
    assert!(projects::list(&core.conn).unwrap().is_empty());
    assert!(core.sync.is_none());
    assert_eq!(settings::load_config(config.path()).vault_path.unwrap(), fresh.display().to_string());
    assert!(db::query_all(&core.conn, "SELECT slug FROM projects", [], |r| r.get::<_, String>(0)).unwrap().is_empty());
}

// ---- how versions came to be ------------------------------------------------------------------------

#[test]
fn version_records_and_the_prompt_a_note_names_survive_a_reload() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let (mut vault, conn) = open(root);
    let p = projects::create(&conn, "Acme").unwrap().slug;
    let t = threads::create(&conn, &p, "Call").unwrap().slug;
    let prompt = prompts::create(&conn, &p, &t, "Summary", "Write it.", None).unwrap();
    deliverables::create(&conn, &p, "Plan", "# Plan\n\ntext\n").unwrap();
    let origin = Origin {
        mmm: Some("piggybacker".into()),
        note: "First draft by the run.".into(),
        ..Origin::run(&t, &prompt.id, Some("run-x"))
    };
    deliverables::publish_run(&conn, root, &p, "plan.md", None, &origin).unwrap();
    vault.persist(&conn).unwrap();

    let record_name = &names(root, &format!("projects/{p}/versions"))[0];
    let record = read(root, &format!("projects/{p}/versions/{record_name}"));
    assert!(
        record.contains("deliverable: plan.md")
            && record.contains(&format!("prompt: {}", prompt.id))
            && record.contains("run: run-x")
            && record.contains("by: ai")
            && record.contains("mmm: piggybacker")
            && record.contains("thread: call")
            && record.ends_with("First draft by the run.\n"),
        "{record}"
    );
    let note_name = names(root, &format!("projects/{p}/threads/{t}/posts")).into_iter().find(|n| read(root, &format!("projects/{p}/threads/{t}/posts/{n}")).contains("versionPublished")).unwrap();
    let note = read(root, &format!("projects/{p}/threads/{t}/posts/{note_name}"));
    assert!(note.contains(&format!("prompt: {}", prompt.id)) && note.contains("deliverableVersion: '0.1'"), "{note}");

    let (mut again, reloaded) = open(root);
    assert_eq!(vault::export(&conn).unwrap(), vault::export(&reloaded).unwrap());
    assert_eq!(again.persist(&reloaded).unwrap(), 0);
    let known = provenance::resolve(&reloaded, &p, "plan.md").unwrap();
    assert_eq!((known[0].author, known[0].run.as_deref(), known[0].mmm.as_deref(), known[0].note.as_str()), ("ai", Some("run-x"), Some("piggybacker"), "First draft by the run."));
    assert_eq!(posts::list(&reloaded, &p, &t).unwrap().iter().find(|x| x.version_published).unwrap().prompt_title.as_deref(), Some("Summary"));
}

#[test]
fn hand_written_version_records_are_read_leniently() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let project = "projects/acme";
    write(root, &format!("{project}/project.md"), "---\nname: Acme\n---\n");
    write(root, &format!("{project}/threads/call/thread.md"), "---\ntitle: Call\n---\n");
    for name in ["plan.md", "plan@v0.1.md", "plan@v0.2.md"] {
        write(root, &format!("{project}/deliverables/{name}"), &format!("# {name}\n"));
    }
    // Upper case and strangers are read as what they mean; an unknown level or author is ignored, not an error.
    write(root, &format!("{project}/versions/a.md"), "---\nid: a\ndeliverable: plan.md\nversion: '0.1'\nby: AI\nmmm: Cyborg\nthread: call\ncreatedAt: 2026-01-01T00:00:00.000Z\nsomething: else\n---\n\nA note.\n");
    write(root, &format!("{project}/versions/b.md"), "---\nid: b\ndeliverable: plan.md\nversion: '0.2'\nby: robot\nmmm: nonsense\n---\n");
    // Not loaded: a version that is not there, one that names no deliverable, a second record of the same version.
    write(root, &format!("{project}/versions/c.md"), "---\nid: c\ndeliverable: plan.md\nversion: '0.9'\nby: human\n---\n");
    write(root, &format!("{project}/versions/d.md"), "---\nid: d\nversion: '0.1'\nby: human\n---\n");
    write(root, &format!("{project}/versions/e.md"), "---\nid: e\ndeliverable: plan.md\nversion: '0.1'\nby: human\ncreatedAt: 2026-06-01T00:00:00.000Z\n---\n");

    let (_vault, conn) = open(root);
    let known = provenance::resolve(&conn, "acme", "plan.md").unwrap();
    assert_eq!(known.len(), 2);
    assert_eq!((known[0].author, known[0].mmm.as_deref(), known[0].thread.as_deref(), known[0].note.as_str()), ("ai", Some("cyborg"), Some("call"), "A note."), "the older record of 0.1 wins");
    assert_eq!((known[1].author, known[1].mmm.as_deref()), ("", None));
    for stray in ["c", "d", "e"] {
        assert!(root.join(format!("{project}/versions/{stray}.md")).is_file(), "{stray} is left where it was");
    }
}

#[test]
fn a_vault_from_before_version_records_loads_as_it_was_and_is_not_rewritten() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let project = "projects/acme";
    write(root, &format!("{project}/project.md"), "---\nname: Acme\n---\n");
    write(root, &format!("{project}/threads/call/thread.md"), "---\ntitle: Call\n---\n");
    write(root, &format!("{project}/deliverables/plan.md"), "# Plan\n\ntwo\n");
    write(root, &format!("{project}/deliverables/plan@v0.1.md"), "# Plan\n\none\n");
    write(
        root,
        &format!("{project}/threads/call/posts/n1.md"),
        "---\nid: n1\nversionPublished: true\ndeliverable: plan.md\ndeliverableVersion: '0.1'\ncreatedAt: 2026-01-01T00:00:00.000Z\n---\n\nVersion 0.1 of **plan.md** was just published.\n",
    );

    let (_vault, conn) = open(root);
    assert!(!root.join(format!("{project}/versions")).exists(), "nothing is written for versions that have no record");
    let known = provenance::resolve(&conn, "acme", "plan.md").unwrap();
    assert_eq!((known[0].author, known[0].thread.as_deref(), known[0].record.is_none()), ("", Some("call"), true));
    assert_eq!(posts::list(&conn, "acme", "call").unwrap()[0].prompt_id, None);

    let (mut again, reloaded) = open(root);
    assert_eq!(again.persist(&reloaded).unwrap(), 0);
    assert!(read(root, &format!("{project}/threads/call/posts/n1.md")).starts_with("---\nid: n1\nversionPublished: true"));
}

#[test]
fn the_config_of_the_old_identifier_is_brought_over_once() {
    let base = tempdir().unwrap();
    let old = base.path().join(settings::OLD_IDENTIFIER);
    let new = base.path().join("nl.openledger.ledger");
    fs::create_dir_all(&old).unwrap();
    fs::write(old.join("config.json"), r#"{"vaultPath":"/somewhere/else"}"#).unwrap();

    assert!(settings::adopt_old_config(&new, settings::OLD_IDENTIFIER));
    assert_eq!(settings::load_config(&new).vault_path.as_deref(), Some("/somewhere/else"));

    // Once there is a config of its own, the old one is left alone.
    settings::update_config(&new, |c| c.vault_path = Some("/chosen/later".into())).unwrap();
    assert!(!settings::adopt_old_config(&new, settings::OLD_IDENTIFIER));
    assert_eq!(settings::load_config(&new).vault_path.as_deref(), Some("/chosen/later"));

    // Nothing to bring over: nothing happens.
    assert!(!settings::adopt_old_config(&base.path().join("fresh"), "no.such.identifier"));
}
