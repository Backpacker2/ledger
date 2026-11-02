//! Behavioural tests against an in-memory database: the rules the stores promise, end to end.

use rusqlite::Connection;

use crate::error::Error;
use crate::models::{Bump, Criterion, StoryDraft, StoryUpdate};
use crate::posts::AddOptions;
use crate::{db, document_types, documents, markdown, planboard, posts, search, stories, threads};

fn conn() -> Connection {
    db::open_in_memory().unwrap()
}

fn reply(to: &str) -> AddOptions {
    AddOptions { reply_to_id: Some(to.to_string()), ..Default::default() }
}

fn story(id: &str, summary: &str) -> StoryDraft {
    StoryDraft { id: id.into(), summary: summary.into(), ..Default::default() }
}

fn message(e: Error) -> String {
    e.to_string()
}

// ---- threads and posts ------------------------------------------------------------------------------

#[test]
fn thread_slugs_are_unique_and_titles_required() {
    let c = conn();
    assert_eq!(threads::create(&c, "Launch plan").unwrap().slug, "launch-plan");
    assert_eq!(threads::create(&c, "Launch plan").unwrap().slug, "launch-plan-2");
    assert_eq!(threads::create(&c, "  ").unwrap_err().to_string(), "Title is required");
    assert_eq!(threads::create(&c, "!!!").unwrap().slug, "thread");
}

#[test]
fn posts_keep_insertion_order_and_replies_must_exist_in_the_same_thread() {
    let c = conn();
    let a = threads::create(&c, "A").unwrap().slug;
    let b = threads::create(&c, "B").unwrap().slug;

    let first = posts::add(&c, &a, "  first  ", AddOptions::default()).unwrap();
    assert_eq!(first.body, "first");
    let second = posts::add(&c, &a, "second", reply(&first.id)).unwrap();
    assert_eq!(second.parent_id.as_deref(), Some(first.id.as_str()));

    let order: Vec<_> = posts::list(&c, &a).unwrap().into_iter().map(|p| p.body).collect();
    assert_eq!(order, ["first", "second"]);

    assert_eq!(message(posts::add(&c, &b, "x", reply(&first.id)).unwrap_err()), "Post being replied to was not found");
    assert_eq!(message(posts::add(&c, &a, "   ", AddOptions::default()).unwrap_err()), "Post body is required");
    assert_eq!(message(posts::add(&c, "nope", "x", AddOptions::default()).unwrap_err()), "Thread not found");
}

#[test]
fn deleting_a_post_keeps_its_replies() {
    let c = conn();
    let t = threads::create(&c, "T").unwrap().slug;
    let parent = posts::add(&c, &t, "parent", AddOptions::default()).unwrap();
    posts::add(&c, &t, "child", reply(&parent.id)).unwrap();

    posts::delete(&c, &t, &parent.id).unwrap();

    let left = posts::list(&c, &t).unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].parent_id.as_deref(), Some(parent.id.as_str()));
}

#[test]
fn updating_a_post_changes_body_and_timestamp() {
    let c = conn();
    let t = threads::create(&c, "T").unwrap().slug;
    let p = posts::add(&c, &t, "before", AddOptions::default()).unwrap();
    let updated = posts::update(&c, &t, &p.id, "after").unwrap();
    assert_eq!(updated.body, "after");
    assert_eq!(updated.created_at, p.created_at);
    assert!(updated.updated_at >= p.updated_at);
    assert!(posts::update(&c, &t, "missing", "x").is_err());
}

#[test]
fn thread_links_are_bidirectional_and_idempotent() {
    let c = conn();
    let a = threads::create(&c, "Alpha").unwrap().slug;
    let b = threads::create(&c, "Beta").unwrap().slug;

    threads::link(&c, &b, &a).unwrap();
    threads::link(&c, &a, &b).unwrap();

    assert_eq!(threads::linked_threads(&c, &a).unwrap()[0].slug, b);
    assert_eq!(threads::linked_threads(&c, &b).unwrap()[0].slug, a);
    let summaries = threads::list(&c).unwrap();
    assert!(summaries.iter().all(|t| t.linked_count == 1));

    assert_eq!(message(threads::link(&c, &a, &a).unwrap_err()), "A thread can't link to itself");
    assert_eq!(message(threads::link(&c, &a, "ghost").unwrap_err()), "Thread not found");

    threads::unlink(&c, &b, &a).unwrap();
    assert!(threads::linked_threads(&c, &a).unwrap().is_empty());
}

#[test]
fn thread_summaries_count_posts_and_order_by_latest_activity() {
    let c = conn();
    let quiet = threads::create(&c, "Quiet").unwrap().slug;
    let busy = threads::create(&c, "Busy").unwrap().slug;
    posts::add(&c, &busy, "one", AddOptions::default()).unwrap();
    posts::add(&c, &busy, "two", AddOptions::default()).unwrap();

    let list = threads::list(&c).unwrap();
    assert_eq!(list[0].slug, busy);
    assert_eq!(list[0].post_count, 2);
    assert_eq!(list[1].slug, quiet);
    assert_eq!(list[1].post_count, 0);
}

// ---- document types and documents -------------------------------------------------------------------

fn interview_type(c: &Connection) -> String {
    document_types::create(c, "Interview", Some("after a call"), Some(&["Who?".to_string(), "  ".to_string(), "Why?".to_string()]))
        .unwrap()
        .slug
}

#[test]
fn document_types_drop_blank_questions_and_keep_order() {
    let c = conn();
    let slug = interview_type(&c);
    let t = document_types::get(&c, &slug).unwrap();
    assert_eq!(t.questions, ["Who?", "Why?"]);
    assert_eq!(t.trigger, "after a call");

    let t = document_types::update(&c, &slug, Some("Interviews"), None, Some(&["Last?".to_string(), "First?".to_string()])).unwrap();
    assert_eq!(t.name, "Interviews");
    assert_eq!(t.trigger, "after a call");
    assert_eq!(t.questions, ["Last?", "First?"]);
}

#[test]
fn creating_a_document_in_a_thread_seeds_guiding_questions_once() {
    let c = conn();
    let ty = interview_type(&c);
    let t = threads::create(&c, "Customer calls").unwrap().slug;

    let doc = documents::create(&c, &ty, "Call with Ana", Some(&t)).unwrap();
    assert_eq!(doc.thread_slug.as_deref(), Some(t.as_str()));

    let seeded = posts::list(&c, &t).unwrap();
    assert_eq!(seeded.len(), 2);
    assert!(seeded.iter().all(|p| p.guiding_question));
    assert_eq!(seeded[0].body, "**Guiding question (Interview):** Who?");
    assert_eq!(seeded[1].body, "**Guiding question (Interview):** Why?");

    // Editing the type afterwards never reaches back into the started document.
    document_types::update(&c, &ty, None, None, Some(&["Something else?".to_string()])).unwrap();
    assert_eq!(posts::list(&c, &t).unwrap().len(), 2);
}

#[test]
fn start_drafting_links_an_existing_or_new_thread_exactly_once() {
    let c = conn();
    let ty = interview_type(&c);
    let doc = documents::create(&c, &ty, "Notes", None).unwrap();
    assert!(doc.thread_slug.is_none());

    assert_eq!(message(documents::start_drafting(&c, &doc.id, None, None).unwrap_err()), "Provide a thread to link, or a title for a new thread");
    assert_eq!(message(documents::start_drafting(&c, &doc.id, Some("ghost"), None).unwrap_err()), "Thread not found");

    let started = documents::start_drafting(&c, &doc.id, None, Some("Fresh thread")).unwrap();
    assert_eq!(started.thread_slug.as_deref(), Some("fresh-thread"));
    assert_eq!(posts::list(&c, "fresh-thread").unwrap().len(), 2);

    assert_eq!(message(documents::start_drafting(&c, &doc.id, Some("fresh-thread"), None).unwrap_err()), "Document is already linked to a thread");
}

#[test]
fn a_failed_start_leaves_no_orphan_thread_or_questions() {
    let c = conn();
    let ty = interview_type(&c);
    let t = threads::create(&c, "T").unwrap().slug;
    assert!(documents::create(&c, "missing-type", "Doc", Some(&t)).is_err());
    assert!(documents::create(&c, &ty, "Doc", Some("ghost")).is_err());
    assert!(documents::list_all(&c).unwrap().is_empty());
    assert!(posts::list(&c, &t).unwrap().is_empty());
}

#[test]
fn version_bumping_follows_major_minor_rules() {
    assert_eq!(documents::bump_version(None, Bump::Minor), "0.1");
    assert_eq!(documents::bump_version(None, Bump::Major), "1.0");
    assert_eq!(documents::bump_version(Some("1.0"), Bump::Minor), "1.1");
    assert_eq!(documents::bump_version(Some("1.4"), Bump::Major), "2.0");
    assert_eq!(documents::bump_version(Some("0.9"), Bump::Minor), "0.10");

    assert_eq!(documents::parse_version("1.4"), Some((1, 4)));
    for bad in ["1", "1.", ".1", "1.2.3", "a.b", "1.-2", " 1.2", ""] {
        assert_eq!(documents::parse_version(bad), None, "{bad:?}");
    }
}

#[test]
fn publishing_snapshots_the_draft_and_posts_a_note_in_the_thread() {
    let c = conn();
    let ty = interview_type(&c);
    let t = threads::create(&c, "T").unwrap().slug;
    let doc = documents::create(&c, &ty, "Report", Some(&t)).unwrap();

    documents::update(&c, &doc.id, None, Some("# v1 body")).unwrap();
    let published = documents::publish(&c, &doc.id, Bump::Minor, None).unwrap();
    assert_eq!(published.version.as_deref(), Some("0.1"));

    // The draft moves on; the snapshot does not.
    documents::update(&c, &doc.id, Some("Report (renamed)"), Some("# v2 body")).unwrap();
    let again = documents::publish(&c, &doc.id, Bump::Major, None).unwrap();
    assert_eq!(again.version.as_deref(), Some("1.0"));
    assert_eq!(again.versions, ["0.1", "1.0"]);

    let old = documents::get_version(&c, &doc.id, "0.1").unwrap();
    assert!(old.snapshot && !old.deleted);
    assert_eq!(old.body, "# v1 body");
    assert_eq!(old.title, "Report");

    let notes: Vec<_> = posts::list(&c, &t).unwrap().into_iter().filter(|p| p.version_published).collect();
    assert_eq!(notes.len(), 2);
    assert_eq!(notes[0].body, "Version 0.1 of **Report** was just published.");
    assert_eq!(notes[0].document_id.as_deref(), Some(doc.id.as_str()));
    assert_eq!(notes[0].document_version.as_deref(), Some("0.1"));
}

#[test]
fn explicit_versions_are_validated_and_never_overwrite() {
    let c = conn();
    let ty = interview_type(&c);
    let doc = documents::create(&c, &ty, "Report", None).unwrap();

    assert_eq!(message(documents::publish(&c, &doc.id, Bump::Minor, Some("v2")).unwrap_err()), "Version must be in M.m format, e.g. 1.4");
    assert_eq!(documents::publish(&c, &doc.id, Bump::Minor, Some("3.2")).unwrap().version.as_deref(), Some("3.2"));
    assert_eq!(message(documents::publish(&c, &doc.id, Bump::Minor, Some("3.2")).unwrap_err()), "Version 3.2 is already published");
    // Auto-bumping continues from the highest published version.
    assert_eq!(documents::publish(&c, &doc.id, Bump::Minor, None).unwrap().version.as_deref(), Some("3.3"));
}

#[test]
fn published_versions_outlive_the_draft_and_deleting_a_thread_only_unlinks() {
    let c = conn();
    let ty = interview_type(&c);
    let t = threads::create(&c, "T").unwrap().slug;
    let doc = documents::create(&c, &ty, "Report", Some(&t)).unwrap();
    documents::publish(&c, &doc.id, Bump::Minor, None).unwrap();

    threads::delete(&c, &t).unwrap();
    let kept = documents::get(&c, &doc.id).unwrap();
    assert!(kept.thread_slug.is_none());

    documents::delete(&c, &doc.id).unwrap();
    assert!(documents::get(&c, &doc.id).is_err());
    let snapshot = documents::get_version(&c, &doc.id, "0.1").unwrap();
    assert!(snapshot.deleted);
}

#[test]
fn a_document_type_in_use_cannot_be_deleted() {
    let c = conn();
    let ty = interview_type(&c);
    let doc = documents::create(&c, &ty, "Report", None).unwrap();

    assert_eq!(message(document_types::delete(&c, &ty).unwrap_err()), "1 document(s) still use this type — delete them first");
    documents::delete(&c, &doc.id).unwrap();
    document_types::delete(&c, &ty).unwrap();
    assert!(document_types::list(&c).unwrap().is_empty());
}

// ---- user stories and the planboard -----------------------------------------------------------------

#[test]
fn story_ids_are_validated_and_unique_ignoring_case() {
    let c = conn();
    assert_eq!(message(stories::create(&c, story("", "x")).unwrap_err()), "ID is required");
    for bad in ["-MM01", "MM 01", "MM.01", &"x".repeat(41)] {
        assert!(stories::create(&c, story(bad, "x")).is_err(), "{bad:?}");
    }
    assert_eq!(message(stories::create(&c, story("MM01", "  ")).unwrap_err()), "Summary is required");

    let first = stories::create(&c, story("MM01", "Do the thing")).unwrap();
    assert_eq!(first.column_id, "backlog");
    assert_eq!(message(stories::create(&c, story("mm01", "Again")).unwrap_err()), "A user story with ID mm01 already exists");
    assert_eq!(stories::get(&c, "mm01").unwrap().id, "MM01");
}

#[test]
fn stories_keep_criteria_in_order_and_drop_blank_ones() {
    let c = conn();
    let draft = StoryDraft {
        criteria: vec![
            Criterion { text: " First ".into(), done: true },
            Criterion { text: "   ".into(), done: false },
            Criterion { text: "Second".into(), done: false },
        ],
        ..story("A1", "With criteria")
    };
    let created = stories::create(&c, draft).unwrap();
    let texts: Vec<_> = created.criteria.iter().map(|k| (k.text.as_str(), k.done)).collect();
    assert_eq!(texts, [("First", true), ("Second", false)]);

    let updated = stories::update(
        &c,
        "a1",
        StoryUpdate { criteria: Some(vec![Criterion { text: "Only".into(), done: true }]), summary: Some("Renamed".into()), ..Default::default() },
    )
    .unwrap();
    assert_eq!(updated.summary, "Renamed");
    assert_eq!(updated.criteria.len(), 1);
    assert_eq!(updated.want, "");
}

#[test]
fn stories_link_to_many_threads_and_survive_thread_deletion() {
    let c = conn();
    let a = threads::create(&c, "A").unwrap().slug;
    let b = threads::create(&c, "B").unwrap().slug;

    assert_eq!(message(stories::create(&c, StoryDraft { thread_slugs: vec!["ghost".into()], ..story("S1", "x") }).unwrap_err()), "Thread not found");
    stories::create(&c, StoryDraft { thread_slugs: vec![a.clone()], ..story("S1", "x") }).unwrap();
    stories::create(&c, StoryDraft { thread_slugs: vec![a.clone(), b.clone()], ..story("S2", "y") }).unwrap();

    assert_eq!(stories::list_for_thread(&c, &a).unwrap().len(), 2);
    assert_eq!(stories::list_for_thread(&c, &b).unwrap().len(), 1);

    stories::add_thread(&c, "S1", &b).unwrap();
    stories::add_thread(&c, "S1", &b).unwrap();
    assert_eq!(stories::get(&c, "S1").unwrap().threads.len(), 2);
    stories::remove_thread(&c, "S1", &a).unwrap();
    assert_eq!(stories::get(&c, "S1").unwrap().threads[0].slug, b);

    threads::delete(&c, &b).unwrap();
    assert!(stories::get(&c, "S1").unwrap().threads.is_empty());
    assert_eq!(stories::get(&c, "S2").unwrap().threads.len(), 1);
    assert_eq!(threads::list(&c).unwrap()[0].story_count, 1);
}

#[test]
fn stories_move_between_existing_columns_only() {
    let c = conn();
    stories::create(&c, story("S1", "x")).unwrap();

    let moved = stories::update(&c, "S1", StoryUpdate { column_id: Some("drafting".into()), ..Default::default() }).unwrap();
    assert_eq!(moved.column_id, "drafting");
    let err = stories::update(&c, "S1", StoryUpdate { column_id: Some("nowhere".into()), ..Default::default() }).unwrap_err();
    assert_eq!(message(err), "Column not found");
    assert_eq!(stories::get(&c, "S1").unwrap().column_id, "drafting");
}

#[test]
fn columns_with_stories_cannot_be_deleted_and_new_stories_follow_the_backlog() {
    let c = conn();
    let board = planboard::get(&c).unwrap();
    let names: Vec<_> = board.columns.iter().map(|k| k.name.as_str()).collect();
    assert_eq!(names, ["Backlog", "Drafting", "Documented"]);

    stories::create(&c, story("S1", "x")).unwrap();
    assert_eq!(message(planboard::delete_column(&c, "backlog").unwrap_err()), "Move or delete this column's user stories first");

    let columns = planboard::add_column(&c, "  Review ").unwrap();
    assert_eq!(columns.last().unwrap().name, "Review");
    let review = columns.last().unwrap().id.clone();
    planboard::rename_column(&c, &review, "Reviewed").unwrap();
    assert_eq!(planboard::columns(&c).unwrap().last().unwrap().name, "Reviewed");
    planboard::delete_column(&c, &review).unwrap();

    // Once the configured backlog column is gone, new stories fall back to the first column.
    stories::delete(&c, "S1").unwrap();
    planboard::delete_column(&c, "backlog").unwrap();
    assert_eq!(stories::create(&c, story("S2", "y")).unwrap().column_id, "drafting");
}

#[test]
fn the_planboard_always_keeps_one_column() {
    let c = conn();
    planboard::delete_column(&c, "backlog").unwrap();
    planboard::delete_column(&c, "drafting").unwrap();
    assert_eq!(message(planboard::delete_column(&c, "documented").unwrap_err()), "The planboard needs at least one column");
}

// ---- search and markdown ----------------------------------------------------------------------------

#[test]
fn search_finds_posts_and_documents_case_insensitively() {
    let c = conn();
    let ty = interview_type(&c);
    let t = threads::create(&c, "Roadmap").unwrap().slug;
    posts::add(&c, &t, "We should revisit the PRICING model next week", AddOptions::default()).unwrap();
    posts::add(&c, &t, "Unrelated", AddOptions::default()).unwrap();
    let doc = documents::create(&c, &ty, "Pricing memo", None).unwrap();
    documents::update(&c, &doc.id, None, Some("Details about pricing tiers")).unwrap();

    let hits = search::search(&c, "  pricing ").unwrap();
    assert_eq!(hits.len(), 2);
    assert!(hits.iter().any(|h| h.kind == "post" && h.thread_slug.as_deref() == Some("roadmap")));
    assert!(hits.iter().any(|h| h.kind == "document" && h.id == doc.id));
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
