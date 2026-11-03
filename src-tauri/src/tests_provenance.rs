//! How a published version came to be: who made it, where it is filed, its AI-use label, and what the side window
//! of a deliverable shows of that.

use rusqlite::Connection;
use tempfile::tempdir;

use crate::claude_code::records::{self, RunRecord};
use crate::error::Error;
use crate::models::{Bump, PublishChoices, VersionUpdate};
use crate::provenance::{self, Entry};
use crate::{db, deliverables, feedback, ids, posts, projects, prompts, threads};

fn message(e: Error) -> String {
    e.to_string()
}

/// A project with two threads, a deliverable "plan.md" that the prompt in the first thread asked for, and nothing
/// published yet. Returns the connection, the project, the two thread slugs and the prompt's id.
fn setup() -> (Connection, String, String, String, String) {
    let c = db::open_in_memory().unwrap();
    let p = projects::create(&c, "Acme").unwrap().slug;
    let t1 = threads::create(&c, &p, "Kick-off").unwrap().slug;
    let t2 = threads::create(&c, &p, "Review").unwrap().slug;
    let prompt = prompts::create(&c, &p, &t1, "Write the plan", "Write the plan for the launch.", None).unwrap();
    deliverables::create(&c, &p, "Plan", "# Plan\n\nfirst\n").unwrap();
    prompts::link_deliverable(&c, &p, &t1, &prompt.id, "plan.md").unwrap();
    (c, p, t1, t2, prompt.id)
}

fn choose(f: impl FnOnce(&mut PublishChoices)) -> PublishChoices {
    let mut choices = PublishChoices::default();
    f(&mut choices);
    choices
}

fn publish(c: &Connection, p: &str, text: &str, choices: &PublishChoices) -> String {
    deliverables::save(c, p, "plan.md", text).unwrap();
    let root = tempdir().unwrap();
    deliverables::publish_with(c, root.path(), p, "plan.md", Bump::Minor, None, choices).unwrap().versions.last().cloned().unwrap()
}

fn notes(c: &Connection, p: &str, thread: &str) -> Vec<crate::models::PostView> {
    posts::list(c, p, thread).unwrap().into_iter().filter(|x| x.version_published).collect()
}

/// A finished or stopped run of the prompt that listed `files`, as the run's note records it.
fn run(c: &Connection, p: &str, thread: &str, prompt: &str, id: &str, status: &str, files: &[&str]) {
    let at = ids::now();
    records::insert(
        c,
        &RunRecord {
            id: id.into(),
            project: p.into(),
            thread: thread.into(),
            prompt_id: prompt.into(),
            status: status.into(),
            model: "claude-test".into(),
            permission_mode: "acceptEdits".into(),
            started_at: at.clone(),
            finished_at: at,
            seconds: 12,
            turns: Some(3),
            files: files.iter().map(|f| f.to_string()).collect(),
            body: "Done.".into(),
        },
    )
    .unwrap();
}

fn version<'a>(prov: &'a provenance::Provenance, v: &str) -> &'a provenance::VersionEntry {
    prov.groups
        .iter()
        .flat_map(|g| &g.entries)
        .find_map(|e| match e {
            Entry::Version(x) if x.version == v => Some(x),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no version {v}"))
}

// ---- publishing by hand --------------------------------------------------------------------------------

#[test]
fn a_version_typed_by_a_person_is_recorded_as_theirs_and_noted_in_the_thread_that_asked_for_it() {
    let (c, p, t1, _, _) = setup();
    let v = publish(&c, &p, "# Plan\n\nsecond\n", &PublishChoices::default());
    assert_eq!(v, "0.1");

    let known = provenance::resolve(&c, &p, "plan.md").unwrap();
    assert_eq!(known.len(), 1);
    assert_eq!((known[0].author, known[0].thread.as_deref(), known[0].prompt.as_deref(), known[0].run.as_deref()), ("human", Some(t1.as_str()), None, None));
    assert_eq!(known[0].sha256.as_deref(), Some(provenance::sha256_hex(b"# Plan\n\nsecond\n").as_str()));
    assert!(known[0].record.is_some());

    // The note reads as before and points at no prompt: nothing but the person caused this version.
    let note = &notes(&c, &p, &t1)[0];
    assert_eq!(note.body, "Version 0.1 of **plan.md** was just published.");
    assert_eq!((note.prompt_id.as_deref(), note.prompt_title.as_deref()), (None, None));
    assert_eq!(known[0].post, Some((t1.clone(), note.id.clone())));
}

#[test]
fn ticking_that_ai_made_the_change_records_it_with_its_label_prompt_and_note() {
    let (c, p, t1, t2, prompt) = setup();
    let v = publish(
        &c,
        &p,
        "# Plan\n\nfrom a chat\n",
        &choose(|x| {
            x.thread = Some(t2.clone());
            x.ai = true;
            x.mmm = Some("cyborg".into());
            x.prompt = Some(prompt.clone());
            x.note = Some("  Pasted from a chat, then rewritten. ".into());
        }),
    );

    let known = &provenance::resolve(&c, &p, "plan.md").unwrap()[0];
    assert_eq!((known.version.as_str(), known.author, known.thread.as_deref(), known.prompt.as_deref(), known.run.as_deref()), (v.as_str(), "ai", Some(t2.as_str()), Some(prompt.as_str()), None));
    assert_eq!((known.mmm.as_deref(), known.note.as_str()), (Some("cyborg"), "Pasted from a chat, then rewritten."));

    let note = &notes(&c, &p, &t2)[0];
    assert_eq!(note.body, "Version 0.1 of **plan.md** was published from the prompt **Write the plan**. It was made with generative AI.");
    assert_eq!((note.prompt_id.as_deref(), note.prompt_title.as_deref()), (Some(prompt.as_str()), Some("Write the plan")));
    assert!(notes(&c, &p, &t1).is_empty(), "it went where the person said");

    let prov = provenance::for_deliverable(&c, &p, "plan.md").unwrap();
    assert_eq!((prov.summary.versions, prov.summary.ai, prov.summary.human, prov.summary.unrecorded), (1, 1, 0, 0));
    assert!(prov.history_has_ai);
    assert_eq!(prov.summary.label.as_ref().map(|l| (l.level.as_str(), l.version.as_str())), Some(("cyborg", "0.1")));
    assert_eq!(prov.suggested_label.as_deref(), Some("cyborg"));
    let listed = deliverables::list(&c, &p).unwrap();
    assert_eq!(listed.iter().find(|d| d.path == "plan.md").unwrap().mmm.as_deref(), Some("cyborg"), "the list shows the label too");
}

#[test]
fn a_prompt_only_explains_a_change_that_ai_made() {
    let (c, p, t1, _, prompt) = setup();
    publish(&c, &p, "# Plan\n\nby hand\n", &choose(|x| x.prompt = Some(prompt.clone())));
    let known = &provenance::resolve(&c, &p, "plan.md").unwrap()[0];
    assert_eq!((known.author, known.prompt.as_deref()), ("human", None));
    assert_eq!(notes(&c, &p, &t1)[0].prompt_id, None);
}

#[test]
fn a_version_can_be_filed_in_no_thread_and_then_has_no_note() {
    let (c, p, t1, _, _) = setup();
    publish(&c, &p, "# Plan\n\nquietly\n", &choose(|x| x.thread = Some(String::new())));
    assert!(notes(&c, &p, &t1).is_empty());

    let prov = provenance::for_deliverable(&c, &p, "plan.md").unwrap();
    let last = prov.groups.last().unwrap();
    assert!(last.thread.is_none(), "what is filed nowhere comes last");
    assert!(matches!(&last.entries[0], Entry::Version(v) if v.version == "0.1" && v.by == "human" && v.post.is_none()));
}

#[test]
fn what_the_person_chose_is_checked_before_anything_is_published() {
    let (c, p, _, _, _) = setup();
    let root = tempdir().unwrap();
    let try_with = |choices: PublishChoices| message(deliverables::publish_with(&c, root.path(), &p, "plan.md", Bump::Minor, None, &choices).unwrap_err());
    assert_eq!(try_with(choose(|x| x.thread = Some("ghost".into()))), "Thread not found");
    assert_eq!(try_with(choose(|x| x.mmm = Some("robot".into()))), "That is not one of the AI-use labels");
    assert_eq!(try_with(choose(|x| {
        x.ai = true;
        x.prompt = Some("nope".into());
    })), "Prompt not found");
    assert_eq!(try_with(choose(|x| x.run = Some("nope".into()))), "Run not found");
    assert!(deliverables::versions_of(&c, &p, "plan.md").unwrap().is_empty(), "nothing was half published");
    assert!(provenance::resolve(&c, &p, "plan.md").unwrap().is_empty());
}

#[test]
fn a_version_goes_to_the_thread_the_deliverable_was_last_filed_in_unless_told_otherwise() {
    let (c, p, t1, t2, _) = setup();
    assert_eq!(provenance::default_thread(&c, &p, "plan.md").unwrap().as_deref(), Some(t1.as_str()), "the prompt that asked for it");

    publish(&c, &p, "# Plan\n\none\n", &choose(|x| x.thread = Some(t2.clone())));
    assert_eq!(provenance::default_thread(&c, &p, "plan.md").unwrap().as_deref(), Some(t2.as_str()), "where it went last");
    let v = publish(&c, &p, "# Plan\n\ntwo\n", &PublishChoices::default());
    assert_eq!(v, "0.2");
    assert_eq!(notes(&c, &p, &t2).len(), 2);
    assert!(notes(&c, &p, &t1).is_empty());

    // A deliverable nothing has asked for, published for the first time with no thread chosen, is filed nowhere.
    deliverables::create(&c, &p, "Loose", "").unwrap();
    assert_eq!(provenance::default_thread(&c, &p, "loose.md").unwrap(), None);
}

// ---- what is known of versions made before there were records -----------------------------------------------

/// Three versions of plan.md with their records removed, as if they were made before records existed: 0.1 is listed
/// by a run, 0.2 is only announced in a thread, 0.3 is announced nowhere.
fn legacy() -> (Connection, String, String, String, String) {
    let (c, p, t1, t2, prompt) = setup();
    publish(&c, &p, "# Plan\n\none\n", &PublishChoices::default());
    publish(&c, &p, "# Plan\n\ntwo\n", &choose(|x| x.thread = Some(t2.clone())));
    publish(&c, &p, "# Plan\n\nthree\n", &choose(|x| x.thread = Some(String::new())));
    c.execute("DELETE FROM version_records", []).unwrap();
    run(&c, &p, &t1, &prompt, "run-a", "finished", &["plan@v0.1.md"]);
    (c, p, t1, t2, prompt)
}

#[test]
fn a_version_a_run_published_is_known_to_be_ai_without_a_record() {
    let (c, p, t1, _, prompt) = legacy();
    let known = provenance::resolve(&c, &p, "plan.md").unwrap();
    assert_eq!(known.iter().map(|v| v.version.as_str()).collect::<Vec<_>>(), ["0.1", "0.2", "0.3"]);
    assert_eq!((known[0].author, known[0].thread.as_deref(), known[0].prompt.as_deref(), known[0].run.as_deref()), ("ai", Some(t1.as_str()), Some(prompt.as_str()), Some("run-a")));
    assert!(known.iter().all(|v| v.record.is_none()));
}

#[test]
fn a_version_only_a_note_announced_is_filed_with_it_and_not_taken_for_a_persons() {
    let (c, p, _, t2, _) = legacy();
    let known = provenance::resolve(&c, &p, "plan.md").unwrap();
    assert_eq!((known[1].author, known[1].thread.as_deref()), ("", Some(t2.as_str())));
    assert!(known[1].post.is_some());
}

#[test]
fn a_version_nothing_mentions_has_no_thread_and_no_origin() {
    let (c, p, _, _, _) = legacy();
    let known = provenance::resolve(&c, &p, "plan.md").unwrap();
    assert_eq!((known[2].author, known[2].thread.as_deref(), known[2].post.is_none()), ("", None, true));
    assert!(known[2].sha256.is_some(), "a markdown version's hash is worked out from its text");

    let prov = provenance::for_deliverable(&c, &p, "plan.md").unwrap();
    assert_eq!((prov.summary.ai, prov.summary.human, prov.summary.unrecorded), (1, 0, 2));
    assert_eq!(version(&prov, "0.3").by, "unknown");
    assert!(prov.groups.last().unwrap().thread.is_none());
}

#[test]
fn marking_a_version_writes_a_record_that_keeps_what_was_known_of_it() {
    let (c, p, _, t2, _) = legacy();
    let mark = |version: &str, by: Option<&str>, mmm: Option<&str>| {
        provenance::update_record(&c, &p, "plan.md", version, &VersionUpdate { by: by.map(str::to_string), mmm: mmm.map(str::to_string), note: None })
    };

    mark("0.2", Some("human"), Some("handyman")).unwrap();
    let known = provenance::resolve(&c, &p, "plan.md").unwrap();
    assert_eq!((known[1].author, known[1].thread.as_deref()), ("human", Some(t2.as_str())));
    assert_eq!(known[1].mmm.as_deref(), Some("handyman"));
    assert!(known[1].record.is_some() && known[0].record.is_none(), "only the marked one got a record");

    // A label alone leaves the origin as it was found.
    mark("0.1", None, Some("piggybacker")).unwrap();
    let known = provenance::resolve(&c, &p, "plan.md").unwrap();
    assert_eq!((known[0].author, known[0].run.as_deref(), known[0].mmm.as_deref()), ("ai", Some("run-a"), Some("piggybacker")));
    mark("0.1", None, Some("")).unwrap();
    assert_eq!(provenance::resolve(&c, &p, "plan.md").unwrap()[0].mmm, None, "an empty label removes it");

    // What Ledger saw a run do is not the person's to change; the rest is.
    assert_eq!(message(mark("0.1", Some("human"), None).unwrap_err()), "Claude Code made this version, which Ledger saw happen. That can't be changed here.");
    mark("0.2", Some("ai"), None).unwrap();
    assert_eq!(provenance::resolve(&c, &p, "plan.md").unwrap()[1].author, "ai");
    assert_eq!(message(mark("0.2", Some("robot"), None).unwrap_err()), "A version is made by a person (human) or by AI (ai)");
    assert_eq!(message(mark("0.2", None, Some("robot")).unwrap_err()), "That is not one of the AI-use labels");
    assert_eq!(message(mark("9.9", Some("ai"), None).unwrap_err()), "Version not found");
}

// ---- records outlive what they point at ---------------------------------------------------------------------

#[test]
fn a_version_keeps_its_record_when_its_note_prompt_and_thread_are_deleted() {
    let (c, p, t1, t2, prompt) = setup();
    publish(&c, &p, "# Plan\n\none\n", &choose(|x| {
        x.thread = Some(t2.clone());
        x.ai = true;
        x.prompt = Some(prompt.clone());
    }));
    let root = tempdir().unwrap();

    let note = notes(&c, &p, &t2)[0].id.clone();
    posts::delete(&c, &p, &t2, &note).unwrap();
    prompts::delete(&c, &p, &t1, &prompt).unwrap();
    let known = &provenance::resolve(&c, &p, "plan.md").unwrap()[0];
    assert_eq!((known.author, known.thread.as_deref(), known.prompt.as_deref()), ("ai", Some(t2.as_str()), Some(prompt.as_str())));
    assert!(known.post.is_none());

    threads::delete(&c, root.path(), &p, &t2).unwrap();
    let prov = provenance::for_deliverable(&c, &p, "plan.md").unwrap();
    let group = prov.groups.iter().find(|g| g.thread.as_ref().is_some_and(|t| t.slug == t2)).unwrap();
    let thread = group.thread.as_ref().unwrap();
    assert_eq!((thread.title.as_str(), thread.exists), ("A deleted thread", false));
    let v = version(&prov, "0.1");
    assert_eq!((v.by.as_str(), v.prompt.as_ref().map(|x| (x.title.as_str(), x.exists))), ("ai", Some(("A deleted prompt", false))));
}

// ---- the side window --------------------------------------------------------------------------------------

#[test]
fn the_side_window_has_a_group_per_thread_with_what_happened_there_in_order() {
    let (c, p, t1, t2, prompt) = setup();
    publish(&c, &p, "# Plan\n\none\n", &PublishChoices::default());
    publish(&c, &p, "# Plan\n\ntwo\n", &choose(|x| x.thread = Some(t2.clone())));
    feedback::add(&c, &p, &t2, "plan@v0.2.md", "Needs a budget.\n\nAnd dates.").unwrap();
    publish(&c, &p, "# Plan\n\nthree\n", &choose(|x| x.thread = Some(t1.clone())));
    run(&c, &p, &t1, &prompt, "run-b", "cancelled", &["plan.md"]);

    let prov = provenance::for_deliverable(&c, &p, "plan.md").unwrap();
    let shape: Vec<(String, Vec<String>)> = prov
        .groups
        .iter()
        .map(|g| {
            let name = g.thread.as_ref().map_or("-".to_string(), |t| t.slug.clone());
            let kinds = g
                .entries
                .iter()
                .map(|e| match e {
                    Entry::Prompt { .. } => "prompt".to_string(),
                    Entry::Run(r) => format!("run {}", r.run.id),
                    Entry::Version(v) => format!("v{}", v.version),
                    Entry::Feedback { .. } => "feedback".to_string(),
                })
                .collect();
            (name, kinds)
        })
        .collect();
    // The thread the deliverable showed up in first comes first; later changes to it stay under it.
    assert_eq!(shape, [(t1.clone(), vec!["prompt".into(), "v0.1".into(), "v0.3".into(), "run run-b".into()]), (t2.clone(), vec!["v0.2".into(), "feedback".into()])]);

    let heading = prov.groups[0].thread.as_ref().unwrap();
    assert_eq!((heading.title.as_str(), heading.exists), ("Kick-off", true));
    let changes: Vec<_> = prov.groups.iter().flat_map(|g| &g.entries).filter_map(|e| if let Entry::Version(v) = e { Some((v.version.clone(), v.size_change.is_some())) } else { None }).collect();
    assert_eq!(changes, [("0.1".to_string(), false), ("0.3".to_string(), true), ("0.2".to_string(), true)]);

    // It goes to the window as JSON, each entry tagged with what it is.
    let json = serde_json::to_value(&prov).unwrap();
    assert_eq!(json["groups"][0]["entries"][0]["kind"], "prompt");
    assert_eq!(json["groups"][0]["entries"][1]["kind"], "version");
    assert_eq!(json["groups"][0]["entries"][1]["sizeChange"], serde_json::Value::Null);
    assert_eq!(json["groups"][0]["entries"][3]["kind"], "run");
    assert_eq!(json["groups"][0]["entries"][3]["permissionMode"], "acceptEdits");
    assert_eq!(json["groups"][1]["thread"]["slug"], t2.as_str());
}

#[test]
fn a_run_shows_under_the_version_it_published_and_a_stopped_run_shows_on_its_own() {
    let (c, p, t1, _, prompt) = setup();
    // A run starts, and when it ends publishes what it wrote: the version comes after the run began.
    run(&c, &p, &t1, &prompt, "run-a", "finished", &["plan@v0.1.md"]);
    let origin = provenance::Origin::run(&t1, &prompt, Some("run-a"));
    deliverables::save(&c, &p, "plan.md", "# Plan\n\nby the run\n").unwrap();
    let root = tempdir().unwrap();
    assert_eq!(deliverables::publish_run(&c, root.path(), &p, "plan.md", None, &origin).unwrap(), "0.1");

    let prov = provenance::for_deliverable(&c, &p, "plan.md").unwrap();
    let entries = &prov.groups[0].entries;
    assert!(matches!(&entries[1], Entry::Run(r) if r.run.id == "run-a" && r.version.as_deref() == Some("0.1") && r.run.model == "claude-test"), "{entries:?}");
    let made = version(&prov, "0.1");
    assert_eq!((made.by.as_str(), made.run.as_ref().map(|r| r.turns)), ("ai", Some(Some(3))));
    assert_eq!(made.prompt.as_ref().map(|x| x.title.as_str()), Some("Write the plan"));
    assert_eq!(notes(&c, &p, &t1)[0].body, "Version 0.1 of **plan.md** was published from the prompt **Write the plan**.");
    assert_eq!(notes(&c, &p, &t1)[0].prompt_id.as_deref(), Some(prompt.as_str()));
}

#[test]
fn the_live_file_is_a_draft_while_it_holds_something_no_version_does() {
    let (c, p, _, _, _) = setup();
    let prov = provenance::for_deliverable(&c, &p, "plan.md").unwrap();
    assert_eq!(prov.draft.as_ref().map(|d| d.since.clone()), Some(None), "never published");

    publish(&c, &p, "# Plan\n\none\n", &PublishChoices::default());
    assert!(provenance::for_deliverable(&c, &p, "plan.md").unwrap().draft.is_none());

    deliverables::save(&c, &p, "plan.md", "# Plan\n\none, edited\n").unwrap();
    let draft = provenance::for_deliverable(&c, &p, "plan.md").unwrap().draft.unwrap();
    assert_eq!(draft.since.as_deref(), Some("0.1"));
    assert!(draft.stopped_run.is_none());
}

#[test]
fn a_run_that_changed_the_file_and_left_it_out_of_any_version_is_remembered_until_a_newer_version() {
    let (c, p, t1, _, prompt) = setup();
    publish(&c, &p, "# Plan\n\none\n", &PublishChoices::default());
    run(&c, &p, &t1, &prompt, "run-stopped", "cancelled", &["plan.md"]);

    let stopped = provenance::stopped_run(&c, &p, "plan.md").unwrap().unwrap();
    assert_eq!((stopped.id.as_str(), stopped.status.as_str(), stopped.prompt_title.as_str()), ("run-stopped", "cancelled", "Write the plan"));
    let prov = provenance::for_deliverable(&c, &p, "plan.md").unwrap();
    assert_eq!(prov.stopped_run.as_ref().map(|r| r.id.as_str()), Some("run-stopped"), "the publish dialog starts with AI ticked");

    // The person publishes it, saying a run made it: the run is on the record, and the warning goes.
    let v = publish(&c, &p, "# Plan\n\nhalf done by the run\n", &choose(|x| {
        x.ai = true;
        x.run = Some("run-stopped".into());
    }));
    let known = provenance::resolve(&c, &p, "plan.md").unwrap();
    let last = known.last().unwrap();
    assert_eq!((last.version.as_str(), last.author, last.run.as_deref()), (v.as_str(), "ai", Some("run-stopped")));
    assert!(provenance::stopped_run(&c, &p, "plan.md").unwrap().is_none());
}

#[test]
fn files_that_are_not_markdown_are_hashed_from_their_bytes() {
    let (c, p, ..) = setup();
    let dir = tempdir().unwrap();
    let folder = deliverables::deliverables_dir(dir.path(), &p);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("chart.png"), [1u8, 2, 3]).unwrap();
    deliverables::upsert(&c, &p, "chart.png", 3, "2026-01-01T00:00:00.000Z", None).unwrap();
    deliverables::publish_with(&c, dir.path(), &p, "chart.png", Bump::Minor, None, &PublishChoices::default()).unwrap();

    let known = &provenance::resolve(&c, &p, "chart.png").unwrap()[0];
    assert_eq!(known.sha256.as_deref(), Some(provenance::sha256_hex(&[1, 2, 3]).as_str()));
    assert_eq!(provenance::sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
}
