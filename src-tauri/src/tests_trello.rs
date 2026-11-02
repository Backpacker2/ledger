//! The Trello sync against a fake Trello (in-memory boards, lists, cards, members and labels): no network, no real
//! credentials. The fake enforces the rules that matter to the sync (a card can only be given to a member of its
//! board, lists belong to boards) and keeps count of what was written to which card.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rusqlite::Connection;
use tempfile::{TempDir, tempdir};

use crate::error::{Error, Result};
use crate::frontmatter::{self, MetaExt};
use crate::models::{Criterion, StoryDraft, StoryView, TaskUpdate, TaskView};
use crate::state::Core;
use crate::sync::{self, Applier, Change as GitChange, Head, LocalOp, Remote};
use crate::trello::{self, Connect, Emit, Timing, TrelloEvent};
use crate::trello_api::*;
use crate::trello_card::*;
use crate::trello_config::{self as config, ProjectLink};
use crate::trello_ledger::{self, CoreLedger, Ledger};
use crate::trello_sync::{self, Mapping, Report};
use crate::{board, ids, projects, stories};

// ---- a fake Trello ------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fail {
    Message,
    RateLimited,
    Unauthorized,
}

fn failure(fail: Fail) -> TrelloError {
    match fail {
        Fail::Message => TrelloError::message("Trello refused the request."),
        // Trello's own "wait a little" is short; the worker's pause for rate limits is the longer of that and its own.
        Fail::RateLimited => TrelloError::RateLimited { retry_after: Duration::from_millis(10) },
        Fail::Unauthorized => TrelloError::Unauthorized,
    }
}

struct FakeBoard {
    id: String,
    name: String,
    members: Vec<Member>,
    lists: Vec<List>,
    labels: Vec<Label>,
    cards_made: i64,
}

#[derive(Default)]
struct World {
    boards: Vec<FakeBoard>,
    cards: Vec<Card>,
    me: Member,
    clock: i64,
    next: u64,
    calls: Vec<(String, Instant)>,
    created: Vec<String>,
    written: Vec<String>,
    labels_created: usize,
    fail_writes: Option<Fail>,
    fail_cards: Option<Fail>,
    /// Cards that exist but are left out of the board's card list (as if the list were cut short).
    hidden: HashSet<String>,
}

struct FakeTrello {
    world: Mutex<World>,
}

fn ana() -> Member {
    Member { id: "m-ana".into(), username: "ana".into(), full_name: "Ana Smith".into() }
}

fn bob() -> Member {
    Member { id: "m-bob".into(), username: "bob".into(), full_name: "Bob Jones".into() }
}

fn now_millis() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl FakeTrello {
    /// Ana's account, with a board "Work" (Backlog, doing, DONE and Ideas; Ana and Bob are members), a second
    /// board "Other" Ana is on, and a board "Bobs" she is not on.
    fn standard() -> FakeTrello {
        let board = |id: &str, name: &str, members: Vec<Member>, lists: &[(&str, &str)]| FakeBoard {
            id: id.into(),
            name: name.into(),
            members,
            lists: lists.iter().map(|(id, name)| List { id: id.to_string(), name: name.to_string() }).collect(),
            labels: vec![],
            cards_made: 0,
        };
        let world = World {
            boards: vec![
                board("b1", "Work", vec![ana(), bob()], &[("l1", "Backlog"), ("l2", "doing"), ("l3", "DONE"), ("l4", "Ideas")]),
                board("b2", "Other", vec![ana()], &[("o1", "Backlog")]),
                board("b3", "Bobs", vec![bob()], &[("p1", "Backlog")]),
            ],
            me: ana(),
            // An hour ago: what happened on Trello is older than anything done here a moment ago, unless a test says otherwise.
            clock: now_millis() - 3_600_000,
            ..World::default()
        };
        FakeTrello { world: Mutex::new(world) }
    }

    fn w(&self) -> std::sync::MutexGuard<'_, World> {
        self.world.lock().unwrap()
    }

    fn tick(w: &mut World) -> String {
        let at = w.clock;
        w.clock += 1000;
        ids::format_millis(at)
    }

    fn set_clock(&self, millis: i64) {
        self.w().clock = millis;
    }

    /// A time later than anything done here during the test.
    fn the_future(&self) {
        self.set_clock(now_millis() + 3_600_000);
    }

    fn label_id(w: &mut World, board: &str, name: &str) -> String {
        let b = w.boards.iter_mut().find(|b| b.id == board).unwrap();
        if let Some(l) = b.labels.iter().find(|l| l.name == name) {
            return l.id.clone();
        }
        let id = format!("lbl-{board}-{}", b.labels.len() + 1);
        b.labels.push(Label { id: id.clone(), name: name.into(), color: None });
        id
    }

    /// A label as the sync would find it (a label somebody made already, or made by the sync).
    fn label(&self, board: &str, name: &str) -> Option<String> {
        self.w().boards.iter().find(|b| b.id == board)?.labels.iter().find(|l| l.name == name).map(|l| l.id.clone())
    }

    /// A card somebody made on Trello. `labels` are names.
    fn add_card(&self, board: &str, list: &str, name: &str, desc: &str, members: &[&str], labels: &[&str]) -> String {
        let mut w = self.w();
        w.next += 1;
        let n = w.next;
        let id = format!("{:08x}{:016x}", 0x5f00_0000u64 + n, n);
        let label_ids = labels.iter().map(|l| Self::label_id(&mut w, board, l)).collect();
        let b = w.boards.iter_mut().find(|b| b.id == board).unwrap();
        b.cards_made += 1;
        let id_short = b.cards_made;
        let at = Self::tick(&mut w);
        w.cards.push(Card {
            id: id.clone(),
            id_short,
            name: name.into(),
            desc: desc.into(),
            id_list: list.into(),
            id_board: board.into(),
            id_labels: label_ids,
            id_members: members.iter().map(|m| m.to_string()).collect(),
            closed: false,
            date_last_activity: at,
        });
        id
    }

    /// A change made on Trello by somebody: counts as activity, but not as a write by the sync.
    fn edit(&self, id: &str, change: impl FnOnce(&mut Card)) {
        let mut w = self.w();
        let at = Self::tick(&mut w);
        let card = w.cards.iter_mut().find(|c| c.id == id).unwrap();
        change(card);
        card.date_last_activity = at;
    }

    fn get(&self, id: &str) -> Card {
        self.w().cards.iter().find(|c| c.id == id).cloned().unwrap_or_else(|| panic!("no card {id}"))
    }

    fn exists(&self, id: &str) -> bool {
        self.w().cards.iter().any(|c| c.id == id)
    }

    fn open_cards(&self, board: &str) -> Vec<Card> {
        self.w().cards.iter().filter(|c| c.id_board == board && !c.closed).cloned().collect()
    }

    fn archive(&self, id: &str) {
        self.edit(id, |c| c.closed = true);
    }

    fn delete(&self, id: &str) {
        self.w().cards.retain(|c| c.id != id);
    }

    fn unassign(&self, id: &str, member: &str) {
        self.edit(id, |c| c.id_members.retain(|m| m != member));
    }

    fn move_to_board(&self, id: &str, board: &str, list: &str) {
        self.edit(id, |c| {
            c.id_board = board.into();
            c.id_list = list.into();
        });
    }

    fn calls(&self, name: &str) -> usize {
        self.w().calls.iter().filter(|(n, _)| n == name).count()
    }

    fn call_times(&self, name: &str) -> Vec<Instant> {
        self.w().calls.iter().filter(|(n, _)| n == name).map(|(_, at)| *at).collect()
    }

    fn created(&self) -> Vec<String> {
        self.w().created.clone()
    }

    fn written(&self) -> Vec<String> {
        self.w().written.clone()
    }

    fn labels_created(&self) -> usize {
        self.w().labels_created
    }
}

impl Trello for FakeTrello {
    fn me(&self) -> TResult<Member> {
        let mut w = self.w();
        w.calls.push(("me".into(), Instant::now()));
        Ok(w.me.clone())
    }

    fn boards(&self) -> TResult<Vec<Board>> {
        let mut w = self.w();
        w.calls.push(("boards".into(), Instant::now()));
        let me = w.me.id.clone();
        Ok(w.boards.iter().filter(|b| b.members.iter().any(|m| m.id == me)).map(|b| Board { id: b.id.clone(), name: b.name.clone() }).collect())
    }

    fn board_members(&self, board: &str) -> TResult<Vec<Member>> {
        let mut w = self.w();
        w.calls.push(("board_members".into(), Instant::now()));
        w.boards.iter().find(|b| b.id == board).map(|b| b.members.clone()).ok_or_else(|| TrelloError::message("no such board"))
    }

    fn lists(&self, board: &str) -> TResult<Vec<List>> {
        let mut w = self.w();
        w.calls.push(("lists".into(), Instant::now()));
        w.boards.iter().find(|b| b.id == board).map(|b| b.lists.clone()).ok_or_else(|| TrelloError::message("no such board"))
    }

    fn labels(&self, board: &str) -> TResult<Vec<Label>> {
        let mut w = self.w();
        w.calls.push(("labels".into(), Instant::now()));
        w.boards.iter().find(|b| b.id == board).map(|b| b.labels.clone()).ok_or_else(|| TrelloError::message("no such board"))
    }

    fn create_label(&self, board: &str, name: &str, color: &str) -> TResult<Label> {
        let mut w = self.w();
        w.calls.push(("create_label".into(), Instant::now()));
        w.labels_created += 1;
        let id = Self::label_id(&mut w, board, name);
        let b = w.boards.iter_mut().find(|b| b.id == board).unwrap();
        let label = b.labels.iter_mut().find(|l| l.id == id).unwrap();
        label.color = Some(color.into());
        Ok(label.clone())
    }

    fn cards(&self, board: &str) -> TResult<Vec<Card>> {
        let mut w = self.w();
        w.calls.push(("cards".into(), Instant::now()));
        if let Some(fail) = w.fail_cards {
            return Err(failure(fail));
        }
        let hidden = w.hidden.clone();
        Ok(w.cards.iter().filter(|c| c.id_board == board && !c.closed && !hidden.contains(&c.id)).cloned().collect())
    }

    fn card(&self, id: &str) -> TResult<Option<Card>> {
        let mut w = self.w();
        w.calls.push(("card".into(), Instant::now()));
        Ok(w.cards.iter().find(|c| c.id == id).cloned())
    }

    fn create_card(&self, new: &NewCard) -> TResult<Card> {
        let mut w = self.w();
        w.calls.push(("create_card".into(), Instant::now()));
        if let Some(fail) = w.fail_writes {
            return Err(failure(fail));
        }
        let board = w
            .boards
            .iter()
            .find(|b| b.lists.iter().any(|l| l.id == new.list))
            .map(|b| (b.id.clone(), b.members.iter().any(|m| m.id == new.member)))
            .ok_or_else(|| TrelloError::message("invalid idList"))?;
        if !board.1 {
            return Err(TrelloError::message("invalid value for idMembers"));
        }
        if new.name.trim().is_empty() {
            return Err(TrelloError::message("invalid value for name"));
        }
        w.next += 1;
        let n = w.next;
        let id = format!("{:08x}{:016x}", 0x5f00_0000u64 + n, n);
        let b = w.boards.iter_mut().find(|b| b.id == board.0).unwrap();
        b.cards_made += 1;
        let id_short = b.cards_made;
        let at = Self::tick(&mut w);
        let card = Card {
            id: id.clone(),
            id_short,
            name: new.name.clone(),
            desc: new.desc.clone(),
            id_list: new.list.clone(),
            id_board: board.0,
            id_labels: new.labels.clone(),
            id_members: vec![new.member.clone()],
            closed: false,
            date_last_activity: at,
        };
        w.cards.push(card.clone());
        w.created.push(id.clone());
        w.written.push(id);
        Ok(card)
    }

    fn update_card(&self, id: &str, update: &CardUpdate) -> TResult<Card> {
        let mut w = self.w();
        w.calls.push(("update_card".into(), Instant::now()));
        if let Some(fail) = w.fail_writes {
            return Err(failure(fail));
        }
        let at = Self::tick(&mut w);
        let index = w.cards.iter().position(|c| c.id == id).ok_or_else(|| TrelloError::message("no such card"))?;
        if let Some(list) = &update.list {
            let board = w.cards[index].id_board.clone();
            if !w.boards.iter().any(|b| b.id == board && b.lists.iter().any(|l| l.id == *list)) {
                return Err(TrelloError::message("invalid idList"));
            }
        }
        let card = &mut w.cards[index];
        if let Some(name) = &update.name {
            card.name = name.clone();
        }
        if let Some(desc) = &update.desc {
            card.desc = desc.clone();
        }
        if let Some(list) = &update.list {
            card.id_list = list.clone();
        }
        card.date_last_activity = at;
        let card = card.clone();
        w.written.push(id.to_string());
        Ok(card)
    }
}

impl Trello for Arc<FakeTrello> {
    fn me(&self) -> TResult<Member> {
        (**self).me()
    }
    fn boards(&self) -> TResult<Vec<Board>> {
        (**self).boards()
    }
    fn board_members(&self, board: &str) -> TResult<Vec<Member>> {
        (**self).board_members(board)
    }
    fn lists(&self, board: &str) -> TResult<Vec<List>> {
        (**self).lists(board)
    }
    fn labels(&self, board: &str) -> TResult<Vec<Label>> {
        (**self).labels(board)
    }
    fn create_label(&self, board: &str, name: &str, color: &str) -> TResult<Label> {
        (**self).create_label(board, name, color)
    }
    fn cards(&self, board: &str) -> TResult<Vec<Card>> {
        (**self).cards(board)
    }
    fn card(&self, id: &str) -> TResult<Option<Card>> {
        (**self).card(id)
    }
    fn create_card(&self, new: &NewCard) -> TResult<Card> {
        (**self).create_card(new)
    }
    fn update_card(&self, id: &str, update: &CardUpdate) -> TResult<Card> {
        (**self).update_card(id, update)
    }
}

// ---- a machine: a vault, loaded ------------------------------------------------------------------------

struct Machine {
    dir: TempDir,
    _config: TempDir,
    core: Arc<Mutex<Core>>,
    ledger: CoreLedger,
    /// How often a round told the GitHub sync that the vault changed.
    changed: Arc<AtomicUsize>,
}

impl Machine {
    fn new() -> Machine {
        let dir = tempdir().unwrap();
        let config = tempdir().unwrap();
        let core = Arc::new(Mutex::new(Core::open_at(dir.path().to_path_buf(), config.path().to_path_buf(), false).unwrap()));
        let changed = Arc::new(AtomicUsize::new(0));
        let hook = changed.clone();
        let ledger = CoreLedger {
            core: core.clone(),
            changed: Arc::new(move || {
                hook.fetch_add(1, Ordering::SeqCst);
            }),
        };
        Machine { dir, _config: config, core, ledger, changed }
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    /// Reads from the database.
    fn read<T>(&self, f: impl FnOnce(&Connection) -> T) -> T {
        f(&self.core.lock().unwrap().conn)
    }

    /// Changes the database and writes the difference to the files, as the app does for every edit.
    fn edit<T>(&self, f: impl FnOnce(&Connection) -> T) -> T {
        let mut guard = self.core.lock().unwrap();
        let core = &mut *guard;
        let out = f(&core.conn);
        core.vault.persist(&core.conn).unwrap();
        out
    }

    fn project(&self, name: &str) -> String {
        self.edit(|c| projects::create(c, name).unwrap().slug)
    }

    fn task(&self, project: &str, title: &str, body: &str) -> String {
        self.edit(|c| board::create_task(c, project, title, body, None, None).unwrap().id)
    }

    fn update_task(&self, project: &str, id: &str, updates: TaskUpdate) {
        self.edit(|c| board::update_task(c, project, id, updates).unwrap());
    }

    fn move_task(&self, project: &str, id: &str, column: &str) {
        self.update_task(project, id, TaskUpdate { column_id: Some(column.into()), ..TaskUpdate::default() });
    }

    fn story(&self, project: &str, draft: StoryDraft) -> String {
        self.edit(|c| stories::create(c, project, draft).unwrap().id)
    }

    fn get_task(&self, project: &str, id: &str) -> TaskView {
        self.read(|c| board::task(c, project, id).unwrap_or_else(|e| panic!("task {id}: {e}")))
    }

    fn get_story(&self, project: &str, id: &str) -> StoryView {
        self.read(|c| stories::get(c, project, id).unwrap_or_else(|e| panic!("story {id}: {e}")))
    }

    fn tasks(&self, project: &str) -> Vec<TaskView> {
        self.read(|c| board::tasks(c, project).unwrap())
    }

    fn stories(&self, project: &str) -> Vec<StoryView> {
        self.read(|c| stories::list(c, project).unwrap())
    }

    fn titles(&self, project: &str) -> Vec<String> {
        let mut titles: Vec<String> = self.tasks(project).into_iter().map(|t| t.title).collect();
        titles.sort();
        titles
    }

    /// Sets the person and links the project to a board, as the settings screen would.
    fn link(&self, member: &Member, project: &str, board: &str, name: &str) {
        config::update_settings(self.root(), |s| {
            s.member = Some(member.clone());
            s.projects.insert(project.into(), ProjectLink { board: board.into(), board_name: name.into(), columns: BTreeMap::new() });
        })
        .unwrap();
    }

    fn try_run(&self, trello: &dyn Trello) -> TResult<Report> {
        let settings = config::read_settings(self.root());
        let snapshot = self.ledger.snapshot().unwrap();
        trello_sync::run(self.root(), trello, &self.ledger, &settings, &snapshot)
    }

    fn run(&self, trello: &FakeTrello) -> Report {
        self.try_run(trello).unwrap_or_else(|e| panic!("the round failed: {e}"))
    }

    fn file(&self, rel: &str) -> String {
        fs::read_to_string(self.root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
    }

    fn task_file(&self, project: &str, id: &str) -> String {
        self.file(&format!("projects/{project}/tasks/{id}.md"))
    }

    fn reload(&self) {
        self.core.lock().unwrap().reload().unwrap();
    }

    /// What another machine would get from the GitHub sync: every file under `projects/`, copied over.
    fn copy_vault_to(&self, other: &Machine) {
        fn copy(from: &Path, to: &Path) {
            for entry in fs::read_dir(from).unwrap().flatten() {
                let target = to.join(entry.file_name());
                if entry.file_type().unwrap().is_dir() {
                    fs::create_dir_all(&target).unwrap();
                    copy(&entry.path(), &target);
                } else {
                    fs::write(&target, fs::read(entry.path()).unwrap()).unwrap();
                }
            }
        }
        fs::create_dir_all(other.root().join("projects")).unwrap();
        copy(&self.root().join("projects"), &other.root().join("projects"));
        other.reload();
    }

    fn conflict_files(&self) -> Vec<(String, String)> {
        fn walk(dir: &Path, base: &Path, out: &mut Vec<(String, String)>) {
            let Ok(entries) = fs::read_dir(dir) else { return };
            for entry in entries.flatten() {
                if entry.file_type().unwrap().is_dir() {
                    walk(&entry.path(), base, out);
                } else {
                    let rel = entry.path().strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
                    out.push((rel, fs::read_to_string(entry.path()).unwrap()));
                }
            }
        }
        let base = self.root().join(".ledger/conflicts");
        let mut out = vec![];
        walk(&base, &base, &mut out);
        out.sort();
        out
    }
}

/// One machine with a project "acme" linked to the board "Work" for Ana, and the fake Trello it talks to.
fn setup() -> (FakeTrello, Machine, String) {
    let trello = FakeTrello::standard();
    let machine = Machine::new();
    let project = machine.project("Acme");
    machine.link(&ana(), &project, "b1", "Work");
    (trello, machine, project)
}

fn story_draft(id: &str, summary: &str) -> StoryDraft {
    StoryDraft {
        id: id.into(),
        summary: summary.into(),
        role: "customer".into(),
        want: "see prices".into(),
        reason: "I can decide".into(),
        notes: "Raised on the first call.".into(),
        criteria: vec![Criterion { text: "A page lists plans".into(), done: true }, Criterion { text: "Prices in euro".into(), done: false }],
        thread_slugs: vec![],
    }
}

const STORY_DESC: &str = "**As a** customer\n**I want to** see prices\n**Such that** I can decide\n\n**Acceptance criteria**\n- [x] A page lists plans\n- [ ] Prices in euro\n\n**Notes**\nRaised on the first call.";

fn footer() -> &'static str {
    "---\n_Synced with Ledger_"
}

// ---- the card format ----------------------------------------------------------------------------------

#[test]
fn a_story_is_written_as_a_readable_description() {
    let text = StoryText {
        role: "customer".into(),
        want: "see prices".into(),
        reason: "I can decide".into(),
        criteria: vec![Check { text: "A page lists plans".into(), done: true }, Check { text: "Prices in euro".into(), done: false }],
        notes: "Raised on the first call.".into(),
    };
    assert_eq!(render_story(&text), STORY_DESC);
    assert_eq!(parse_story(STORY_DESC), text);

    // Parts that are empty are left out.
    let only_want = StoryText { want: "see prices".into(), ..StoryText::default() };
    assert_eq!(render_story(&only_want), "**I want to** see prices");
    assert_eq!(parse_story("**I want to** see prices"), only_want);
    assert_eq!(render_story(&StoryText::default()), "");
}

#[test]
fn a_story_survives_the_trip_to_a_card_and_back() {
    let samples = [
        StoryText::default(),
        StoryText { notes: "Just notes.".into(), ..StoryText::default() },
        StoryText { role: "admin".into(), want: "export **everything**".into(), reason: "audits".into(), ..StoryText::default() },
        // Awkward text: markers and list syntax inside the values and in the notes (which come last and are verbatim).
        StoryText {
            role: "a\nb".into(),
            want: "`code` and [x] boxes".into(),
            reason: "- [ ] not a criterion".into(),
            criteria: vec![Check { text: "[x] looks done but isn't".into(), done: false }, Check { text: "**bold** item".into(), done: true }],
            notes: "**Notes**\n**Acceptance criteria**\n- [x] inside the notes\n\n---\n_Synced with Ledger_ is just text here".into(),
        },
    ];
    for text in samples {
        let rendered = render_story(&text);
        assert_eq!(parse_story(&rendered), text, "{rendered}");
        // What comes back renders to the same text: nothing keeps changing.
        assert_eq!(render_story(&parse_story(&rendered)), rendered);
    }
}

#[test]
fn a_description_written_by_hand_loses_no_text() {
    // No part of the format at all: all of it is notes.
    let (text, marked) = parse_story_marked("Please make this faster.\n\nSee the thread.");
    assert!(!marked);
    assert_eq!(text, StoryText { notes: "Please make this faster.\n\nSee the thread.".into(), ..StoryText::default() });

    // Text in front of the parts, and between them, goes in front of the notes.
    let (text, marked) = parse_story_marked("Intro line\n\n**As a** user\n\nstray words\n\n**Notes**\nthe notes");
    assert!(marked);
    assert_eq!(text.role, "user");
    assert_eq!(text.notes, "Intro line\n\nstray words\n\nthe notes");

    // Boxes in any common spelling, bare bullets, and a line that is not a list item.
    let (text, _) = parse_story_marked("**Acceptance criteria**\n* [X] shout\n- plain bullet\nnot a bullet\n+ [ ] plus");
    assert_eq!(
        text.criteria,
        [
            Check { text: "shout".into(), done: true },
            Check { text: "plain bullet".into(), done: false },
            Check { text: "plus".into(), done: false }
        ]
    );
    assert_eq!(text.notes, "not a bullet");

    // Line endings and capitals don't matter.
    let (text, _) = parse_story_marked("**as a** user\r\n**I WANT TO** go\r\n");
    assert_eq!((text.role.as_str(), text.want.as_str()), ("user", "go"));
}

#[test]
fn the_footer_marks_a_card_and_comes_off_again() {
    assert_eq!(with_footer(""), footer());
    assert_eq!(with_footer("Body"), format!("Body\n\n{}", footer()));
    for body in ["", "Body", "Two\n\nparagraphs", "ends with the footer's text\n\n---\n_Synced with Ledger_"] {
        let (stripped, had) = strip_footer(&with_footer(body));
        assert!(had);
        assert_eq!(stripped, body);
    }
    // A description that merely mentions it, or doesn't end with it, isn't marked.
    assert_eq!(strip_footer("Body"), ("Body".to_string(), false));
    assert!(!strip_footer("see ---\n_Synced with Ledger_").1);
    assert!(!strip_footer("---\n_Synced with Ledger_ and more").1);
}

#[test]
fn columns_and_lists_match_by_name_unless_told_otherwise() {
    let columns: Vec<crate::models::Column> = [("backlog", "Backlog"), ("doing", "Doing"), ("done", "Done"), ("review", " review ")]
        .iter()
        .map(|(id, name)| crate::models::Column { id: id.to_string(), name: name.to_string() })
        .collect();
    let lists: Vec<List> = [("l1", "BACKLOG"), ("l2", "doing"), ("l3", "Shipped"), ("l4", "Review"), ("l5", "Doing")]
        .iter()
        .map(|(id, name)| List { id: id.to_string(), name: name.to_string() })
        .collect();

    // By name, ignoring case and surrounding space; the first list with a name is the column's.
    let mapping = Mapping::new(&columns, &lists, &BTreeMap::new());
    assert_eq!(mapping.list_for("backlog"), Some("l1"));
    assert_eq!(mapping.list_for("doing"), Some("l2"));
    assert_eq!(mapping.list_for("done"), None, "no list is called Done");
    assert_eq!(mapping.list_for("review"), Some("l4"));
    assert_eq!(mapping.column_for("l2"), Some("doing"));
    assert_eq!(mapping.column_for("l5"), None, "a second list of the same name has no column");
    assert_eq!(mapping.column_for("l3"), None);

    // A choice beats the name; "don't sync" (empty) leaves the column without a list; a list is one column's only.
    let overrides: BTreeMap<String, String> =
        [("done".to_string(), "l3".to_string()), ("backlog".to_string(), String::new()), ("review".to_string(), "l2".to_string())].into();
    let mapping = Mapping::new(&columns, &lists, &overrides);
    assert_eq!(mapping.list_for("done"), Some("l3"));
    assert_eq!(mapping.list_for("backlog"), None);
    assert_eq!(mapping.list_for("review"), Some("l2"));
    assert_eq!(mapping.list_for("doing"), Some("l5"), "l2 went to review, so doing takes the next list called doing");
    assert_eq!(mapping.column_for("l1"), None);

    // A choice that points at a list that is gone leaves the column without one.
    let gone: BTreeMap<String, String> = [("done".to_string(), "l99".to_string())].into();
    assert_eq!(Mapping::new(&columns, &lists, &gone).list_for("done"), None);
}

// ---- Ledger to Trello ---------------------------------------------------------------------------------

#[test]
fn a_new_task_becomes_a_card_with_the_person_assigned() {
    let (trello, m, p) = setup();
    let id = m.task(&p, "Draw the page", "In figma.");
    let before = m.get_task(&p, &id);

    let report = m.run(&trello);
    assert_eq!((report.cards_created, report.cards_updated, report.items_created), (1, 0, 0));

    let cards = trello.open_cards("b1");
    assert_eq!(cards.len(), 1);
    let card = &cards[0];
    assert_eq!(card.name, "Draw the page");
    assert_eq!(card.desc, format!("In figma.\n\n{}", footer()));
    assert_eq!(card.id_list, "l1", "a new task is in the backlog column, whose list is Backlog");
    assert_eq!(card.id_members, ["m-ana"], "the person is assigned automatically");
    assert_eq!(card.id_labels, [trello.label("b1", "Task").unwrap()]);
    // The two labels did not exist and were made.
    assert_eq!(trello.labels_created(), 2);
    assert!(trello.label("b1", "User story").is_some());

    // The card id is in the task's own frontmatter; linking is not an edit.
    let task = m.get_task(&p, &id);
    assert_eq!(task.trello_card.as_deref(), Some(card.id.as_str()));
    assert_eq!(task.trello_unlinked, None);
    assert_eq!(task.updated_at, before.updated_at);
    assert!(m.task_file(&p, &id).contains(&format!("trelloCard: {}", card.id)), "{}", m.task_file(&p, &id));
    assert!(m.changed.load(Ordering::SeqCst) > 0, "the GitHub sync is told about the vault change");
}

#[test]
fn a_new_story_becomes_a_card_in_the_backlog_list_with_a_readable_description() {
    let (trello, m, p) = setup();
    m.story(&p, story_draft("MM01", "Clarify pricing"));
    let report = m.run(&trello);
    assert_eq!(report.cards_created, 1);

    let card = trello.open_cards("b1").pop().unwrap();
    assert_eq!(card.name, "Clarify pricing");
    assert_eq!(card.desc, format!("{STORY_DESC}\n\n{}", footer()));
    assert_eq!(card.id_list, "l1", "stories go to the list of the backlog column");
    assert_eq!(card.id_labels, [trello.label("b1", "User story").unwrap()]);
    assert_eq!(card.id_members, ["m-ana"]);
    assert_eq!(m.get_story(&p, "MM01").trello_card.as_deref(), Some(card.id.as_str()));
    assert!(m.file(&format!("projects/{p}/userstories/MM01.md")).contains(&format!("trelloCard: {}", card.id)));
}

#[test]
fn labels_that_are_already_on_the_board_are_used_not_made_again() {
    let (trello, m, p) = setup();
    // The board has "task" in lower case already (and the sync matches names ignoring case).
    let existing = {
        let mut w = trello.w();
        FakeTrello::label_id(&mut w, "b1", "task")
    };
    m.task(&p, "A", "");
    m.run(&trello);
    assert_eq!(trello.labels_created(), 1, "only 'User story' was missing");
    assert_eq!(trello.open_cards("b1")[0].id_labels, [existing]);
}

#[test]
fn nothing_is_made_on_trello_for_a_project_with_no_board() {
    let trello = FakeTrello::standard();
    let m = Machine::new();
    let p = m.project("Acme");
    let other = m.project("Other");
    m.link(&ana(), &other, "b2", "Other"); // only "Other" is linked
    m.task(&p, "Not synced", "");
    m.task(&other, "Synced", "");
    m.run(&trello);
    assert_eq!(trello.open_cards("b1").len(), 0);
    assert_eq!(trello.open_cards("b2").iter().map(|c| c.name.clone()).collect::<Vec<_>>(), ["Synced"]);
    assert_eq!(m.get_task(&p, &m.tasks(&p)[0].id).trello_card, None);
}

#[test]
fn a_task_in_a_column_with_no_list_is_left_out_and_reported() {
    let (trello, m, p) = setup();
    m.edit(|c| board::add_column(c, &p, "Review").unwrap());
    let review = m.read(|c| board::columns(c, &p).unwrap()).into_iter().find(|c| c.name == "Review").unwrap().id;
    let stuck = m.task(&p, "Needs review", "");
    m.move_task(&p, &stuck, &review);
    m.task(&p, "Fine", "");

    let report = m.run(&trello);
    assert_eq!(report.cards_created, 1);
    assert_eq!(report.unmatched, 1);
    assert!(report.notes.iter().any(|n| n.contains("Review") && n.contains("no list")), "{:?}", report.notes);
    assert_eq!(m.get_task(&p, &stuck).trello_card, None);
    // Said again every round, until a list is chosen for the column.
    assert_eq!(m.run(&trello).unmatched, 1);
}

// ---- Trello to Ledger ---------------------------------------------------------------------------------

#[test]
fn only_the_selected_persons_cards_are_ever_read_or_touched() {
    let (trello, m, p) = setup();
    let mine = trello.add_card("b1", "l2", "Mine", "Details", &["m-ana"], &["Task"]);
    let shared = trello.add_card("b1", "l1", "Shared with Bob", "", &["m-ana", "m-bob"], &[]);
    let bobs = trello.add_card("b1", "l1", "Bob's card", "secret plans", &["m-bob"], &[]);
    let nobodys = trello.add_card("b1", "l1", "Nobody's card", "", &[], &["Task"]);
    let bobs_story = trello.add_card("b1", "l1", "Bob's story", "**As a** boss", &["m-bob"], &["User story"]);
    let activity: Vec<_> = [&bobs, &nobodys, &bobs_story].iter().map(|id| trello.get(id)).collect();

    let report = m.run(&trello);
    assert_eq!(report.items_created, 2);
    assert_eq!(m.titles(&p), ["Mine", "Shared with Bob"]);
    assert!(m.stories(&p).is_empty(), "Bob's story did not come in");
    assert_eq!(m.get_task(&p, &format!("trello-{mine}")).column_id, "doing");
    assert_eq!(m.get_task(&p, &format!("trello-{shared}")).trello_card.as_deref(), Some(shared.as_str()));

    // Edits here, moves there, new items here: other people's cards are not read into the vault and never written.
    m.update_task(&p, &format!("trello-{mine}"), TaskUpdate { title: Some("Mine, renamed".into()), ..TaskUpdate::default() });
    m.task(&p, "New here", "");
    trello.edit(&bobs, |c| c.name = "Bob renamed his card".into());
    m.run(&trello);
    m.run(&trello);
    assert_eq!(m.titles(&p), ["Mine, renamed", "New here", "Shared with Bob"]);
    let written = trello.written();
    for other in [&bobs, &nobodys, &bobs_story] {
        assert!(!written.contains(other), "a card that isn't Ana's was written");
    }
    assert_eq!(trello.get(&nobodys), activity[1]);
    assert_eq!(trello.get(&bobs_story), activity[2]);
    assert_eq!(trello.get(&bobs).desc, "secret plans");
    // Nothing of Bob's reached the vault, not even as text.
    assert!(!m.tasks(&p).iter().any(|t| t.body.contains("secret") || t.title.contains("Bob's")));
}

#[test]
fn a_card_comes_in_as_a_task_in_the_column_of_its_list() {
    let (trello, m, p) = setup();
    let id = trello.add_card("b1", "l2", "Fix login", "Users can't sign in.", &["m-ana"], &["Task"]);
    let report = m.run(&trello);
    assert_eq!((report.items_created, report.cards_created, report.cards_updated), (1, 0, 0));

    let task = m.get_task(&p, &format!("trello-{id}"));
    assert_eq!((task.title.as_str(), task.body.as_str(), task.column_id.as_str()), ("Fix login", "Users can't sign in.", "doing"));
    assert_eq!(task.trello_card.as_deref(), Some(id.as_str()));
    // Made from the card alone, so that every machine that imports it makes the same file.
    let card = trello.get(&id);
    assert_eq!(task.created_at, card_created_at(&id).unwrap());
    assert_eq!(task.updated_at, card.date_last_activity);
    assert!(m.task_file(&p, &task.id).contains(&format!("trelloCard: {id}")));
    // The card was only read.
    assert!(trello.written().is_empty());
}

#[test]
fn a_card_with_the_story_label_comes_in_as_a_story_and_keeps_its_parts() {
    let (trello, m, p) = setup();
    let id = trello.add_card("b1", "l1", "Clarify pricing", STORY_DESC, &["m-ana"], &["User story"]);
    let plain = trello.add_card("b1", "l1", "No label", "", &["m-ana"], &[]);
    let both = trello.add_card("b1", "l1", "Both labels", "", &["m-ana"], &["Task", "User story"]);
    m.run(&trello);

    assert_eq!(m.titles(&p), ["No label"], "no label is a task");
    let mut kinds: Vec<_> = m.stories(&p).into_iter().map(|s| s.summary).collect();
    kinds.sort();
    assert_eq!(kinds, ["Both labels", "Clarify pricing"], "the story label wins");
    let story = m.stories(&p).into_iter().find(|s| s.summary == "Clarify pricing").unwrap();
    assert_eq!(story.id, "TR-1");
    assert_eq!(
        (story.role.as_str(), story.want.as_str(), story.reason.as_str(), story.notes.as_str()),
        ("customer", "see prices", "I can decide", "Raised on the first call.")
    );
    assert_eq!(story.criteria.iter().map(|c| (c.text.as_str(), c.done)).collect::<Vec<_>>(), [("A page lists plans", true), ("Prices in euro", false)]);
    assert_eq!(story.trello_card.as_deref(), Some(id.as_str()));
    assert!(m.get_task(&p, &format!("trello-{plain}")).trello_card.is_some());
    assert!(trello.get(&both).id_labels.len() == 2, "labels are only read");
}

#[test]
fn a_card_in_a_list_with_no_column_is_not_added_and_is_reported() {
    let (trello, m, p) = setup();
    trello.add_card("b1", "l4", "An idea", "", &["m-ana"], &["Task"]);
    trello.add_card("b1", "l1", "A task", "", &["m-ana"], &["Task"]);
    let report = m.run(&trello);
    assert_eq!(m.titles(&p), ["A task"]);
    assert_eq!(report.unmatched, 1);
    assert!(report.notes.iter().any(|n| n.contains("Ideas") && n.contains("no column")), "{:?}", report.notes);

    // A column made later, with that name, takes the cards of that list.
    m.edit(|c| board::add_column(c, &p, "ideas").unwrap());
    m.run(&trello);
    assert_eq!(m.titles(&p), ["A task", "An idea"]);
}

#[test]
fn a_column_can_be_given_a_list_of_another_name_or_none() {
    let (trello, m, p) = setup();
    config::update_settings(m.root(), |s| {
        let link = s.projects.get_mut(&p).unwrap();
        link.columns.insert("backlog".into(), String::new()); // don't sync Backlog
        link.columns.insert("doing".into(), "l4".into()); // Doing is the Ideas list
    })
    .unwrap();
    let in_backlog = m.task(&p, "Stays here", "");
    let in_doing = m.task(&p, "On the Ideas list", "");
    m.move_task(&p, &in_doing, "doing");
    trello.add_card("b1", "l2", "A card in 'doing' (no column uses that list now)", "", &["m-ana"], &["Task"]);
    let idea = trello.add_card("b1", "l4", "A card in Ideas", "", &["m-ana"], &["Task"]);

    let report = m.run(&trello);
    assert_eq!(m.get_task(&p, &in_backlog).trello_card, None, "a column set to 'don't sync' has no list");
    let card = trello.open_cards("b1").into_iter().find(|c| c.name == "On the Ideas list").unwrap();
    assert_eq!(card.id_list, "l4");
    assert_eq!(m.get_task(&p, &format!("trello-{idea}")).column_id, "doing");
    assert!(m.titles(&p).iter().all(|t| !t.starts_with("A card in 'doing'")), "{:?}", m.titles(&p));
    assert_eq!(report.unmatched, 2, "the task in Backlog, and the card in the list that no column uses");
}

// ---- both ways: edits and moves -----------------------------------------------------------------------

#[test]
fn edits_to_a_task_go_to_its_card_and_edits_to_the_card_come_back() {
    let (trello, m, p) = setup();
    let id = m.task(&p, "Draw the page", "In figma.");
    m.run(&trello);
    let card_id = m.get_task(&p, &id).trello_card.unwrap();

    // Here: title and body.
    m.update_task(&p, &id, TaskUpdate { title: Some("Draw the pricing page".into()), body: Some("In figma, then review.".into()), ..TaskUpdate::default() });
    let report = m.run(&trello);
    assert_eq!((report.cards_updated, report.items_updated, report.conflicts), (1, 0, 0));
    let card = trello.get(&card_id);
    assert_eq!(card.name, "Draw the pricing page");
    assert_eq!(card.desc, format!("In figma, then review.\n\n{}", footer()));

    // There: title and description, edited by hand (the footer may be gone).
    trello.edit(&card_id, |c| {
        c.name = "Pricing page".into();
        c.desc = "Reviewed.".into();
    });
    let report = m.run(&trello);
    assert_eq!((report.cards_updated, report.items_updated, report.conflicts), (0, 1, 0));
    let task = m.get_task(&p, &id);
    assert_eq!((task.title.as_str(), task.body.as_str()), ("Pricing page", "Reviewed."));
    assert_eq!(task.updated_at, trello.get(&card_id).date_last_activity);
    assert!(m.changed.load(Ordering::SeqCst) > 0);

    // Both are in step now: another round does nothing, and a hand-edited description is left as it is.
    let writes = trello.written().len();
    let report = m.run(&trello);
    assert_eq!(report.summary(), "Up to date");
    assert_eq!((report.vault_writes, trello.written().len()), (0, writes));
    assert_eq!(trello.get(&card_id).desc, "Reviewed.");
}

#[test]
fn edits_to_a_story_go_to_its_card_and_the_parts_come_back_whole() {
    let (trello, m, p) = setup();
    m.story(&p, story_draft("MM01", "Clarify pricing"));
    m.run(&trello);
    let card_id = m.get_story(&p, "MM01").trello_card.unwrap();

    // Here: tick a criterion, change the goal, add a criterion.
    m.edit(|c| {
        stories::update(
            c,
            &p,
            "MM01",
            crate::models::StoryUpdate {
                want: Some("see the prices".into()),
                criteria: Some(vec![
                    Criterion { text: "A page lists plans".into(), done: true },
                    Criterion { text: "Prices in euro".into(), done: true },
                    Criterion { text: "Prices in dollars".into(), done: false },
                ]),
                ..Default::default()
            },
        )
        .unwrap()
    });
    m.run(&trello);
    let desc = trello.get(&card_id).desc;
    assert!(desc.contains("**I want to** see the prices") && desc.contains("- [x] Prices in euro") && desc.contains("- [ ] Prices in dollars"), "{desc}");

    // There: the description is edited in Trello (role changed, criterion unticked, one removed), and the title.
    trello.edit(&card_id, |c| {
        c.name = "Clarify the pricing".into();
        c.desc = "**As a** shopper\n**I want to** see the prices\n**Such that** I can decide\n\n**Acceptance criteria**\n- [ ] A page lists plans\n- [x] Prices in euro\n\n**Notes**\nRaised on the first call.".into();
    });
    m.run(&trello);
    let story = m.get_story(&p, "MM01");
    assert_eq!((story.summary.as_str(), story.role.as_str(), story.want.as_str(), story.reason.as_str()), ("Clarify the pricing", "shopper", "see the prices", "I can decide"));
    assert_eq!(story.criteria.iter().map(|c| (c.text.as_str(), c.done)).collect::<Vec<_>>(), [("A page lists plans", false), ("Prices in euro", true)]);
    assert_eq!(story.notes, "Raised on the first call.");

    let writes = trello.written().len();
    assert_eq!(m.run(&trello).summary(), "Up to date");
    assert_eq!(trello.written().len(), writes);
}

#[test]
fn a_description_written_freely_in_trello_keeps_the_stories_parts() {
    let (trello, m, p) = setup();
    m.story(&p, story_draft("MM01", "Clarify pricing"));
    m.run(&trello);
    let card_id = m.get_story(&p, "MM01").trello_card.unwrap();

    // Someone replaces the structured description with a plain sentence.
    trello.edit(&card_id, |c| c.desc = "Ask finance first.".into());
    m.run(&trello);
    let story = m.get_story(&p, "MM01");
    assert_eq!(story.notes, "Ask finance first.");
    assert_eq!((story.role.as_str(), story.want.as_str(), story.reason.as_str()), ("customer", "see prices", "I can decide"));
    assert_eq!(story.criteria.len(), 2);

    // The next round writes the structure back into the description, with their sentence as the notes; then it is quiet.
    m.run(&trello);
    let desc = trello.get(&card_id).desc;
    assert!(desc.contains("**As a** customer") && desc.contains("**Notes**\nAsk finance first."), "{desc}");
    let writes = trello.written().len();
    assert_eq!(m.run(&trello).summary(), "Up to date");
    assert_eq!(trello.written().len(), writes);
}

#[test]
fn moving_a_task_moves_its_card_and_moving_the_card_moves_the_task() {
    let (trello, m, p) = setup();
    let id = m.task(&p, "Draw the page", "");
    m.run(&trello);
    let card_id = m.get_task(&p, &id).trello_card.unwrap();
    assert_eq!(trello.get(&card_id).id_list, "l1");

    m.move_task(&p, &id, "doing");
    let report = m.run(&trello);
    assert_eq!(report.cards_updated, 1);
    assert_eq!(trello.get(&card_id).id_list, "l2", "Doing matches the list 'doing' ignoring case");

    trello.edit(&card_id, |c| c.id_list = "l3".into());
    m.run(&trello);
    assert_eq!(m.get_task(&p, &id).column_id, "done");
    assert_eq!(m.run(&trello).summary(), "Up to date");

    // Into a list no column uses: the task stays where it is, and that is said.
    trello.edit(&card_id, |c| c.id_list = "l4".into());
    let report = m.run(&trello);
    assert_eq!(m.get_task(&p, &id).column_id, "done");
    assert_eq!(report.unmatched, 1);
    assert_eq!(trello.get(&card_id).id_list, "l4", "the card is not pulled back either");
}

#[test]
fn a_linked_task_moved_to_a_column_with_no_list_keeps_syncing_its_text_but_is_not_moved() {
    let (trello, m, p) = setup();
    let id = m.task(&p, "Draw the page", "");
    m.run(&trello);
    let card_id = m.get_task(&p, &id).trello_card.unwrap();

    m.edit(|c| board::add_column(c, &p, "Review").unwrap());
    let review = m.read(|c| board::columns(c, &p).unwrap()).into_iter().find(|c| c.name == "Review").unwrap().id;
    m.move_task(&p, &id, &review);
    m.update_task(&p, &id, TaskUpdate { title: Some("Draw it".into()), ..TaskUpdate::default() });

    let report = m.run(&trello);
    assert_eq!(report.unmatched, 1);
    assert!(report.notes.iter().any(|n| n.contains("Review")), "{:?}", report.notes);
    let card = trello.get(&card_id);
    assert_eq!((card.name.as_str(), card.id_list.as_str()), ("Draw it", "l1"), "the text went across, the card stayed in its list");
    assert_eq!(m.get_task(&p, &id).column_id, review, "the task stayed where it was put");
}

#[test]
fn a_title_edited_here_and_a_card_moved_there_do_not_collide() {
    let (trello, m, p) = setup();
    let id = m.task(&p, "Draw the page", "");
    m.run(&trello);
    let card_id = m.get_task(&p, &id).trello_card.unwrap();

    m.update_task(&p, &id, TaskUpdate { title: Some("Draw the pricing page".into()), ..TaskUpdate::default() });
    trello.edit(&card_id, |c| c.id_list = "l3".into());
    let report = m.run(&trello);

    assert_eq!(report.conflicts, 0);
    let card = trello.get(&card_id);
    assert_eq!((card.name.as_str(), card.id_list.as_str()), ("Draw the pricing page", "l3"));
    let task = m.get_task(&p, &id);
    assert_eq!((task.title.as_str(), task.column_id.as_str()), ("Draw the pricing page", "done"));
    assert!(m.conflict_files().is_empty());
}

#[test]
fn a_task_edited_while_the_round_runs_is_left_for_the_next_round() {
    let (trello, m, p) = setup();
    let id = m.task(&p, "Draw the page", "");
    m.run(&trello);
    let card_id = m.get_task(&p, &id).trello_card.unwrap();
    trello.edit(&card_id, |c| c.name = "Renamed on Trello".into());

    // The round has read the vault; then the task is edited, before the round writes anything.
    let settings = config::read_settings(m.root());
    let snapshot = m.ledger.snapshot().unwrap();
    m.update_task(&p, &id, TaskUpdate { body: Some("Edited a moment ago".into()), ..TaskUpdate::default() });
    let report = trello_sync::run(m.root(), &trello, &m.ledger, &settings, &snapshot).unwrap();
    assert_eq!((report.items_updated, report.skipped), (0, 1));
    assert_eq!(m.get_task(&p, &id).title, "Draw the page", "nothing was written over the newer edit");
    assert_eq!(m.get_task(&p, &id).body, "Edited a moment ago");

    // The next round sees both changes and puts them together.
    let report = m.run(&trello);
    assert_eq!(report.conflicts, 0);
    let task = m.get_task(&p, &id);
    assert_eq!((task.title.as_str(), task.body.as_str()), ("Renamed on Trello", "Edited a moment ago"));
    assert_eq!(trello.get(&card_id).desc, format!("Edited a moment ago\n\n{}", footer()));
}

// ---- labels decide what an item is --------------------------------------------------------------------

#[test]
fn changing_the_label_on_a_card_changes_what_its_item_is() {
    let (trello, m, p) = setup();
    let id = m.task(&p, "Draw the page", "In figma.");
    m.run(&trello);
    let card_id = m.get_task(&p, &id).trello_card.unwrap();
    let story_label = trello.label("b1", "User story").unwrap();
    let task_label = trello.label("b1", "Task").unwrap();

    // Task to story.
    trello.edit(&card_id, |c| c.id_labels = vec![story_label.clone()]);
    let report = m.run(&trello);
    assert_eq!(report.converted, 1);
    assert!(m.tasks(&p).is_empty(), "the task became the story");
    let stories = m.stories(&p);
    assert_eq!(stories.len(), 1);
    assert_eq!((stories[0].summary.as_str(), stories[0].notes.as_str()), ("Draw the page", "In figma."));
    assert_eq!(stories[0].trello_card.as_deref(), Some(card_id.as_str()), "the card stays linked: no second card, no duplicate");
    assert_eq!(trello.open_cards("b1").len(), 1);
    assert_eq!(m.run(&trello).summary(), "Up to date");

    // A card with no label of its own kind, or none at all, leaves its item as it is.
    trello.edit(&card_id, |c| c.id_labels = vec![]);
    m.run(&trello);
    assert_eq!((m.stories(&p).len(), m.tasks(&p).len()), (1, 0));

    // Story to task, in the column of the card's list.
    trello.edit(&card_id, |c| {
        c.id_labels = vec![task_label.clone()];
        c.id_list = "l2".into();
    });
    let report = m.run(&trello);
    assert_eq!(report.converted, 1);
    assert!(m.stories(&p).is_empty());
    let task = m.get_task(&p, &format!("trello-{card_id}"));
    assert_eq!((task.title.as_str(), task.column_id.as_str(), task.trello_card.as_deref()), ("Draw the page", "doing", Some(card_id.as_str())));
    assert_eq!(m.run(&trello).summary(), "Up to date");
}

#[test]
fn a_story_turned_into_a_task_keeps_its_parts_in_the_body() {
    let (trello, m, p) = setup();
    m.story(&p, story_draft("MM01", "Clarify pricing"));
    m.run(&trello);
    let card_id = m.get_story(&p, "MM01").trello_card.unwrap();
    let task_label = trello.label("b1", "Task").unwrap();
    trello.edit(&card_id, |c| c.id_labels = vec![task_label]);
    m.run(&trello);

    let task = m.get_task(&p, &format!("trello-{card_id}"));
    assert_eq!(task.body, STORY_DESC, "role, goal, reason, criteria and notes are all still there, as text");
}

#[test]
fn a_change_made_on_both_sides_at_the_moment_of_a_relabel_is_set_aside() {
    let (trello, m, p) = setup();
    let id = m.task(&p, "Draw the page", "In figma.");
    m.run(&trello);
    let card_id = m.get_task(&p, &id).trello_card.unwrap();

    m.update_task(&p, &id, TaskUpdate { body: Some("A longer body written here".into()), ..TaskUpdate::default() });
    let story_label = trello.label("b1", "User story").unwrap();
    trello.edit(&card_id, |c| c.id_labels = vec![story_label]);
    m.run(&trello);

    assert_eq!(m.stories(&p)[0].notes, "In figma.", "the card says what the story is");
    let files = m.conflict_files();
    assert!(files.iter().any(|(path, text)| path.ends_with(&format!("tasks-{id}.ledger.md")) && text.contains("A longer body written here")), "{files:?}");
}

// ---- no duplicates ------------------------------------------------------------------------------------

#[test]
fn syncing_again_and_again_changes_nothing_and_makes_nothing() {
    let (trello, m, p) = setup();
    m.task(&p, "One", "body");
    m.story(&p, story_draft("MM01", "Two"));
    trello.add_card("b1", "l2", "Three", "from trello", &["m-ana"], &["Task"]);
    trello.add_card("b1", "l1", "Four", STORY_DESC, &["m-ana"], &["User story"]);

    let first = m.run(&trello);
    assert_eq!((first.cards_created, first.items_created), (2, 2));
    let (created, written) = (trello.created().len(), trello.written().len());
    let files_before = crate::vault::export(&m.core.lock().unwrap().conn).unwrap();
    let tasks = m.tasks(&p).len();

    for _ in 0..3 {
        let report = m.run(&trello);
        assert_eq!(report.summary(), "Up to date");
        assert_eq!((report.vault_writes, report.skipped, report.conflicts), (0, 0, 0));
    }
    assert_eq!((trello.created().len(), trello.written().len()), (created, written));
    assert_eq!(m.tasks(&p).len(), tasks);
    assert_eq!(crate::vault::export(&m.core.lock().unwrap().conn).unwrap(), files_before, "the vault was not touched");
    assert_eq!(trello.open_cards("b1").len(), 4);
}

#[test]
fn a_second_machine_that_has_the_card_id_makes_no_card_and_no_duplicate() {
    let (trello, a, p) = setup();
    let id = a.task(&p, "Draw the page", "In figma.");
    a.story(&p, story_draft("MM01", "Clarify pricing"));
    a.run(&trello);
    assert_eq!(trello.created().len(), 2);

    // The GitHub sync carries the files, card ids included, to a second machine with the same settings.
    let b = Machine::new();
    a.copy_vault_to(&b);
    b.link(&ana(), &p, "b1", "Work");
    assert_eq!(b.get_task(&p, &id).trello_card, a.get_task(&p, &id).trello_card);

    let report = b.run(&trello);
    assert_eq!((report.cards_created, report.cards_updated, report.items_created, report.vault_writes), (0, 0, 0, 0), "{:?}", report.notes);
    assert_eq!(trello.created().len(), 2, "no second card for either item");
    assert_eq!(trello.open_cards("b1").len(), 2);
    assert_eq!(b.tasks(&p).len(), 1);
    assert_eq!(b.stories(&p).len(), 1);

    // An edit on the second machine reaches the card, and the first machine takes it from there.
    b.update_task(&p, &id, TaskUpdate { title: Some("Draw the pricing page".into()), ..TaskUpdate::default() });
    assert_eq!(b.run(&trello).cards_updated, 1);
    assert_eq!(a.run(&trello).items_updated, 1);
    assert_eq!(a.get_task(&p, &id).title, "Draw the pricing page");
    assert_eq!(trello.created().len(), 2);
}

#[test]
fn two_machines_that_import_the_same_card_make_the_same_file() {
    let trello = FakeTrello::standard();
    trello.add_card("b1", "l2", "Fix login", "Users can't sign in.", &["m-ana"], &["Task"]);
    let story = trello.add_card("b1", "l1", "Clarify pricing", STORY_DESC, &["m-ana"], &["User story"]);

    let a = Machine::new();
    let b = Machine::new();
    for m in [&a, &b] {
        let p = m.project("Acme");
        m.link(&ana(), &p, "b1", "Work");
        m.run(&trello);
    }
    let (ta, tb) = (a.tasks("acme"), b.tasks("acme"));
    assert_eq!((ta.len(), tb.len()), (1, 1));
    assert_eq!(ta[0].id, tb[0].id);
    assert_eq!(a.task_file("acme", &ta[0].id), b.task_file("acme", &tb[0].id), "the same bytes, so the GitHub sync sees one file");
    let file = |m: &Machine| m.file("projects/acme/userstories/TR-2.md");
    assert_eq!(file(&a), file(&b));
    assert_eq!(a.get_story("acme", "TR-2").trello_card.as_deref(), Some(story.as_str()));
    assert!(trello.written().is_empty());
}

#[test]
fn a_card_made_by_ledger_is_never_imported_so_a_machine_that_polls_early_makes_no_duplicate() {
    let (trello, a, p) = setup();
    let id = a.task(&p, "Draw the page", "");
    a.run(&trello);
    assert_eq!(trello.created().len(), 1);

    // A second machine asks Trello before the item has reached it from GitHub.
    let b = Machine::new();
    let q = b.project("Acme");
    b.link(&ana(), &q, "b1", "Work");
    let report = b.run(&trello);
    assert_eq!(report.items_created, 0, "the card belongs to an item that is on its way");
    assert!(b.tasks(&q).is_empty());
    assert!(report.notes.iter().any(|n| n.contains("made by Ledger")), "{:?}", report.notes);

    // Then the item arrives, and everything is in step.
    a.copy_vault_to(&b);
    let report = b.run(&trello);
    assert_eq!((report.items_created, report.cards_created, report.cards_updated), (0, 0, 0));
    assert_eq!(b.tasks(&q).len(), 1);
    assert_eq!(b.get_task(&q, &id).trello_card, a.get_task(&p, &id).trello_card);
    assert_eq!(trello.created().len(), 1);

    // Taking the footer off a card says "add this one anyway".
    let loose = trello.add_card("b1", "l1", "Made by hand", "", &["m-ana"], &["Task"]);
    trello.edit(&loose, |c| c.desc = with_footer(""));
    assert_eq!(b.run(&trello).items_created, 0);
    trello.edit(&loose, |c| c.desc = String::new());
    assert_eq!(b.run(&trello).items_created, 1);
}

#[test]
fn two_items_claiming_one_card_leave_one_syncing_and_the_other_marked() {
    let (trello, m, p) = setup();
    let first = m.task(&p, "Original", "");
    m.run(&trello);
    let card_id = m.get_task(&p, &first).trello_card.unwrap();

    // A copy of the file under another id, as a merge of two machines' work could leave.
    let text = m.task_file(&p, &first).replace(&first, "copy-of-it");
    fs::write(m.root().join(format!("projects/{p}/tasks/copy-of-it.md")), text).unwrap();
    m.reload();
    assert_eq!(m.tasks(&p).len(), 2);

    let report = m.run(&trello);
    assert_eq!(report.detached, 1);
    assert_eq!(m.get_task(&p, &first).trello_unlinked, None, "the item that was syncing keeps the card");
    let copy = m.get_task(&p, "copy-of-it");
    assert!(copy.trello_unlinked.as_deref().unwrap().contains("same Trello card"), "{:?}", copy.trello_unlinked);
    assert_eq!(copy.trello_card.as_deref(), Some(card_id.as_str()));
    assert_eq!(trello.created().len(), 1);
}

#[test]
fn a_story_card_whose_number_is_taken_gets_an_id_that_is_free() {
    let (trello, m, p) = setup();
    // A story of the same id, ignoring case; marked as not syncing, so the clash is purely about ids.
    m.story(&p, story_draft("tr-1", "Mine, with a clashing id"));
    m.edit(|c| trello_ledger::set_link(c, &p, Kind::Story, "tr-1", Some("someone-elses-card"), Some("kept apart")).unwrap());
    let card = trello.add_card("b1", "l1", "From Trello", "", &["m-ana"], &["User story"]);
    m.run(&trello);
    let tail = &card[card.len() - 4..];
    let ids: Vec<_> = m.stories(&p).into_iter().map(|s| s.id).collect();
    assert!(ids.contains(&format!("TR-1-{tail}")), "{ids:?}");
}

// ---- conflicts ----------------------------------------------------------------------------------------

#[test]
fn when_both_sides_change_a_title_the_newer_edit_wins_and_the_other_is_kept() {
    // Trello's edit is the newer one.
    let (trello, m, p) = setup();
    let id = m.task(&p, "Draw the page", "In figma.");
    m.run(&trello);
    let card_id = m.get_task(&p, &id).trello_card.unwrap();
    m.update_task(&p, &id, TaskUpdate { title: Some("Ledger's title".into()), ..TaskUpdate::default() });
    trello.the_future();
    trello.edit(&card_id, |c| c.name = "Trello's title".into());

    let report = m.run(&trello);
    assert_eq!(report.conflicts, 1);
    assert_eq!(m.get_task(&p, &id).title, "Trello's title");
    assert_eq!(trello.get(&card_id).name, "Trello's title");
    let files = m.conflict_files();
    assert_eq!(files.len(), 1, "{files:?}");
    let (path, text) = &files[0];
    assert!(path.contains("/trello/") && path.ends_with(&format!("{p}/tasks-{id}.ledger.md")), "{path}");
    assert!(text.contains("Ledger's title") && text.contains("kept: ledger"), "{text}");
    assert!(report.notes.iter().any(|n| n.contains("changed on both sides")), "{:?}", report.notes);

    // Ledger's edit is the newer one.
    let (trello, m, p) = setup();
    let id = m.task(&p, "Draw the page", "In figma.");
    m.run(&trello);
    let card_id = m.get_task(&p, &id).trello_card.unwrap();
    trello.edit(&card_id, |c| c.name = "Trello's title".into());
    m.update_task(&p, &id, TaskUpdate { title: Some("Ledger's title".into()), ..TaskUpdate::default() });

    let report = m.run(&trello);
    assert_eq!(report.conflicts, 1);
    assert_eq!(m.get_task(&p, &id).title, "Ledger's title");
    assert_eq!(trello.get(&card_id).name, "Ledger's title");
    let files = m.conflict_files();
    assert_eq!(files.len(), 1, "{files:?}");
    assert!(files[0].0.ends_with(&format!("{p}/tasks-{id}.trello.md")), "{}", files[0].0);
    assert!(files[0].1.contains("Trello's title") && files[0].1.contains("In figma."), "{}", files[0].1);

    // Afterwards both sides agree, and nothing more is set aside.
    assert_eq!(m.run(&trello).summary(), "Up to date");
    assert_eq!(m.conflict_files().len(), 1);
}

#[test]
fn a_story_edited_on_both_sides_keeps_the_loser_whole() {
    let (trello, m, p) = setup();
    m.story(&p, story_draft("MM01", "Clarify pricing"));
    m.run(&trello);
    let card_id = m.get_story(&p, "MM01").trello_card.unwrap();
    m.edit(|c| stories::update(c, &p, "MM01", crate::models::StoryUpdate { want: Some("see all prices".into()), ..Default::default() }).unwrap());
    trello.the_future();
    trello.edit(&card_id, |c| c.desc = STORY_DESC.replace("customer", "visitor"));

    let report = m.run(&trello);
    assert_eq!(report.conflicts, 1);
    assert_eq!(m.get_story(&p, "MM01").role, "visitor");
    assert_eq!(m.get_story(&p, "MM01").want, "see prices", "the whole description went the winner's way");
    let kept = m.conflict_files();
    assert_eq!(kept.len(), 1);
    assert!(kept[0].1.contains("**I want to** see all prices"), "{}", kept[0].1);
}

#[test]
fn a_second_machine_with_no_record_of_the_last_sync_settles_a_difference_the_same_way() {
    let (trello, a, p) = setup();
    let id = a.task(&p, "Draw the page", "In figma.");
    a.run(&trello);
    let card_id = a.get_task(&p, &id).trello_card.unwrap();

    // The second machine has an older copy of the item (and has never synced it), while the card moved on.
    let b = Machine::new();
    a.copy_vault_to(&b);
    b.link(&ana(), &p, "b1", "Work");
    trello.the_future();
    trello.edit(&card_id, |c| c.name = "Renamed on Trello".into());

    let report = b.run(&trello);
    assert_eq!(b.get_task(&p, &id).title, "Renamed on Trello", "the newer edit won");
    assert_eq!(report.conflicts, 1);
    assert!(b.conflict_files().iter().any(|(_, text)| text.contains("Draw the page")), "the older version was kept");
    assert_eq!(b.run(&trello).summary(), "Up to date");
}

// ---- deletes never propagate --------------------------------------------------------------------------

/// A synced task whose card then goes through `change`; returns what is needed to look at the aftermath.
fn detached_by(change: impl FnOnce(&FakeTrello, &str)) -> (FakeTrello, Machine, String, String, String, Report) {
    let (trello, m, p) = setup();
    let id = m.task(&p, "Draw the page", "In figma.");
    m.run(&trello);
    let card_id = m.get_task(&p, &id).trello_card.unwrap();
    change(&trello, &card_id);
    let report = m.run(&trello);
    (trello, m, p, id, card_id, report)
}

fn assert_detached(trello: &FakeTrello, m: &Machine, p: &str, id: &str, card_id: &str, report: &Report, reason: &str) {
    assert_eq!((report.detached, report.unlinked), (1, 1));
    // The item is exactly as it was, says why it stopped, and keeps its card id.
    let task = m.get_task(p, id);
    assert_eq!((task.title.as_str(), task.body.as_str()), ("Draw the page", "In figma."));
    assert_eq!(task.trello_card.as_deref(), Some(card_id));
    assert!(task.trello_unlinked.as_deref().unwrap().contains(reason), "{:?}", task.trello_unlinked);
    assert!(m.task_file(p, id).contains("trelloUnlinked:"), "{}", m.task_file(p, id));

    // It stopped syncing: an edit here goes nowhere, and no new card is made for it.
    let written = trello.written();
    m.update_task(p, id, TaskUpdate { title: Some("Edited after".into()), ..TaskUpdate::default() });
    let report = m.run(trello);
    assert_eq!((report.detached, report.unlinked, report.cards_created, report.cards_updated), (0, 1, 0, 0));
    assert_eq!(trello.written(), written);
    assert_eq!(trello.created().len(), 1);
    assert_eq!(m.get_task(p, id).title, "Edited after");

    // The settings screen lists it with the reason.
    let snapshot = m.ledger.snapshot().unwrap();
    let view = trello::view(m.root(), &snapshot, Default::default());
    assert_eq!(view.unlinked.len(), 1);
    assert!(view.unlinked[0].reason.contains(reason), "{}", view.unlinked[0].reason);
}

#[test]
fn an_archived_card_leaves_its_item_in_place_and_stops_the_sync() {
    let (trello, m, p, id, card_id, report) = detached_by(|t, card| t.archive(card));
    assert_detached(&trello, &m, &p, &id, &card_id, &report, "archived");
    assert!(trello.exists(&card_id), "nothing was deleted on Trello");
}

#[test]
fn a_deleted_card_leaves_its_item_in_place_and_stops_the_sync() {
    let (trello, m, p, id, card_id, report) = detached_by(|t, card| t.delete(card));
    assert_detached(&trello, &m, &p, &id, &card_id, &report, "no longer exists");
}

#[test]
fn a_card_the_person_was_taken_off_leaves_its_item_in_place_and_stops_the_sync() {
    let (trello, m, p, id, card_id, report) = detached_by(|t, card| t.unassign(card, "m-ana"));
    assert_detached(&trello, &m, &p, &id, &card_id, &report, "no longer a member of the card");
    assert!(trello.exists(&card_id) && !trello.get(&card_id).closed);
}

#[test]
fn a_card_moved_to_another_board_leaves_its_item_in_place_and_stops_the_sync() {
    let (trello, m, p, id, card_id, report) = detached_by(|t, card| t.move_to_board(card, "b2", "o1"));
    assert_detached(&trello, &m, &p, &id, &card_id, &report, "another Trello board");
}

#[test]
fn what_stopped_syncing_on_one_machine_is_not_synced_by_another_either() {
    let (trello, a, p, id, _card, _) = detached_by(|t, card| t.archive(card));
    let b = Machine::new();
    a.copy_vault_to(&b);
    b.link(&ana(), &p, "b1", "Work");
    let report = b.run(&trello);
    assert_eq!((report.cards_created, report.cards_updated, report.detached, report.unlinked), (0, 0, 0, 1));
    assert_eq!(trello.created().len(), 1);
    assert!(b.get_task(&p, &id).trello_unlinked.is_some());
}

#[test]
fn deleting_an_item_here_leaves_its_card_alone_and_it_does_not_come_back() {
    let (trello, m, p) = setup();
    let made_here = m.task(&p, "Made here", "");
    let from_trello = trello.add_card("b1", "l1", "From Trello", "", &["m-ana"], &["Task"]);
    m.run(&trello);
    let here_card = m.get_task(&p, &made_here).trello_card.unwrap();
    let before = (trello.get(&here_card), trello.get(&from_trello));

    m.edit(|c| board::delete_task(c, &p, &made_here).unwrap());
    m.edit(|c| board::delete_task(c, &p, &format!("trello-{from_trello}")).unwrap());
    for _ in 0..3 {
        m.run(&trello);
    }
    assert!(m.tasks(&p).is_empty(), "neither came back");
    assert_eq!((trello.get(&here_card), trello.get(&from_trello)), before, "both cards are as they were");
    assert!(trello.exists(&here_card) && trello.exists(&from_trello));
    assert_eq!(trello.created().len(), 1);
    // This machine remembers it let them go.
    let state = config::read_state(m.root());
    assert!(state.released.contains(&here_card) && state.released.contains(&from_trello));
    assert!(state.items.is_empty());
}

#[test]
fn an_item_given_a_new_card_leaves_the_old_one_alone() {
    let (trello, m, p, id, card_id, _) = detached_by(|t, card| t.unassign(card, "m-ana"));
    // The old card gets its person back (or someone unarchives it) while the item is given a card of its own.
    let old = m.edit(|c| trello_ledger::clear_link(c, &p, Kind::Task, &id).unwrap()).unwrap();
    config::update_state(m.root(), |s| {
        s.released.insert(old.clone());
    })
    .unwrap();
    assert_eq!(old, card_id);
    trello.edit(&card_id, |c| c.id_members.push("m-ana".into()));

    let report = m.run(&trello);
    assert_eq!(report.cards_created, 1);
    assert_eq!(report.items_created, 0, "the old card does not come back as an item");
    assert_eq!(m.tasks(&p).len(), 1);
    let new_card = m.get_task(&p, &id).trello_card.unwrap();
    assert_ne!(new_card, card_id);
    assert_eq!(m.get_task(&p, &id).trello_unlinked, None);
    assert_eq!(trello.open_cards("b1").len(), 2);
}

#[test]
fn a_failed_listing_detaches_nothing_and_a_card_missing_from_it_is_looked_up() {
    let (trello, m, p) = setup();
    let id = m.task(&p, "Draw the page", "");
    m.run(&trello);
    let card_id = m.get_task(&p, &id).trello_card.unwrap();

    // The listing fails: the round fails, naming the project; nothing is judged from an answer that never came.
    trello.w().fail_cards = Some(Fail::Message);
    let error = m.try_run(&trello).unwrap_err().to_string();
    assert!(error.starts_with("Acme: ") && error.contains("refused"), "{error}");
    assert_eq!(m.get_task(&p, &id).trello_unlinked, None);
    assert_eq!(config::read_state(m.root()).items.len(), 1, "what is known about the card is kept");

    // A token that is refused, or a rate limit, ends the round with that error, also without judging anything.
    trello.w().fail_cards = Some(Fail::Unauthorized);
    assert!(matches!(m.try_run(&trello), Err(TrelloError::Unauthorized)));
    trello.w().fail_cards = Some(Fail::RateLimited);
    assert!(matches!(m.try_run(&trello), Err(TrelloError::RateLimited { .. })));
    trello.w().fail_cards = None;
    assert_eq!(m.get_task(&p, &id).trello_unlinked, None);

    // The listing leaves out a card that is there and still the person's (a cut-short list): it is not detached.
    trello.w().hidden.insert(card_id.clone());
    let report = m.run(&trello);
    assert_eq!((report.detached, report.unlinked), (0, 0));
    assert_eq!(m.get_task(&p, &id).trello_unlinked, None);
    assert!(trello.calls("card") > 0, "it was looked up by id");
}

#[test]
fn a_project_that_cannot_be_reached_is_a_note_and_the_others_still_sync() {
    let (trello, m, p) = setup();
    let lost = m.project("Lost");
    // Linked to a board that isn't there any more.
    config::update_settings(m.root(), |s| {
        s.projects.insert(lost.clone(), ProjectLink { board: "gone".into(), board_name: "Gone".into(), columns: BTreeMap::new() });
    })
    .unwrap();
    m.task(&p, "Fine", "");
    m.task(&lost, "Stranded", "");

    let report = m.run(&trello);
    assert_eq!(report.cards_created, 1);
    assert!(report.notes.iter().any(|n| n.starts_with("Lost: ") && n.contains("no such board")), "{:?}", report.notes);
    assert_eq!(m.get_task(&lost, &m.tasks(&lost)[0].id).trello_card, None);

    // With no project reachable at all, the round fails instead of calling itself up to date.
    config::update_settings(m.root(), |s| {
        s.projects.remove(&p);
    })
    .unwrap();
    let error = m.try_run(&trello).unwrap_err().to_string();
    assert!(error.starts_with("Lost: "), "{error}");
}

#[test]
fn a_failure_on_one_card_is_a_note_and_the_others_carry_on() {
    let (trello, m, p) = setup();
    m.task(&p, "One", "");
    trello.w().fail_writes = Some(Fail::Message);
    let report = m.run(&trello);
    assert_eq!(report.cards_created, 0);
    assert!(report.notes.iter().any(|n| n.contains("One") && n.contains("refused")), "{:?}", report.notes);
    trello.w().fail_writes = None;
    assert_eq!(m.run(&trello).cards_created, 1, "tried again next time");

    // Authorisation failures end the round: they would hit every card.
    m.task(&p, "Two", "");
    trello.w().fail_writes = Some(Fail::Unauthorized);
    assert!(matches!(m.try_run(&trello), Err(TrelloError::Unauthorized)));
}

#[test]
fn a_person_who_is_not_on_the_board_gets_nothing_synced_and_a_note() {
    let trello = FakeTrello::standard();
    let m = Machine::new();
    let p = m.project("Acme");
    m.link(&bob(), &p, "b2", "Other"); // Bob is not a member of "Other"
    m.task(&p, "One", "");
    let report = m.run(&trello);
    assert_eq!(report.cards_created, 0);
    assert!(report.notes.iter().any(|n| n.contains("Bob Jones") && n.contains("not a member")), "{:?}", report.notes);
    assert!(trello.written().is_empty());
}

// ---- setting up ---------------------------------------------------------------------------------------

#[test]
fn testing_the_connection_names_the_account() {
    let trello = FakeTrello::standard();
    assert_eq!(trello::test_connection(&trello).unwrap(), "Connected to Trello as Ana Smith (@ana).");
}

#[test]
fn the_key_and_token_are_checked_and_kept_in_the_env_file_only() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let message = |e: Error| e.to_string();

    assert_eq!(message(trello::save_credentials(root, None, Some("tok")).unwrap_err()), "Enter the Trello API key");
    assert_eq!(message(trello::save_credentials(root, Some("key"), Some("  ")).unwrap_err()), "Enter the Trello token");
    for bad in ["has space", "quo\"te", "back\\slash", "com,ma", "ünïcode"] {
        assert!(trello::save_credentials(root, Some(bad), Some("tok")).unwrap_err().to_string().contains("no spaces"), "{bad}");
        assert!(trello::save_credentials(root, Some("key"), Some(bad)).is_err(), "{bad}");
    }
    assert!(!config::read_credentials(root).complete(), "nothing was saved by the failures");

    trello::save_credentials(root, Some("  KEY1234567890KEY  "), Some("TOKEN1234567890TOKEN")).unwrap();
    let saved = config::read_credentials(root);
    assert_eq!((saved.api_key.as_str(), saved.token.as_str()), ("KEY1234567890KEY", "TOKEN1234567890TOKEN"));
    let env = fs::read_to_string(crate::settings::secrets_path(root)).unwrap();
    assert!(env.lines().any(|l| l == "TRELLO_API_KEY=KEY1234567890KEY") && env.lines().any(|l| l == "TRELLO_TOKEN=TOKEN1234567890TOKEN"), "{env}");

    // A blank field keeps what is saved; other lines of the file are left alone.
    crate::settings::write_secret(root, "GITHUB_TOKEN", "ghp_other").unwrap();
    trello::save_credentials(root, None, Some("NEWTOKEN1234567890")).unwrap();
    let saved = config::read_credentials(root);
    assert_eq!((saved.api_key.as_str(), saved.token.as_str()), ("KEY1234567890KEY", "NEWTOKEN1234567890"));
    assert_eq!(crate::settings::read_secret(root, "GITHUB_TOKEN").as_deref(), Some("ghp_other"));

    // Removing them leaves the person and the project links.
    config::update_settings(root, |s| s.member = Some(ana())).unwrap();
    trello::disconnect(root).unwrap();
    assert!(!config::read_credentials(root).complete());
    assert!(config::read_settings(root).member.is_some());
    assert_eq!(crate::settings::read_secret(root, "GITHUB_TOKEN").as_deref(), Some("ghp_other"));
}

#[test]
fn the_person_is_chosen_from_a_boards_members() {
    let trello = FakeTrello::standard();
    let dir = tempdir().unwrap();
    let root = dir.path();

    assert!(trello::save_member(root, &trello, "b3", "m-bob").unwrap_err().to_string().contains("isn't one of your open Trello boards"));
    assert!(trello::save_member(root, &trello, "b2", "m-bob").unwrap_err().to_string().contains("isn't a member of the board"));
    assert!(config::read_settings(root).member.is_none());

    let chosen = trello::save_member(root, &trello, "b1", "m-bob").unwrap();
    assert_eq!(chosen.username, "bob");
    assert_eq!(config::read_settings(root).member.unwrap().id, "m-bob");
    trello::save_member(root, &trello, "b1", "m-ana").unwrap();
    assert_eq!(config::read_settings(root).member.unwrap().full_name, "Ana Smith", "one person at a time");
}

#[test]
fn a_project_is_linked_to_a_board_the_person_is_on_and_a_board_serves_one_project() {
    let trello = FakeTrello::standard();
    let dir = tempdir().unwrap();
    let root = dir.path();

    // A person has to be chosen first.
    assert!(trello::link_project(root, &trello, "acme", Some("b1")).unwrap_err().to_string().contains("Choose whose cards"));
    trello::save_member(root, &trello, "b1", "m-ana").unwrap();

    assert!(trello::link_project(root, &trello, "acme", Some("nope")).unwrap_err().to_string().contains("isn't one of your open Trello boards"));
    trello::link_project(root, &trello, "acme", Some("b1")).unwrap();
    assert_eq!(config::read_settings(root).projects["acme"].board_name, "Work");

    // One board, one project.
    let e = trello::link_project(root, &trello, "other", Some("b1")).unwrap_err().to_string();
    assert!(e.contains("already linked to the project acme"), "{e}");
    trello::link_project(root, &trello, "other", Some("b2")).unwrap();

    // The person must be on the board: switch the chosen person to someone who isn't on "Other" (and free the board).
    trello::link_project(root, &trello, "other", None).unwrap();
    trello::save_member(root, &trello, "b1", "m-bob").unwrap();
    let e = trello::link_project(root, &trello, "third", Some("b2")).unwrap_err().to_string();
    assert!(e.contains("Bob Jones") && e.contains("isn't a member of the board Other"), "{e}");

    // Choosing lists for columns needs a link; changing the board forgets them; unlinking forgets the project.
    assert!(trello::set_column_list(root, "ghost", "doing", "l4").is_err());
    trello::set_column_list(root, "acme", "doing", "l4").unwrap();
    trello::set_column_list(root, "acme", "backlog", "none").unwrap();
    trello::set_column_list(root, "acme", "done", "auto").unwrap();
    let link = config::read_settings(root).projects["acme"].clone();
    assert_eq!(link.columns, BTreeMap::from([("doing".to_string(), "l4".to_string()), ("backlog".to_string(), String::new())]));
    trello::link_project(root, &trello, "acme", Some("b1")).unwrap();
    assert_eq!(config::read_settings(root).projects["acme"].columns.len(), 2, "the same board: choices stay");
    config::update_state(root, |s| {
        s.items.insert(
            "card".into(),
            config::Tracked { project: "acme".into(), kind: Kind::Task, id: "t".into(), base: Content::default() },
        );
    })
    .unwrap();
    trello::link_project(root, &trello, "acme", None).unwrap();
    assert!(!config::read_settings(root).projects.contains_key("acme"));
    assert!(config::read_state(root).items.is_empty(), "what was remembered about the project's cards is dropped");
}

#[test]
fn the_sync_is_on_only_when_there_is_something_to_sync() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    assert!(!config::is_configured(root));
    trello::save_credentials(root, Some("KEY1234567890KEY"), Some("TOKEN1234567890TOKEN")).unwrap();
    assert!(!config::is_configured(root), "no person yet");
    config::update_settings(root, |s| s.member = Some(ana())).unwrap();
    assert!(!config::is_configured(root), "no project linked");
    config::update_settings(root, |s| {
        s.projects.insert("acme".into(), ProjectLink { board: "b1".into(), board_name: "Work".into(), columns: BTreeMap::new() });
    })
    .unwrap();
    assert!(config::is_configured(root));
    trello::disconnect(root).unwrap();
    assert!(!config::is_configured(root), "the key and token are gone");
}

#[test]
fn the_screen_is_told_which_list_each_column_uses() {
    let columns: Vec<crate::models::Column> = [("backlog", "Backlog"), ("doing", "Doing"), ("done", "Done"), ("review", "Review")]
        .iter()
        .map(|(id, name)| crate::models::Column { id: id.to_string(), name: name.to_string() })
        .collect();
    let lists = vec![
        List { id: "l1".into(), name: "Backlog".into() },
        List { id: "l2".into(), name: "doing".into() },
        List { id: "l4".into(), name: "Ideas".into() },
    ];
    let link = ProjectLink {
        board: "b1".into(),
        board_name: "Work".into(),
        columns: BTreeMap::from([("review".to_string(), "l4".to_string()), ("done".to_string(), String::new())]),
    };
    let view = trello::mapping_view(&columns, &lists, &link);
    let row = |id: &str| view.columns.iter().find(|c| c.id == id).unwrap();
    assert_eq!((row("backlog").choice.as_str(), row("backlog").list_name.as_deref()), ("auto", Some("Backlog")));
    assert_eq!((row("doing").choice.as_str(), row("doing").list_name.as_deref()), ("auto", Some("doing")));
    assert_eq!((row("done").choice.as_str(), row("done").list_id.clone()), ("none", None));
    assert_eq!((row("review").choice.as_str(), row("review").list_name.as_deref()), ("l4", Some("Ideas")));
    assert!(view.unused_lists.is_empty());
    let unused = trello::mapping_view(&columns[..1], &lists, &ProjectLink::default());
    assert_eq!(unused.unused_lists, ["doing", "Ideas"]);
}

// ---- secrets ------------------------------------------------------------------------------------------

#[test]
fn the_key_and_token_never_appear_anywhere_but_the_env_file() {
    const KEY: &str = "KEYSECRET1234567890abcdef";
    const TOKEN: &str = "TOKENSECRET1234567890abcdef";
    let (trello, m, p) = setup();
    trello::save_credentials(m.root(), Some(KEY), Some(TOKEN)).unwrap();
    m.task(&p, "A task", "");
    trello.add_card("b1", "l1", "A card", "", &["m-ana"], &["Task"]);
    m.run(&trello);
    trello.w().fail_cards = Some(Fail::Unauthorized);
    let error = m.try_run(&trello).unwrap_err();
    trello.w().fail_cards = None;
    m.run(&trello);

    // Nothing the screen is given, said in a status, or written in an error has them.
    let snapshot = m.ledger.snapshot().unwrap();
    let status = trello::TrelloStatus { last_error: Some(error.to_string()), ..Default::default() };
    let view = trello::view(m.root(), &snapshot, status);
    let shown = serde_json::to_string(&view).unwrap();
    assert!(view.has_key && view.has_token);
    assert!(!shown.contains(KEY) && !shown.contains(TOKEN), "{shown}");
    assert_eq!((view.key_hint.as_str(), view.token_hint.as_str()), ("…cdef", "…cdef"), "only how they end");
    assert!(!error.to_string().contains(KEY) && !error.to_string().contains(TOKEN));

    // Not in the settings, the bookkeeping, the vault or the conflicts: only in .ledger/.env.
    fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in fs::read_dir(dir).unwrap().flatten() {
            if entry.file_type().unwrap().is_dir() {
                walk(&entry.path(), out);
            } else {
                out.push(entry.path());
            }
        }
    }
    let mut files = vec![];
    walk(m.root(), &mut files);
    let mut holders = vec![];
    for file in files {
        let text = String::from_utf8_lossy(&fs::read(&file).unwrap()).into_owned();
        if text.contains(KEY) || text.contains(TOKEN) {
            holders.push(file.file_name().unwrap().to_string_lossy().into_owned());
        }
    }
    assert_eq!(holders, [".env"]);
    assert!(fs::read_to_string(config::settings_path(m.root())).unwrap().contains("\"member\""));
    assert!(m.root().join(".ledger/trello-sync.json").exists());
    let ignore = fs::read_to_string(m.root().join(".gitignore")).unwrap();
    assert!(ignore.lines().any(|l| l == ".ledger/"), "{ignore}");
}

// ---- the client: rate limits, retries, credentials ----------------------------------------------------

struct Scripted {
    replies: Mutex<VecDeque<std::result::Result<Reply, String>>>,
    seen: Mutex<Vec<Request>>,
}

struct Shared(Arc<Scripted>);

impl Wire for Shared {
    fn send(&self, request: &Request) -> std::result::Result<Reply, String> {
        self.0.seen.lock().unwrap().push(request.clone());
        self.0.replies.lock().unwrap().pop_front().unwrap_or_else(|| Err("the script ran out".into()))
    }
}

fn reply(status: u16, body: &str) -> std::result::Result<Reply, String> {
    Ok(Reply { status, retry_after: None, body: body.into() })
}

fn limited(seconds: Option<u64>) -> std::result::Result<Reply, String> {
    Ok(Reply { status: 429, retry_after: seconds.map(Duration::from_secs), body: "API_TOKEN_LIMIT_EXCEEDED".into() })
}

/// A client over scripted replies; its pauses are recorded instead of taken.
fn scripted(replies: Vec<std::result::Result<Reply, String>>) -> (HttpTrello<Shared>, Arc<Scripted>, Arc<Mutex<Vec<Duration>>>) {
    let script = Arc::new(Scripted { replies: Mutex::new(replies.into()), seen: Mutex::new(vec![]) });
    let pauses = Arc::new(Mutex::new(vec![]));
    let record = pauses.clone();
    let client = HttpTrello::with_wire(Shared(script.clone()), Arc::new(move |d| record.lock().unwrap().push(d)));
    (client, script, pauses)
}

const ME: &str = r#"{"id":"m1","username":"ana","fullName":"Ana Smith"}"#;

fn long_pauses(pauses: &Mutex<Vec<Duration>>) -> Vec<Duration> {
    pauses.lock().unwrap().iter().copied().filter(|d| *d >= Duration::from_secs(1)).collect()
}

#[test]
fn a_rate_limited_request_waits_as_long_as_trello_says_and_is_tried_again() {
    let (client, script, pauses) = scripted(vec![limited(Some(3)), limited(Some(2)), reply(200, ME)]);
    assert_eq!(client.me().unwrap().username, "ana");
    assert_eq!(script.seen.lock().unwrap().len(), 3);
    assert_eq!(long_pauses(&pauses), [Duration::from_secs(3), Duration::from_secs(2)]);
}

#[test]
fn without_a_retry_after_the_wait_grows_and_in_the_end_the_caller_is_told_to_slow_down() {
    let (client, script, pauses) = scripted((0..5).map(|_| limited(None)).collect());
    let error = client.me().unwrap_err();
    assert!(matches!(error, TrelloError::RateLimited { .. }), "{error}");
    assert!(error.to_string().contains("rate limiting"));
    assert_eq!(script.seen.lock().unwrap().len(), 5, "it gave up after a few tries, not never");
    assert_eq!(long_pauses(&pauses), [1, 2, 4, 8].map(Duration::from_secs));
}

#[test]
fn a_wait_longer_than_a_minute_is_passed_on_instead_of_blocking_for_it() {
    let (client, script, pauses) = scripted(vec![limited(Some(300)), reply(200, ME)]);
    match client.me().unwrap_err() {
        TrelloError::RateLimited { retry_after } => assert_eq!(retry_after, Duration::from_secs(300)),
        other => panic!("{other}"),
    }
    assert_eq!(script.seen.lock().unwrap().len(), 1);
    assert!(long_pauses(&pauses).is_empty());
}

#[test]
fn server_errors_and_dropped_connections_are_retried_with_a_growing_pause() {
    let (client, _, pauses) = scripted(vec![reply(503, "unavailable"), Err("Could not reach Trello: connection reset".into()), reply(502, ""), reply(200, ME)]);
    assert_eq!(client.me().unwrap().id, "m1");
    assert_eq!(long_pauses(&pauses), [1, 2, 4].map(Duration::from_secs));

    let (client, script, _) = scripted((0..5).map(|_| reply(500, "oops")).collect());
    let error = client.me().unwrap_err().to_string();
    assert!(error.contains("500") && error.contains("try again"), "{error}");
    assert_eq!(script.seen.lock().unwrap().len(), 5);

    let (client, _, _) = scripted((0..5).map(|_| Err("Could not reach Trello: offline".to_string())).collect());
    assert_eq!(client.me().unwrap_err().to_string(), "Could not reach Trello: offline");
}

#[test]
fn a_refused_token_is_not_retried_and_other_refusals_say_what_to_do() {
    let (client, script, _) = scripted(vec![reply(401, "invalid token"), reply(200, ME)]);
    assert!(matches!(client.me().unwrap_err(), TrelloError::Unauthorized));
    assert_eq!(script.seen.lock().unwrap().len(), 1);
    assert!(TrelloError::Unauthorized.to_string().contains("rejected the API key or token"));

    let (client, _, _) = scripted(vec![reply(403, "x")]);
    assert!(client.me().unwrap_err().to_string().contains("read and write"));
    let (client, _, _) = scripted(vec![reply(400, "invalid id")]);
    assert!(client.boards().unwrap_err().to_string().contains("invalid id"));
    let (client, _, _) = scripted(vec![reply(404, "not found")]);
    assert!(client.lists("b1").unwrap_err().to_string().contains("can't find"));
    let (client, _, _) = scripted(vec![reply(200, "not json")]);
    assert!(client.me().unwrap_err().to_string().contains("not what was expected"));
}

#[test]
fn a_card_that_is_gone_is_none_and_one_that_is_there_is_read() {
    let card = r#"{"id":"c1","idShort":7,"name":"N","desc":"D","idList":"l1","idBoard":"b1","idLabels":["x"],"idMembers":["m1"],"closed":true,"dateLastActivity":"2026-01-02T03:04:05.000Z"}"#;
    let (client, script, _) = scripted(vec![reply(404, "The requested resource was not found."), reply(200, card), reply(400, "invalid id")]);
    assert_eq!(client.card("c0").unwrap(), None);
    let found = client.card("c1").unwrap().unwrap();
    assert_eq!((found.id_short, found.name.as_str(), found.id_list.as_str(), found.id_board.as_str(), found.closed), (7, "N", "l1", "b1", true));
    assert_eq!(found.id_labels, ["x"]);
    assert_eq!(found.id_members, ["m1"]);
    assert!(client.card("bad").is_err());
    let seen = script.seen.lock().unwrap();
    assert_eq!((seen[0].method, seen[0].path.as_str()), ("GET", "/cards/c0"));
}

#[test]
fn requests_are_paced_and_carry_no_credentials() {
    let board_cards = r#"[{"id":"c1","name":"Open","closed":false},{"id":"c2","name":"Archived","closed":true}]"#;
    let (client, script, pauses) = scripted(vec![reply(200, board_cards), reply(200, ME), reply(200, r#"{"id":"c9","name":"New"}"#), reply(200, r#"{"id":"c9","name":"Changed"}"#)]);

    // Only open cards, even if Trello includes others.
    let cards = client.cards("b1").unwrap();
    assert_eq!(cards.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["c1"]);
    client.me().unwrap();
    client
        .create_card(&NewCard { list: "l1".into(), name: "New".into(), desc: "d".into(), member: "m-ana".into(), labels: vec!["x".into(), "y".into()] })
        .unwrap();
    client.update_card("c9", &CardUpdate { name: Some("Changed".into()), ..Default::default() }).unwrap();

    // Never two requests in quick succession without a pause between them.
    assert!(pauses.lock().unwrap().iter().filter(|d| **d <= MIN_GAP && !d.is_zero()).count() >= 3, "{:?}", pauses.lock().unwrap());

    let seen = script.seen.lock().unwrap();
    assert_eq!((seen[0].method, seen[0].path.as_str()), ("GET", "/boards/b1/cards/open"));
    let create = &seen[2];
    assert_eq!((create.method, create.path.as_str()), ("POST", "/cards"));
    let body = create.body.as_ref().unwrap();
    assert_eq!((body["idList"].as_str(), body["idMembers"].as_str(), body["idLabels"].as_str(), body["name"].as_str()), (Some("l1"), Some("m-ana"), Some("x,y"), Some("New")));
    // A change sends only what changed.
    assert_eq!(seen[3].body.as_ref().unwrap(), &serde_json::json!({ "name": "Changed" }));
    // A request has no place for credentials: they are added to the headers by the wire and nowhere else.
    for request in seen.iter() {
        let text = format!("{request:?}").to_lowercase();
        assert!(!text.contains("oauth") && !text.contains("token=") && !text.contains("key="), "{text}");
    }
}

#[test]
fn credentials_go_in_an_authorization_header_and_a_bad_one_is_refused_without_repeating_it() {
    assert_eq!(auth_header("K", "T"), "OAuth oauth_consumer_key=\"K\", oauth_token=\"T\"");
    assert!(valid_credential("abc123_-.") && !valid_credential("") && !valid_credential("a b") && !valid_credential("a\"b") && !valid_credential("a\nb"));
    let error = match ReqwestWire::new("bad key\nSECRETPART", "tok") {
        Ok(_) => panic!("a key with a newline can't be sent"),
        Err(e) => e.to_string(),
    };
    assert!(!error.contains("SECRETPART"), "{error}");
    assert!(ReqwestWire::new("KEY123", "TOKEN123").is_ok());
}

// ---- the vault keeps the card id ----------------------------------------------------------------------

#[test]
fn the_card_id_and_the_reason_survive_being_written_and_read_back() {
    let m = Machine::new();
    let p = m.project("Acme");
    let task = m.task(&p, "A task", "body");
    m.story(&p, story_draft("MM01", "A story"));
    m.edit(|c| {
        trello_ledger::set_link(c, &p, Kind::Task, &task, Some("card-1"), None).unwrap();
        trello_ledger::set_link(c, &p, Kind::Story, "MM01", Some("card-2"), Some("The card was archived on Trello.")).unwrap();
    });

    let text = m.task_file(&p, &task);
    assert!(text.contains("trelloCard: card-1") && !text.contains("trelloUnlinked"), "{text}");
    let story = m.file(&format!("projects/{p}/userstories/MM01.md"));
    assert!(story.contains("trelloCard: card-2") && story.contains("trelloUnlinked: The card was archived on Trello."), "{story}");

    // Reading the files in again gives the same database, and writes nothing.
    let before = crate::vault::export(&m.core.lock().unwrap().conn).unwrap();
    m.reload();
    assert_eq!(crate::vault::export(&m.core.lock().unwrap().conn).unwrap(), before);
    assert_eq!(m.get_task(&p, &task).trello_card.as_deref(), Some("card-1"));
    let story = m.get_story(&p, "MM01");
    assert_eq!((story.trello_card.as_deref(), story.trello_unlinked.as_deref()), (Some("card-2"), Some("The card was archived on Trello.")));
    let mut guard = m.core.lock().unwrap();
    let core = &mut *guard;
    assert_eq!(core.vault.persist(&core.conn).unwrap(), 0);
}

#[test]
fn files_made_before_the_card_id_existed_load_unchanged() {
    let m = Machine::new();
    let p = m.project("Acme");
    let task = m.task(&p, "Old task", "body");
    m.story(&p, story_draft("MM01", "Old story"));
    let task_text = m.task_file(&p, &task);
    let story_path = format!("projects/{p}/userstories/MM01.md");
    let story_text = m.file(&story_path);
    assert!(!task_text.contains("trello") && !story_text.contains("trello"), "no card id: no keys, files as short as before");

    m.reload();
    assert_eq!(m.task_file(&p, &task), task_text);
    assert_eq!(m.file(&story_path), story_text);
    assert_eq!(m.get_task(&p, &task).trello_card, None);
    let mut guard = m.core.lock().unwrap();
    let core = &mut *guard;
    assert_eq!(core.vault.persist(&core.conn).unwrap(), 0, "reading old files back rewrites none of them");
    drop(guard);

    // A file written by hand with the keys is read, an empty value counts as none.
    fs::write(
        m.root().join(format!("projects/{p}/tasks/by-hand.md")),
        "---\ntitle: By hand\ncolumnId: doing\ntrelloCard: abc123\ntrelloUnlinked: ''\n---\n\nWritten in Obsidian.\n",
    )
    .unwrap();
    m.reload();
    let by_hand = m.get_task(&p, "by-hand");
    assert_eq!((by_hand.trello_card.as_deref(), by_hand.trello_unlinked.as_deref(), by_hand.column_id.as_str()), (Some("abc123"), None, "doing"));
    let parsed = frontmatter::parse(&m.task_file(&p, "by-hand")).unwrap();
    assert_eq!(parsed.meta.text("trelloCard").as_deref(), Some("abc123"));
}

#[test]
fn editing_through_the_apps_own_stores_keeps_the_card_id() {
    let (trello, m, p) = setup();
    let id = m.task(&p, "A task", "");
    m.story(&p, story_draft("MM01", "A story"));
    m.run(&trello);
    let (task_card, story_card) = (m.get_task(&p, &id).trello_card.unwrap(), m.get_story(&p, "MM01").trello_card.unwrap());

    m.update_task(&p, &id, TaskUpdate { title: Some("Renamed".into()), body: Some("body".into()), ..TaskUpdate::default() });
    m.move_task(&p, &id, "doing");
    m.edit(|c| stories::update(c, &p, "MM01", crate::models::StoryUpdate { summary: Some("Renamed".into()), criteria: Some(vec![]), ..Default::default() }).unwrap());
    m.edit(|c| {
        stories::add_thread(c, &p, "MM01", &crate::threads::create(c, &p, "T").unwrap().slug).unwrap();
    });
    assert_eq!(m.get_task(&p, &id).trello_card.as_deref(), Some(task_card.as_str()));
    assert_eq!(m.get_story(&p, "MM01").trello_card.as_deref(), Some(story_card.as_str()));
    m.reload();
    assert_eq!(m.get_task(&p, &id).trello_card.as_deref(), Some(task_card.as_str()));
    assert_eq!(m.get_story(&p, "MM01").trello_card.as_deref(), Some(story_card.as_str()));
}

// ---- the two syncs, side by side ----------------------------------------------------------------------

/// A branch on "GitHub" as a list of snapshots.
#[derive(Default)]
struct MemRemote {
    commits: Mutex<Vec<BTreeMap<String, Vec<u8>>>>,
    pushes: AtomicUsize,
}

impl Remote for MemRemote {
    fn head(&self) -> Result<Option<Head>> {
        let commits = self.commits.lock().unwrap();
        Ok(commits.len().checked_sub(1).map(|i| Head { commit: format!("commit-{i}"), tree: format!("tree-{i}") }))
    }

    fn tree(&self, head: &Head) -> Result<BTreeMap<String, String>> {
        let i: usize = head.commit.strip_prefix("commit-").unwrap().parse().unwrap();
        Ok(self.commits.lock().unwrap()[i].iter().map(|(p, b)| (p.clone(), sync::blob_sha(b))).collect())
    }

    fn blob(&self, _path: &str, sha: &str) -> Result<Vec<u8>> {
        let commits = self.commits.lock().unwrap();
        commits.iter().flat_map(|files| files.values()).find(|b| sync::blob_sha(b) == sha).cloned().ok_or_else(|| Error::sync("no such blob"))
    }

    fn push(&self, parent: Option<&Head>, changes: &[GitChange], _message: &str) -> Result<Head> {
        let mut commits = self.commits.lock().unwrap();
        let current = commits.len().checked_sub(1);
        let parent_index = parent.map(|h| h.commit.strip_prefix("commit-").unwrap().parse::<usize>().unwrap());
        if parent_index != current {
            return Err(Error::Rejected);
        }
        let mut files = commits.last().cloned().unwrap_or_default();
        for change in changes {
            match change {
                GitChange::Put { path, bytes } => files.insert(path.clone(), bytes.clone()),
                GitChange::Delete { path } => files.remove(path),
            };
        }
        commits.push(files);
        self.pushes.fetch_add(1, Ordering::SeqCst);
        let i = commits.len() - 1;
        Ok(Head { commit: format!("commit-{i}"), tree: format!("tree-{i}") })
    }
}

/// What the app's GitHub syncer does with a pull: write the files, then load them in again.
struct ReloadingApplier(Arc<Mutex<Core>>);

impl Applier for ReloadingApplier {
    fn apply(&self, ops: Vec<LocalOp>) -> Result<Vec<String>> {
        let mut core = self.0.lock().unwrap();
        let root = core.vault.root().to_path_buf();
        let (skipped, wrote) = sync::apply_to_disk(&root, ops)?;
        if wrote {
            core.reload()?;
        }
        Ok(skipped)
    }
}

impl Machine {
    fn github(&self, remote: &MemRemote) -> sync::Outcome {
        sync::run(self.root(), "me/notes", "main", remote, &ReloadingApplier(self.core.clone())).unwrap()
    }

    fn managed_files(&self) -> BTreeMap<String, String> {
        crate::vault::export(&self.core.lock().unwrap().conn).unwrap()
    }
}

#[test]
fn the_trello_sync_and_the_github_sync_settle_instead_of_echoing_each_other() {
    let trello = FakeTrello::standard();
    let remote = MemRemote::default();
    let a = Machine::new();
    let b = Machine::new();
    let p = a.project("Acme");
    a.link(&ana(), &p, "b1", "Work");

    // A card comes in on A; GitHub carries the item to B; B's Trello sync finds nothing to do.
    let card = trello.add_card("b1", "l2", "From Trello", "details", &["m-ana"], &["Task"]);
    a.run(&trello);
    a.github(&remote);
    b.github(&remote);
    b.link(&ana(), &p, "b1", "Work");
    assert_eq!(b.tasks(&p).len(), 1);
    let report = b.run(&trello);
    assert_eq!((report.cards_created, report.cards_updated, report.items_created, report.vault_writes), (0, 0, 0, 0));
    assert_eq!(b.github(&remote).pushed, 0, "nothing for GitHub to take from B");

    // A task made on B goes to Trello, its card id goes to A through GitHub, and A's Trello sync makes no second card.
    let made = b.task(&p, "Made on B", "body");
    assert_eq!(b.run(&trello).cards_created, 1);
    b.github(&remote);
    a.github(&remote);
    assert_eq!(a.run(&trello).cards_created, 0);
    assert_eq!(trello.created().len(), 1);

    // An edit on Trello comes in on A, and travels on through GitHub to B.
    trello.edit(&card, |c| c.name = "Renamed on Trello".into());
    assert_eq!(a.run(&trello).items_updated, 1);
    a.github(&remote);
    b.github(&remote);
    assert_eq!(b.get_task(&p, &format!("trello-{card}")).title, "Renamed on Trello");

    // Then everything is quiet: more rounds of both syncs, in any order, write nothing anywhere.
    let pushes = remote.pushes.load(Ordering::SeqCst);
    let (created, written) = (trello.created().len(), trello.written().len());
    for _ in 0..3 {
        for m in [&a, &b] {
            let report = m.run(&trello);
            assert_eq!((report.vault_writes, report.cards_created, report.cards_updated, report.conflicts), (0, 0, 0, 0), "{:?}", report.notes);
            let outcome = m.github(&remote);
            assert_eq!((outcome.pushed, outcome.pulled), (0, 0));
        }
    }
    assert_eq!(remote.pushes.load(Ordering::SeqCst), pushes);
    assert_eq!((trello.created().len(), trello.written().len()), (created, written));
    assert_eq!(a.managed_files(), b.managed_files(), "both machines hold the same vault");
    assert_eq!(a.get_task(&p, &made).trello_card, b.get_task(&p, &made).trello_card);
    assert!(a.conflict_files().is_empty() && b.conflict_files().is_empty());
}

// ---- the worker ---------------------------------------------------------------------------------------

fn quick() -> Timing {
    Timing {
        debounce: Duration::from_millis(150),
        max_debounce: Duration::from_millis(900),
        poll: Duration::from_secs(60),
        retry: Duration::from_millis(80),
        rate_limited: Duration::from_millis(400),
    }
}

struct Worker {
    syncer: trello::TrelloSyncer,
    events: Arc<Mutex<Vec<String>>>,
    vault_changes: Arc<AtomicUsize>,
    connects: Arc<AtomicUsize>,
}

fn start_worker(m: &Machine, trello: &Arc<FakeTrello>, timing: Timing) -> Worker {
    let events = Arc::new(Mutex::new(vec![]));
    let vault_changes = Arc::new(AtomicUsize::new(0));
    let (log, changes) = (events.clone(), vault_changes.clone());
    let emit: Emit = Arc::new(move |event| match event {
        TrelloEvent::Status(s) => log.lock().unwrap().push(s.state),
        TrelloEvent::VaultChanged => {
            changes.fetch_add(1, Ordering::SeqCst);
        }
    });
    let connects = Arc::new(AtomicUsize::new(0));
    let (counted, fake) = (connects.clone(), trello.clone());
    let connect: Connect = Arc::new(move |_| {
        counted.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(fake.clone()))
    });
    let syncer = trello::spawn(m.core.clone(), emit, m.ledger.changed.clone(), connect, timing);
    Worker { syncer, events, vault_changes, connects }
}

fn wait_for(what: &str, condition: impl Fn() -> bool) {
    let start = Instant::now();
    while !condition() {
        assert!(start.elapsed() < Duration::from_secs(10), "gave up waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn configured_machine() -> (Arc<FakeTrello>, Machine, String) {
    let (trello, m, p) = setup();
    trello::save_credentials(m.root(), Some("KEY1234567890KEY"), Some("TOKEN1234567890TOKEN")).unwrap();
    (Arc::new(trello), m, p)
}

#[test]
fn the_worker_does_nothing_until_the_sync_is_set_up() {
    let trello = Arc::new(FakeTrello::standard());
    let m = Machine::new();
    let p = m.project("Acme");
    m.task(&p, "A task", "");
    let worker = start_worker(&m, &trello, quick());

    worker.syncer.now();
    worker.syncer.dirty();
    wait_for("the status", || !worker.events.lock().unwrap().is_empty());
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(worker.syncer.status().state, "disabled");
    assert_eq!(worker.connects.load(Ordering::SeqCst), 0, "Trello was never contacted");
    assert!(trello.w().calls.is_empty());
    assert_eq!(m.get_task(&p, &m.tasks(&p)[0].id).trello_card, None);
}

#[test]
fn the_worker_syncs_on_request_and_says_what_it_did() {
    let (trello, m, p) = configured_machine();
    m.task(&p, "Made here", "");
    trello.add_card("b1", "l2", "Made there", "", &["m-ana"], &["Task"]);
    let worker = start_worker(&m, &trello, quick());

    worker.syncer.now();
    wait_for("the first round", || worker.syncer.status().last_sync_at.is_some());
    let status = worker.syncer.status();
    assert_eq!((status.state.as_str(), status.last_error.as_deref()), ("idle", None));
    let summary = status.summary.unwrap().to_lowercase();
    assert!(summary.contains("created 1 card") && summary.contains("added 1 item"), "{summary}");
    assert_eq!(status.report.unwrap().cards_created, 1);
    assert_eq!(m.titles(&p), ["Made here", "Made there"]);
    assert!(worker.events.lock().unwrap().iter().any(|s| s == "syncing"));
    wait_for("the screens to be told to refresh", || worker.vault_changes.load(Ordering::SeqCst) == 1);
}

#[test]
fn changes_are_synced_after_a_pause_and_a_burst_of_them_is_one_round() {
    let (trello, m, p) = configured_machine();
    let id = m.task(&p, "A task", "");
    let worker = start_worker(&m, &trello, quick());
    worker.syncer.now();
    wait_for("the first round", || worker.syncer.status().last_sync_at.is_some());
    let rounds = trello.calls("cards");

    // A burst of edits and notifications, as a person typing would cause.
    for n in 1..=4 {
        m.update_task(&p, &id, TaskUpdate { title: Some(format!("Title {n}")), ..TaskUpdate::default() });
        worker.syncer.dirty();
        std::thread::sleep(Duration::from_millis(30));
    }
    wait_for("the card to follow", || trello.open_cards("b1")[0].name == "Title 4");
    wait_for("the round to end", || worker.syncer.status().state == "idle" && !worker.syncer.status().pending);
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(trello.calls("cards"), rounds + 1, "one round for the whole burst");
    assert_eq!(trello.written().len(), 2, "the card was made, then updated once");
}

#[test]
fn a_change_that_trello_doesnt_care_about_costs_no_request() {
    let (trello, m, p) = configured_machine();
    m.task(&p, "A task", "");
    let worker = start_worker(&m, &trello, quick());
    worker.syncer.now();
    wait_for("the first round", || worker.syncer.status().last_sync_at.is_some());
    // The round has linked the card, so the vault looks different now: one more round settles that.
    worker.syncer.dirty();
    wait_for("the settling round", || trello.calls("cards") == 2 && worker.syncer.status().state == "idle");
    let rounds = trello.calls("cards");

    // A post in a thread, which has nothing to do with Trello: the worker looks, and finds no reason to ask.
    m.edit(|c| {
        let t = crate::threads::create(c, &p, "Calls").unwrap().slug;
        crate::posts::add(c, &p, &t, "A note", Default::default()).unwrap();
    });
    worker.syncer.dirty();
    wait_for("the worker to look", || !worker.syncer.status().pending);
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(trello.calls("cards"), rounds, "no request");
    assert_eq!(worker.syncer.status().state, "idle");
}

#[test]
fn the_worker_polls_for_changes_made_on_trello() {
    let (trello, m, p) = configured_machine();
    let worker = start_worker(&m, &trello, Timing { poll: Duration::from_millis(250), ..quick() });
    worker.syncer.now();
    wait_for("the first round", || worker.syncer.status().last_sync_at.is_some());
    assert!(m.tasks(&p).is_empty());

    trello.add_card("b1", "l1", "Appeared on Trello", "", &["m-ana"], &["Task"]);
    wait_for("the poll to find it", || m.tasks(&p).len() == 1);
    assert_eq!(m.titles(&p), ["Appeared on Trello"]);
    wait_for("the screens to be told to refresh", || worker.vault_changes.load(Ordering::SeqCst) >= 1);
}

#[test]
fn a_rate_limit_makes_the_worker_wait_longer_than_a_plain_failure() {
    let (trello, m, p) = configured_machine();
    m.task(&p, "A task", "");
    trello.w().fail_cards = Some(Fail::RateLimited);
    let worker = start_worker(&m, &trello, quick());
    worker.syncer.now();
    wait_for("the failure", || worker.syncer.status().state == "error");
    let status = worker.syncer.status();
    assert!(status.last_error.unwrap().contains("rate limiting"));
    assert!(status.last_sync_at.is_none());

    // It tries again, but not before the pause for rate limits (400 ms here, against a retry pause of 80 ms).
    wait_for("the second try", || trello.calls("cards") >= 2);
    let times = trello.call_times("cards");
    assert!(times[1].duration_since(times[0]) >= Duration::from_millis(390), "{:?}", times[1].duration_since(times[0]));

    // Once Trello is willing again, the sync goes through and the error is gone.
    trello.w().fail_cards = None;
    wait_for("the sync to recover", || worker.syncer.status().state == "idle" && worker.syncer.status().last_sync_at.is_some());
    assert_eq!(worker.syncer.status().last_error, None);
    assert_eq!(trello.open_cards("b1").len(), 1);

    // A plain failure is retried after the short pause.
    trello.w().fail_cards = Some(Fail::Message);
    worker.syncer.now();
    wait_for("the failure", || worker.syncer.status().state == "error");
    let calls = trello.calls("cards");
    wait_for("the retry", || trello.calls("cards") > calls);
    trello.w().fail_cards = None;
}

#[test]
fn settings_changes_start_the_worker() {
    let (trello, m, p) = configured_machine();
    m.task(&p, "A task", "");
    let worker = start_worker(&m, &trello, quick());
    worker.syncer.reconfigure(m.root());
    wait_for("the sync to start", || worker.syncer.status().last_sync_at.is_some());
    assert!(worker.syncer.status().configured);

    trello::disconnect(m.root()).unwrap();
    worker.syncer.reconfigure(m.root());
    assert_eq!(worker.syncer.status().state, "disabled");
    assert!(!worker.syncer.status().configured);
}
