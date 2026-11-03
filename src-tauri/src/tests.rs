//! Behavioural tests for the stores, against an in-memory database: the rules each one promises.

use std::fs;

use rusqlite::Connection;
use tempfile::tempdir;

use crate::error::Error;
use crate::models::{Bump, Criterion, StoryDraft, StoryUpdate, TaskUpdate};
use crate::posts::AddOptions;
use crate::{activity, board, db, deliverables, feedback, markdown, posts, projects, prompts, search, stories, templates, threads, uploads};

fn conn() -> Connection {
    db::open_in_memory().unwrap()
}

fn message(e: Error) -> String {
    e.to_string()
}

/// A project and one thread in it.
fn setup(c: &Connection) -> (String, String) {
    let project = projects::create(c, "Acme").unwrap().slug;
    let thread = threads::create(c, &project, "Calls").unwrap().slug;
    (project, thread)
}

fn reply(to: &str) -> AddOptions {
    AddOptions { reply_to_id: Some(to.to_string()), ..Default::default() }
}

fn story(id: &str, summary: &str) -> StoryDraft {
    StoryDraft { id: id.into(), summary: summary.into(), ..Default::default() }
}

// ---- projects ---------------------------------------------------------------------------------------

#[test]
fn a_project_starts_with_a_reading_guide_and_a_board() {
    let c = conn();
    let created = projects::create(&c, "  Launch plan ").unwrap();
    assert_eq!((created.slug.as_str(), created.name.as_str()), ("launch-plan", "Launch plan"));
    assert_eq!(projects::create(&c, "Launch plan").unwrap().slug, "launch-plan-2");
    assert_eq!(message(projects::create(&c, "  ").unwrap_err()), "Name is required");

    let view = projects::get(&c, "launch-plan").unwrap();
    assert!(view.readme.starts_with("# Launch plan"), "{}", view.readme);
    assert!(view.readme_html.contains("<h1>Launch plan</h1>"));

    let columns: Vec<_> = board::columns(&c, "launch-plan").unwrap().into_iter().map(|k| k.name).collect();
    assert_eq!(columns, ["Backlog", "Doing", "Done"]);

    let saved = projects::save_readme(&c, "launch-plan", "# Read me first\n\nHere is how it fits.").unwrap();
    assert_eq!(saved.readme, "# Read me first\n\nHere is how it fits.");
    assert_eq!(projects::rename(&c, "launch-plan", "Launch").unwrap().name, "Launch");
    assert_eq!(message(projects::get(&c, "nope").unwrap_err()), "Project not found");
}

#[test]
fn deleting_a_project_deletes_everything_in_it_and_nothing_elsewhere() {
    let c = conn();
    let (p, t) = setup(&c);
    let (other, other_thread) = {
        let o = projects::create(&c, "Other").unwrap().slug;
        let ot = threads::create(&c, &o, "Calls").unwrap().slug;
        (o, ot)
    };
    posts::add(&c, &p, &t, "a note", AddOptions::default()).unwrap();
    posts::add(&c, &other, &other_thread, "a note elsewhere", AddOptions::default()).unwrap();
    stories::create(&c, &p, story("S1", "story")).unwrap();
    board::create_task(&c, &p, "task", "", None, None).unwrap();
    deliverables::create(&c, &p, "Plan", "").unwrap();
    templates::create(&c, &p, "Mine", "", &[], "").unwrap();
    templates::create(&c, "", "Shared", "", &[], "").unwrap();

    projects::delete(&c, &p).unwrap();

    assert!(!projects::exists(&c, &p).unwrap());
    assert_eq!(posts::list(&c, &p, &t).unwrap().len(), 0);
    assert!(deliverables::list(&c, &p).unwrap().is_empty());
    assert!(templates::list(&c, &p).unwrap().is_empty());
    assert_eq!(templates::list(&c, "").unwrap().len(), 1, "vault-wide templates stay");
    assert_eq!(posts::list(&c, &other, &other_thread).unwrap().len(), 1);
}

// ---- threads and posts ------------------------------------------------------------------------------

#[test]
fn thread_slugs_are_unique_within_a_project_only() {
    let c = conn();
    let (a, _) = setup(&c);
    let b = projects::create(&c, "Beta").unwrap().slug;

    assert_eq!(threads::create(&c, &a, "Calls").unwrap().slug, "calls-2");
    assert_eq!(threads::create(&c, &b, "Calls").unwrap().slug, "calls");
    assert_eq!(threads::create(&c, &a, "!!!").unwrap().slug, "thread");
    assert_eq!(message(threads::create(&c, &a, "  ").unwrap_err()), "Title is required");
    assert_eq!(message(threads::create(&c, "ghost", "x").unwrap_err()), "Project not found");
}

#[test]
fn posts_keep_insertion_order_and_replies_must_exist_in_the_same_thread() {
    let c = conn();
    let (p, t) = setup(&c);
    let other = threads::create(&c, &p, "Other").unwrap().slug;

    let first = posts::add(&c, &p, &t, "  first  ", AddOptions::default()).unwrap();
    assert_eq!(first.body, "first");
    let second = posts::add(&c, &p, &t, "second", reply(&first.id)).unwrap();
    assert_eq!(second.parent_id.as_deref(), Some(first.id.as_str()));

    let order: Vec<_> = posts::list(&c, &p, &t).unwrap().into_iter().map(|x| x.body).collect();
    assert_eq!(order, ["first", "second"]);

    assert_eq!(message(posts::add(&c, &p, &other, "x", reply(&first.id)).unwrap_err()), "Post being replied to was not found");
    assert_eq!(message(posts::add(&c, &p, &t, "   ", AddOptions::default()).unwrap_err()), "Post body is required");
    assert_eq!(message(posts::add(&c, &p, "nope", "x", AddOptions::default()).unwrap_err()), "Thread not found");
}

#[test]
fn deleting_a_post_keeps_its_replies_and_editing_moves_the_timestamp() {
    let c = conn();
    let (p, t) = setup(&c);
    let parent = posts::add(&c, &p, &t, "parent", AddOptions::default()).unwrap();
    posts::add(&c, &p, &t, "child", reply(&parent.id)).unwrap();

    let edited = posts::update(&c, &p, &t, &parent.id, "parent, edited").unwrap();
    assert_eq!(edited.created_at, parent.created_at);
    assert!(edited.updated_at >= parent.updated_at);

    posts::delete(&c, &p, &t, &parent.id).unwrap();
    let left = posts::list(&c, &p, &t).unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].parent_id.as_deref(), Some(parent.id.as_str()));
    assert!(posts::update(&c, &p, &t, "missing", "x").is_err());
}

#[test]
fn thread_links_stay_inside_a_project_and_work_both_ways() {
    let c = conn();
    let (p, a) = setup(&c);
    let b = threads::create(&c, &p, "Beta").unwrap().slug;
    let elsewhere = projects::create(&c, "Other").unwrap().slug;
    let stranger = threads::create(&c, &elsewhere, "Stranger").unwrap().slug;

    threads::link(&c, &p, &b, &a).unwrap();
    threads::link(&c, &p, &a, &b).unwrap();
    assert_eq!(threads::linked_threads(&c, &p, &a).unwrap()[0].slug, b);
    assert_eq!(threads::linked_threads(&c, &p, &b).unwrap()[0].slug, a);
    assert!(threads::list(&c, &p).unwrap().iter().all(|t| t.linked_count == 1));

    assert_eq!(message(threads::link(&c, &p, &a, &a).unwrap_err()), "A thread can't link to itself");
    assert_eq!(message(threads::link(&c, &p, &a, &stranger).unwrap_err()), "Thread not found");
    threads::unlink(&c, &p, &b, &a).unwrap();
    assert!(threads::linked_threads(&c, &p, &a).unwrap().is_empty());
}

#[test]
fn a_thread_summary_counts_everything_in_it_and_orders_by_latest_activity() {
    let c = conn();
    let (p, quiet) = setup(&c);
    let busy = threads::create(&c, &p, "Busy").unwrap().slug;
    posts::add(&c, &p, &busy, "one", AddOptions::default()).unwrap();
    prompts::create(&c, &p, &busy, "", "write the thing", None).unwrap();

    let list = threads::list(&c, &p).unwrap();
    assert_eq!((list[0].slug.as_str(), list[0].post_count, list[0].prompt_count), (busy.as_str(), 1, 1));
    assert_eq!((list[1].slug.as_str(), list[1].post_count), (quiet.as_str(), 0));
}

// ---- templates and prompts --------------------------------------------------------------------------

#[test]
fn project_templates_replace_vault_wide_ones_with_the_same_id() {
    let c = conn();
    let (p, _) = setup(&c);
    templates::create(&c, "", "Interview", "after a call", &["Who?".into(), "  ".into(), "Why?".into()], "# Interview\n").unwrap();
    templates::create(&c, "", "Memo", "", &[], "").unwrap();

    let global = templates::get(&c, "", "interview").unwrap();
    assert_eq!(global.questions, ["Who?", "Why?"]);
    assert_eq!(global.trigger, "after a call");

    // The project defines its own "interview".
    templates::create(&c, &p, "Interview", "", &["Which account?".into()], "# Account interview\n").unwrap();
    let shown = templates::for_project(&c, &p).unwrap();
    let marks: Vec<_> = shown.iter().map(|t| (t.scope.as_str(), t.id.as_str(), t.overridden)).collect();
    assert_eq!(marks, [(p.as_str(), "interview", false), ("", "interview", true), ("", "memo", false)]);

    let usable: Vec<_> = templates::effective(&c, &p).unwrap().into_iter().map(|t| (t.scope, t.id)).collect();
    assert_eq!(usable, [(p.clone(), "interview".into()), ("".into(), "memo".into())]);
    assert_eq!(templates::find(&c, &p, "interview").unwrap().questions, ["Which account?"]);
    assert_eq!(templates::find(&c, &p, "memo").unwrap().scope, "");

    let updated = templates::update(&c, "", "interview", Some("Interviews"), None, Some(&["Last?".into(), "First?".into()]), None).unwrap();
    assert_eq!((updated.name.as_str(), updated.trigger.as_str()), ("Interviews", "after a call"));
    assert_eq!(updated.questions, ["Last?", "First?"]);

    templates::delete(&c, &p, "interview").unwrap();
    assert_eq!(templates::find(&c, &p, "interview").unwrap().scope, "", "the vault-wide one is back");
    assert_eq!(message(templates::create(&c, "ghost", "x", "", &[], "").unwrap_err()), "Project not found");
}

#[test]
fn a_typed_md_extension_is_not_part_of_a_deliverables_name() {
    let c = conn();
    let (p, _) = setup(&c);
    let typed = deliverables::create(&c, &p, "Roadmap.md", "").unwrap();
    assert_eq!((typed.path.as_str(), typed.body.as_deref()), ("roadmap.md", Some("# Roadmap\n")));
    assert_eq!(deliverables::create(&c, &p, "Notes.MD", "").unwrap().path, "notes.md");
    assert_eq!(message(deliverables::create(&c, &p, " .md ", "").unwrap_err()), "Name is required");
}

#[test]
fn a_project_can_copy_a_vault_wide_template_to_change_it_for_itself() {
    let c = conn();
    let (p, _) = setup(&c);
    templates::create(&c, "", "Interview", "after a call", &["Who?".into(), "Why?".into()], "# Interview\n").unwrap();

    let copy = templates::override_global(&c, &p, "interview").unwrap();
    assert_eq!((copy.scope.as_str(), copy.id.as_str(), copy.name.as_str()), (p.as_str(), "interview", "Interview"));
    assert_eq!((copy.trigger.as_str(), copy.body.as_str()), ("after a call", "# Interview"));
    assert_eq!(copy.questions, ["Who?", "Why?"]);
    assert!(templates::for_project(&c, &p).unwrap().iter().any(|t| t.scope.is_empty() && t.overridden));

    templates::update(&c, &p, "interview", None, None, Some(&["Which account?".into()]), None).unwrap();
    assert_eq!(templates::get(&c, "", "interview").unwrap().questions, ["Who?", "Why?"], "the shared one is untouched");

    assert_eq!(message(templates::override_global(&c, &p, "interview").unwrap_err()), "This project already has its own version of that template");
    assert_eq!(message(templates::override_global(&c, &p, "nope").unwrap_err()), "Template not found");
}

#[test]
fn a_prompt_from_a_template_seeds_its_questions_once_and_comes_after_them() {
    let c = conn();
    let (p, t) = setup(&c);
    templates::create(&c, "", "Interview", "", &["Who?".into(), "Why?".into()], "# Interview\n").unwrap();

    let prompt = prompts::create(&c, &p, &t, "Summary of the call", "  Write up the call with Ana.  ", Some("interview")).unwrap();
    assert_eq!((prompt.body.as_str(), prompt.template_name.as_deref()), ("Write up the call with Ana.", Some("Interview")));

    let seeded = posts::list(&c, &p, &t).unwrap();
    assert_eq!(seeded.len(), 2);
    assert!(seeded.iter().all(|x| x.guiding_question));
    assert_eq!(seeded[0].body, "**Guiding question (Interview):** Who?");
    assert!(seeded[1].created_at <= prompt.created_at, "the prompt comes after the questions it prompted");

    // Editing the template afterwards never reaches back into the thread.
    templates::update(&c, "", "interview", None, None, Some(&["Something else?".into()]), None).unwrap();
    assert_eq!(posts::list(&c, &p, &t).unwrap().len(), 2);

    assert_eq!(message(prompts::create(&c, &p, &t, "", "   ", None).unwrap_err()), "Say what you want delivered");
    assert_eq!(message(prompts::create(&c, &p, &t, "", "x", Some("nope")).unwrap_err()), "Template not found");
    assert_eq!(posts::list(&c, &p, &t).unwrap().len(), 2, "a refused prompt seeds nothing");

    let renamed = prompts::update(&c, &p, &t, &prompt.id, Some("Call summary"), Some("Write it shorter.")).unwrap();
    assert_eq!((renamed.title.as_str(), renamed.body.as_str()), ("Call summary", "Write it shorter."));
    prompts::delete(&c, &p, &t, &prompt.id).unwrap();
    assert!(prompts::list(&c, &p, &t).unwrap().is_empty());
}

#[test]
fn a_prompt_can_be_made_for_an_existing_deliverable_or_for_a_new_one() {
    let c = conn();
    let (p, t) = setup(&c);
    deliverables::create(&c, &p, "Pricing brief", "# Pricing brief\n\nDraft.\n").unwrap();

    // For an existing deliverable: it is linked, the way "Link existing" would.
    let revise = prompts::create_linked(&c, &p, &t, "Revise it", "Tighten the pricing brief.", None, Some("pricing-brief.md")).unwrap();
    assert_eq!(revise.deliverables.iter().map(|d| (d.path.as_str(), d.exists)).collect::<Vec<_>>(), [("pricing-brief.md", true)]);
    assert_eq!(prompts::get(&c, &p, &t, &revise.id).unwrap().deliverables.len(), 1);

    // For a new one: nothing is linked until something is produced. A blank choice is the same as none.
    for none in [None, Some(""), Some("  ")] {
        let fresh = prompts::create_linked(&c, &p, &t, "Another", "Write a new brief.", None, none).unwrap();
        assert!(fresh.deliverables.is_empty(), "{none:?}");
    }

    // One that isn't there is refused before anything is written.
    let before = prompts::list(&c, &p, &t).unwrap().len();
    assert_eq!(message(prompts::create_linked(&c, &p, &t, "", "x", None, Some("nope.md")).unwrap_err()), "Deliverable not found");
    assert_eq!(prompts::list(&c, &p, &t).unwrap().len(), before, "no prompt is left half made");
    assert_eq!(message(prompts::create_linked(&c, &p, &t, "", "   ", None, Some("pricing-brief.md")).unwrap_err()), "Say what you want delivered");
}

// ---- deliverables -----------------------------------------------------------------------------------

#[test]
fn version_tags_are_split_and_rebuilt() {
    assert_eq!(deliverables::split_version("plan@v1.2.md"), ("plan.md".to_string(), Some("1.2".to_string())));
    assert_eq!(deliverables::split_version("reports/q1.final@v0.10.pdf"), ("reports/q1.final.pdf".to_string(), Some("0.10".to_string())));
    assert_eq!(deliverables::split_version("LICENSE@v2.0"), ("LICENSE".to_string(), Some("2.0".to_string())));
    for plain in ["plan.md", "plan@vx.md", "plan@v1.md", "@v1.0.md", "a@b.md", "plan@v1.0.2.md"] {
        assert_eq!(deliverables::split_version(plain), (plain.to_string(), None), "{plain}");
    }
    assert_eq!(deliverables::snapshot_path("reports/q1.md", "1.0"), "reports/q1@v1.0.md");
    assert_eq!(deliverables::snapshot_path("LICENSE", "1.0"), "LICENSE@v1.0");
    for path in ["plan.md", "a/b/c.tar.gz", "noext"] {
        assert_eq!(deliverables::split_version(&deliverables::snapshot_path(path, "3.4")), (path.to_string(), Some("3.4".to_string())));
    }

    assert_eq!(deliverables::bump_version(None, Bump::Minor), "0.1");
    assert_eq!(deliverables::bump_version(None, Bump::Major), "1.0");
    assert_eq!(deliverables::bump_version(Some("0.9"), Bump::Minor), "0.10");
    assert_eq!(deliverables::bump_version(Some("1.4"), Bump::Major), "2.0");
    for bad in ["1", "1.", ".1", "1.2.3", "a.b", "1.-2", " 1.2", ""] {
        assert_eq!(deliverables::parse_version(bad), None, "{bad:?}");
    }
    assert_eq!(deliverables::kind_of("a.MD"), "markdown");
    assert_eq!(deliverables::kind_of("chart.PNG"), "image");
    assert_eq!(deliverables::kind_of("data.xlsx"), "other");
}

#[test]
fn a_prompts_deliverable_is_edited_published_and_announced_in_its_thread() {
    let c = conn();
    let dir = tempdir().unwrap();
    let (p, t) = setup(&c);
    let prompt = prompts::create(&c, &p, &t, "", "write the plan", None).unwrap();

    // The deliverable is a file; the prompt keeps the link.
    let made = deliverables::create(&c, &p, "Launch Plan", "").unwrap();
    assert_eq!((made.path.as_str(), made.kind.as_str()), ("launch-plan.md", "markdown"));
    assert_eq!(made.body.as_deref(), Some("# Launch Plan\n"));
    assert_eq!(deliverables::create(&c, &p, "Launch Plan", "").unwrap().path, "launch-plan-2.md");
    prompts::link_deliverable(&c, &p, &t, &prompt.id, "launch-plan.md").unwrap();
    assert_eq!(message(prompts::link_deliverable(&c, &p, &t, &prompt.id, "ghost.md").unwrap_err()), "Deliverable not found");

    deliverables::save(&c, &p, "launch-plan.md", "# Plan\n\nPhase one.").unwrap();
    let v1 = deliverables::publish(&c, dir.path(), &p, "launch-plan.md", Bump::Major, None).unwrap();
    assert_eq!(v1.versions, ["1.0"]);
    deliverables::save(&c, &p, "launch-plan.md", "# Plan\n\nPhase one and two.").unwrap();
    let v2 = deliverables::publish(&c, dir.path(), &p, "launch-plan.md", Bump::Minor, None).unwrap();
    assert_eq!(v2.versions, ["1.0", "1.1"]);

    // Published versions never change.
    let old = deliverables::get(&c, &p, "launch-plan@v1.0.md").unwrap();
    assert_eq!((old.version.as_deref(), old.editable), (Some("1.0"), false));
    assert_eq!(old.body.as_deref(), Some("# Plan\n\nPhase one."));
    assert_eq!(message(deliverables::save(&c, &p, "launch-plan@v1.0.md", "x").unwrap_err()), "Only the current version of a markdown deliverable can be edited");
    assert_eq!(message(deliverables::publish(&c, dir.path(), &p, "launch-plan.md", Bump::Minor, Some("1.0")).unwrap_err()), "Version 1.0 is already published");
    assert_eq!(message(deliverables::publish(&c, dir.path(), &p, "launch-plan.md", Bump::Minor, Some("v2")).unwrap_err()), "Version must be in M.m format, e.g. 1.4");
    assert_eq!(message(deliverables::publish(&c, dir.path(), &p, "launch-plan@v1.0.md", Bump::Minor, None).unwrap_err()), "Publish the current version, not a published one");

    // The thread of the prompt that produced it hears about each publish.
    let notes: Vec<_> = posts::list(&c, &p, &t).unwrap().into_iter().filter(|x| x.version_published).collect();
    assert_eq!(notes.len(), 2);
    assert_eq!(notes[0].body, "Version 1.0 of **launch-plan.md** was just published.");
    assert_eq!((notes[0].deliverable_path.as_deref(), notes[0].deliverable_version.as_deref()), (Some("launch-plan.md"), Some("1.0")));

    let listed = deliverables::list(&c, &p).unwrap();
    assert_eq!(listed.len(), 2, "versions are not listed as deliverables of their own");
    let plan = listed.iter().find(|d| d.path == "launch-plan.md").unwrap();
    assert_eq!(plan.versions, ["1.0", "1.1"]);
    assert_eq!(plan.prompt_count, 1);
    assert_eq!(deliverables::count_live(&c, &p).unwrap(), 2);
    assert_eq!(deliverables::get(&c, &p, "launch-plan.md").unwrap().prompts[0].id, prompt.id);
}

#[test]
fn files_that_are_not_markdown_are_copied_and_removed_on_disk() {
    let c = conn();
    let dir = tempdir().unwrap();
    let (p, _) = setup(&c);
    let folder = deliverables::deliverables_dir(dir.path(), &p);
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("chart.png"), [1u8, 2, 3]).unwrap();
    deliverables::upsert(&c, &p, "chart.png", 3, "2026-01-01T00:00:00.000Z", None).unwrap();

    let view = deliverables::get(&c, &p, "chart.png").unwrap();
    assert_eq!((view.kind.as_str(), view.editable, view.body.is_none()), ("image", false, true));

    deliverables::publish(&c, dir.path(), &p, "chart.png", Bump::Minor, None).unwrap();
    assert_eq!(fs::read(folder.join("chart@v0.1.png")).unwrap(), [1, 2, 3]);

    // Deleting the current file removes it from disk; the published copy stays and still lists the deliverable.
    deliverables::delete(&c, dir.path(), &p, "chart.png").unwrap();
    assert!(!folder.join("chart.png").exists() && folder.join("chart@v0.1.png").exists());
    let listed = deliverables::list(&c, &p).unwrap();
    assert_eq!((listed[0].path.as_str(), listed[0].exists, listed[0].versions.clone()), ("chart.png", false, vec!["0.1".to_string()]));
    assert_eq!(message(deliverables::delete(&c, dir.path(), &p, "chart@v0.1.png").unwrap_err()), "Published versions are kept");
}

#[test]
fn deliverables_can_be_started_from_a_prompts_template() {
    let c = conn();
    let (p, t) = setup(&c);
    templates::create(&c, "", "Brief", "", &[], "# Brief\n\n## Goal\n").unwrap();
    let prompt = prompts::create(&c, &p, &t, "", "write the brief", Some("brief")).unwrap();

    let skeleton = templates::find(&c, &p, prompt.template_id.as_deref().unwrap()).unwrap().body;
    let made = deliverables::create(&c, &p, "Brief", &skeleton).unwrap();
    let linked = prompts::link_deliverable(&c, &p, &t, &prompt.id, &made.path).unwrap();
    assert_eq!(made.body.as_deref(), Some("# Brief\n\n## Goal\n"));
    assert_eq!(linked.deliverables.len(), 1);
    assert!(linked.deliverables[0].exists);

    prompts::unlink_deliverable(&c, &p, &t, &prompt.id, &made.path).unwrap();
    assert!(prompts::get(&c, &p, &t, &prompt.id).unwrap().deliverables.is_empty());
}

// ---- feedback ---------------------------------------------------------------------------------------

#[test]
fn feedback_lands_on_a_deliverable_or_one_of_its_versions() {
    let c = conn();
    let dir = tempdir().unwrap();
    let (p, t) = setup(&c);
    let other_thread = threads::create(&c, &p, "Review").unwrap().slug;
    deliverables::create(&c, &p, "Plan", "").unwrap();
    deliverables::publish(&c, dir.path(), &p, "plan.md", Bump::Major, None).unwrap();

    let live = feedback::add(&c, &p, &t, "plan.md", "  Needs a timeline.  ").unwrap();
    assert_eq!((live.body.as_str(), live.status.as_str(), live.deliverable_version.clone()), ("Needs a timeline.", "open", None));
    let on_version = feedback::add(&c, &p, &other_thread, "plan@v1.0.md", "The v1 summary is vague.").unwrap();
    assert_eq!(on_version.deliverable_version.as_deref(), Some("1.0"));

    assert_eq!(message(feedback::add(&c, &p, &t, "ghost.md", "x").unwrap_err()), "Deliverable not found");
    assert_eq!(message(feedback::add(&c, &p, &t, "plan.md", "  ").unwrap_err()), "Write the feedback first");

    // Both pieces of feedback belong to the deliverable, whichever thread they were written in.
    let all = feedback::list_for_deliverable(&c, &p, "plan.md").unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(deliverables::list(&c, &p).unwrap()[0].open_feedback, 2);

    let resolved = feedback::set_status(&c, &p, &t, &live.id, "resolved").unwrap();
    assert_eq!(resolved.status, "resolved");
    assert_eq!(deliverables::list(&c, &p).unwrap()[0].open_feedback, 1);
    assert_eq!(message(feedback::set_status(&c, &p, &t, &live.id, "done").unwrap_err()), "Feedback is either open or resolved");
    assert_eq!(feedback::update(&c, &p, &t, &live.id, "Needs a timeline and owners.").unwrap().body, "Needs a timeline and owners.");
    feedback::delete(&c, &p, &t, &live.id).unwrap();
    assert!(feedback::list_for_thread(&c, &p, &t).unwrap().is_empty());
}

// ---- uploads ----------------------------------------------------------------------------------------

#[test]
fn an_upload_is_a_file_in_attachments_and_a_manifest_in_the_thread() {
    let c = conn();
    let dir = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let (p, t) = setup(&c);
    let source = outside.path().join("Site plan.png");
    fs::write(&source, [9u8; 10]).unwrap();

    let added = uploads::add_files(&c, dir.path(), &p, &t, std::slice::from_ref(&source), "  The site plan  ").unwrap();
    let upload = &added[0];
    assert_eq!((upload.file.as_str(), upload.name.as_str(), upload.size, upload.image, upload.exists), ("attachments/Site plan.png", "Site plan.png", 10, true, true));
    assert_eq!(upload.description, "The site plan");
    assert_eq!(fs::read(uploads::project_dir(dir.path(), &p).join(&upload.file)).unwrap(), [9u8; 10]);

    // A name that is taken gets a number instead of overwriting.
    let again = uploads::add_files(&c, dir.path(), &p, &t, &[source], "").unwrap();
    assert_eq!(again[0].file, "attachments/Site plan-2.png");

    // Screenshots arrive as bytes, and names are made safe on every platform.
    let shot = uploads::add_bytes(&c, dir.path(), &p, &t, "a:b?c.png", b"png", "").unwrap();
    assert_eq!(shot.file, "attachments/a_b_c.png");

    assert!(message(uploads::add_files(&c, dir.path(), &p, &t, &[outside.path().to_path_buf()], "").unwrap_err()).contains("isn't a file"));
    assert_eq!(message(uploads::add_bytes(&c, dir.path(), &p, "ghost", "x.png", b"", "").unwrap_err()), "Thread not found");

    uploads::set_description(&c, dir.path(), &p, &t, &upload.id, "Revised").unwrap();
    assert_eq!(uploads::list(&c, dir.path(), &p, &t).unwrap()[0].description, "Revised");

    // Deleting the manifest takes the file with it, unless another manifest still points at it.
    let twin = uploads::add_bytes(&c, dir.path(), &p, &t, "twin.png", b"x", "").unwrap();
    c.execute(
        "INSERT INTO uploads (id, project_slug, thread_slug, file, name, size, uploaded_at) VALUES ('2', ?1, ?2, ?3, 'twin.png', 1, '2026')",
        [&p, &t, &twin.file],
    )
    .unwrap();
    let file = uploads::project_dir(dir.path(), &p).join(&twin.file);
    uploads::delete(&c, dir.path(), &p, &t, &twin.id).unwrap();
    assert!(file.exists(), "still referenced");
    uploads::delete(&c, dir.path(), &p, &t, "2").unwrap();
    assert!(!file.exists());

    // Deleting a thread removes the attachments only it used.
    let kept = uploads::project_dir(dir.path(), &p).join(&shot.file);
    threads::delete(&c, dir.path(), &p, &t).unwrap();
    assert!(!kept.exists());
}

// ---- stories and the board --------------------------------------------------------------------------

#[test]
fn story_ids_are_validated_and_unique_per_project_ignoring_case() {
    let c = conn();
    let (p, _) = setup(&c);
    let other = projects::create(&c, "Other").unwrap().slug;

    assert_eq!(message(stories::create(&c, &p, story("", "x")).unwrap_err()), "ID is required");
    for bad in ["-MM01", "MM 01", "MM.01", &"x".repeat(41)] {
        assert!(stories::create(&c, &p, story(bad, "x")).is_err(), "{bad:?}");
    }
    assert_eq!(message(stories::create(&c, &p, story("MM01", "  ")).unwrap_err()), "Summary is required");

    stories::create(&c, &p, story("MM01", "Do the thing")).unwrap();
    assert_eq!(message(stories::create(&c, &p, story("mm01", "Again")).unwrap_err()), "A user story with ID mm01 already exists");
    stories::create(&c, &other, story("MM01", "Same id, other project")).unwrap();
    assert_eq!(stories::get(&c, &p, "mm01").unwrap().id, "MM01");
    assert_eq!(stories::list(&c, &other).unwrap().len(), 1);
}

#[test]
fn stories_keep_criteria_in_order_link_to_threads_and_survive_thread_deletion() {
    let c = conn();
    let dir = tempdir().unwrap();
    let (p, a) = setup(&c);
    let b = threads::create(&c, &p, "B").unwrap().slug;

    let draft = StoryDraft {
        criteria: vec![
            Criterion { text: " First ".into(), done: true },
            Criterion { text: "   ".into(), done: false },
            Criterion { text: "Second".into(), done: false },
        ],
        thread_slugs: vec![a.clone(), b.clone()],
        notes: " Context ".into(),
        ..story("S1", "With criteria")
    };
    let created = stories::create(&c, &p, draft).unwrap();
    let texts: Vec<_> = created.criteria.iter().map(|k| (k.text.as_str(), k.done)).collect();
    assert_eq!(texts, [("First", true), ("Second", false)]);
    assert_eq!((created.notes.as_str(), created.threads.len()), ("Context", 2));
    assert_eq!(message(stories::create(&c, &p, StoryDraft { thread_slugs: vec!["ghost".into()], ..story("S2", "x") }).unwrap_err()), "Thread not found");

    let updated = stories::update(
        &c,
        &p,
        "s1",
        StoryUpdate { criteria: Some(vec![Criterion { text: "Only".into(), done: true }]), summary: Some("Renamed".into()), ..Default::default() },
    )
    .unwrap();
    assert_eq!((updated.summary.as_str(), updated.criteria.len(), updated.want.as_str()), ("Renamed", 1, ""));

    stories::remove_thread(&c, &p, "S1", &a).unwrap();
    assert_eq!(stories::list_for_thread(&c, &p, &a).unwrap().len(), 0);
    stories::add_thread(&c, &p, "S1", &a).unwrap();
    stories::add_thread(&c, &p, "S1", &a).unwrap();
    assert_eq!(stories::get(&c, &p, "S1").unwrap().threads.len(), 2);

    threads::delete(&c, dir.path(), &p, &b).unwrap();
    assert_eq!(stories::get(&c, &p, "S1").unwrap().threads.len(), 1);
}

#[test]
fn tasks_land_in_the_backlog_move_between_columns_and_link_to_stories_and_threads() {
    let c = conn();
    let dir = tempdir().unwrap();
    let (p, t) = setup(&c);
    stories::create(&c, &p, story("MM01", "Pricing")).unwrap();

    let task = board::create_task(&c, &p, "  Draw the pricing page ", " in figma ", Some("mm01"), Some(&t)).unwrap();
    assert_eq!((task.title.as_str(), task.body.as_str(), task.column_id.as_str()), ("Draw the pricing page", "in figma", "backlog"));
    assert_eq!((task.story_id.as_deref(), task.story_summary.as_deref(), task.thread_title.as_deref()), (Some("MM01"), Some("Pricing"), Some("Calls")));
    assert_eq!(stories::get(&c, &p, "MM01").unwrap().task_count, 1);

    let moved = board::update_task(&c, &p, &task.id, TaskUpdate { column_id: Some("doing".into()), ..Default::default() }).unwrap();
    assert_eq!(moved.column_id, "doing");
    assert_eq!(message(board::update_task(&c, &p, &task.id, TaskUpdate { column_id: Some("nowhere".into()), ..Default::default() }).unwrap_err()), "Column not found");
    assert_eq!(message(board::create_task(&c, &p, " ", "", None, None).unwrap_err()), "Title is required");
    assert_eq!(message(board::create_task(&c, &p, "x", "", Some("ghost"), None).unwrap_err()), "User story not found");

    // An empty string clears a link.
    let cleared = board::update_task(&c, &p, &task.id, TaskUpdate { story_id: Some(String::new()), ..Default::default() }).unwrap();
    assert!(cleared.story_id.is_none() && cleared.thread_slug.is_some());

    // Deleting what a task points at only clears the link.
    board::update_task(&c, &p, &task.id, TaskUpdate { story_id: Some("MM01".into()), ..Default::default() }).unwrap();
    stories::delete(&c, &p, "MM01").unwrap();
    threads::delete(&c, dir.path(), &p, &t).unwrap();
    let left = board::task(&c, &p, &task.id).unwrap();
    assert!(left.story_id.is_none() && left.thread_slug.is_none());

    assert_eq!(board::open_task_count(&c, &p).unwrap(), 1);
    board::update_task(&c, &p, &task.id, TaskUpdate { column_id: Some("done".into()), ..Default::default() }).unwrap();
    assert_eq!(board::open_task_count(&c, &p).unwrap(), 0);
}

#[test]
fn columns_are_per_project_and_guarded() {
    let c = conn();
    let (p, _) = setup(&c);
    let other = projects::create(&c, "Other").unwrap().slug;
    let task = board::create_task(&c, &p, "x", "", None, None).unwrap();

    assert_eq!(message(board::delete_column(&c, &p, "backlog").unwrap_err()), "Move or delete this column's tasks first");
    let columns = board::add_column(&c, &p, "  Review ").unwrap();
    let review = columns.last().unwrap().clone();
    assert_eq!(review.name, "Review");
    assert_eq!(board::columns(&c, &other).unwrap().len(), 3, "another project's board is untouched");

    board::rename_column(&c, &p, &review.id, "Reviewed").unwrap();
    board::delete_column(&c, &p, &review.id).unwrap();
    assert_eq!(board::columns(&c, &p).unwrap().len(), 3);

    // Once the backlog is gone, new tasks fall back to the first column left.
    board::update_task(&c, &p, &task.id, TaskUpdate { column_id: Some("doing".into()), ..Default::default() }).unwrap();
    board::delete_column(&c, &p, "backlog").unwrap();
    assert_eq!(board::create_task(&c, &p, "y", "", None, None).unwrap().column_id, "doing");

    board::delete_column(&c, &p, "done").unwrap();
    assert_eq!(message(board::delete_column(&c, &p, "doing").unwrap_err()), "Move or delete this column's tasks first");

    // A board always keeps one column.
    board::delete_column(&c, &other, "backlog").unwrap();
    board::delete_column(&c, &other, "doing").unwrap();
    assert_eq!(message(board::delete_column(&c, &other, "done").unwrap_err()), "The board needs at least one column");
}

// ---- activity ---------------------------------------------------------------------------------------

#[test]
fn activity_counts_what_you_made_and_not_what_the_app_wrote_for_you() {
    let c = conn();
    let dir = tempdir().unwrap();
    let (p, t) = setup(&c);
    let other = projects::create(&c, "Other").unwrap().slug;

    let first = posts::add(&c, &p, &t, "A thought", AddOptions::default()).unwrap();
    posts::add(&c, &p, &t, "A reply", reply(&first.id)).unwrap();
    posts::add(&c, &p, &t, "What is it for?", AddOptions { guiding_question: true, ..Default::default() }).unwrap();
    prompts::create(&c, &p, &t, "Memo", "Write it", None).unwrap();
    let memo = deliverables::create(&c, &p, "Memo", "Draft").unwrap();
    deliverables::publish(&c, dir.path(), &p, &memo.path, Bump::Major, None).unwrap(); // also posts a "version published" note
    feedback::add(&c, &p, &t, &memo.path, "Needs numbers").unwrap();
    stories::create(&c, &p, story("MM01", "Sign in")).unwrap();
    board::create_task(&c, &other, "Elsewhere", "", None, None).unwrap();

    let events = activity::list(&c).unwrap();
    let count = |project: &str, kind: &str| events.iter().filter(|e| e.project_slug == project && e.kind == kind).count();
    assert_eq!(count(&p, "thread"), 1);
    assert_eq!(count(&p, "post"), 2, "guiding questions and the publish note are not yours");
    assert_eq!(count(&p, "prompt"), 1);
    assert_eq!(count(&p, "version"), 1);
    assert_eq!(count(&p, "feedback"), 1);
    assert_eq!(count(&p, "story"), 1);
    assert_eq!(count(&other, "task"), 1);
    assert!(events.windows(2).all(|w| w[0].at <= w[1].at), "oldest first");
}

// ---- search and markdown ----------------------------------------------------------------------------

#[test]
fn search_spans_posts_prompts_feedback_and_deliverables_across_projects() {
    let c = conn();
    let dir = tempdir().unwrap();
    let (p, t) = setup(&c);
    let other = projects::create(&c, "Other").unwrap().slug;
    let other_thread = threads::create(&c, &other, "Elsewhere").unwrap().slug;

    posts::add(&c, &p, &t, "We should revisit the PRICING model next week", AddOptions::default()).unwrap();
    posts::add(&c, &other, &other_thread, "Pricing is different here", AddOptions::default()).unwrap();
    posts::add(&c, &p, &t, "Unrelated", AddOptions::default()).unwrap();
    prompts::create(&c, &p, &t, "Pricing memo", "Write the memo", None).unwrap();
    let memo = deliverables::create(&c, &p, "Memo", "Details about pricing tiers").unwrap();
    deliverables::publish(&c, dir.path(), &p, &memo.path, Bump::Major, None).unwrap();
    feedback::add(&c, &p, &t, &memo.path, "pricing tiers need numbers").unwrap();

    let hits = search::search(&c, "  pricing ").unwrap();
    let kinds: Vec<_> = hits.iter().map(|h| h.kind.as_str()).collect();
    assert_eq!(hits.len(), 5, "{kinds:?}"); // 2 posts, a prompt, feedback, and the live deliverable (not its frozen copy)
    for kind in ["post", "prompt", "feedback", "deliverable"] {
        assert!(kinds.contains(&kind), "{kind} in {kinds:?}");
    }
    assert!(hits.iter().any(|h| h.project_slug == other));
    assert!(hits.iter().filter(|h| h.kind == "deliverable").all(|h| h.id == "memo.md"));
    assert!(search::search(&c, "   ").unwrap().is_empty());
    assert!(search::search(&c, "zzz").unwrap().is_empty());
}

#[test]
fn snippets_are_cut_on_character_boundaries() {
    let long = format!("{}needle{}", "é".repeat(100), "ü".repeat(100));
    let s = search::snippet(&long, "NEEDLE").unwrap();
    assert!(s.starts_with('…') && s.ends_with('…'));
    assert!(s.contains("needle"));
    assert_eq!(search::snippet("short needle here", "needle").unwrap(), "short needle here");
    assert_eq!(search::snippet("İstanbul", "stanbul").unwrap(), "İstanbul");
    assert!(search::snippet("abc", "").is_none());
}

#[test]
fn markdown_escapes_raw_html_and_strips_unsafe_links() {
    let html = markdown::render("<script>alert(1)</script> and <b>bold</b>");
    assert!(!html.contains("<script>") && !html.contains("<b>"));
    assert!(html.contains("&lt;script&gt;"));

    let html = markdown::render("[x](javascript:alert(1)) [y](JaVa\tScRiPt:alert(1)) ![z](data:text/html;base64,AAAA)");
    assert!(!html.to_lowercase().contains("javascript"), "{html}");
    assert!(!html.contains("data:"), "{html}");

    let html = markdown::render("[ok](https://example.com) [mail](mailto:a@b.c) [rel](#top) [page](notes/page)");
    for target in ["https://example.com", "mailto:a@b.c", "#top", "notes/page"] {
        assert!(html.contains(&format!("href=\"{target}\"")), "{html}");
    }

    let html = markdown::render("**strong**\n\n- [x] done\n- [ ] todo\n\n| a | b |\n|---|---|\n| 1 | 2 |");
    assert!(html.contains("<strong>strong</strong>") && html.contains("<table>") && html.contains("checkbox"));
}
