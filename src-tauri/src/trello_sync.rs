//! The two-way sync between the vault's tasks and user stories and the cards of one Trello person.
//!
//! * **Scope.** Only cards the selected person is a member of are ever read or written. Cards of anyone else, or of
//!   nobody, are never looked at beyond the board's card list, never imported and never changed.
//! * **Identity.** An item and its card are tied together by the card id in the item's own frontmatter
//!   (`trelloCard`), which the GitHub sync carries to other machines. A card with no item here becomes an item
//!   whose id is made from the card's (`trello-<card id>` for a task, `TR-<card number>` for a story), so two
//!   machines importing the same card make the same file instead of two. A card Ledger made itself (its description
//!   ends with Ledger's footer) is never imported: it reaches other machines through the item's `trelloCard`, so a
//!   machine that polls Trello before the item arrives doesn't make a second item, and a deleted item stays deleted.
//! * **Changes.** For every linked pair the title, the description and (for a task) the list are compared with what
//!   both sides agreed on when they were last in step (the bookkeeping in `.ledger/trello-sync.json`). A field
//!   that only one side changed goes to the other; fields are merged one by one, so a title edited on one side
//!   and a card moved on the other don't collide.
//! * **Conflicts.** When both sides changed the same field, the newer edit wins (the item's `updatedAt` against the
//!   card's latest activity; a tie goes to this machine), and the losing version is kept in
//!   `.ledger/conflicts/<time>/trello/`. Nothing is dropped silently.
//! * **Deletes never propagate.** A card that is archived, deleted, moved to another board or no longer the
//!   person's leaves its item in place, marked as no longer linked (`trelloUnlinked`, with the reason), and it
//!   stops syncing. An item deleted here leaves its card where it is, and the card is not imported again. Nothing
//!   is ever deleted on Trello.
//! * **Columns and lists** match by name (ignoring case) unless the project's link says otherwise. A task whose
//!   column has no list isn't put on Trello, and a card whose list has no column isn't imported; both are counted
//!   and reported, and a linked task or card in such a place keeps its text synced but isn't moved.
//! * **Labels.** "User story" and "Task" are created on a linked board if missing. A card with the story label is a
//!   user story, any other card a task; once linked, an item changes kind only when its card carries the other
//!   kind's label.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;

use serde::Serialize;
use serde_yaml_ng::Mapping as Meta;

use crate::error::Error;
use crate::models::Column;
use crate::trello_api::{Card, CardUpdate, List, Member, NewCard, TResult, Trello, TrelloError};
use crate::trello_card::*;
use crate::trello_config::*;
use crate::trello_ledger::*;
use crate::{frontmatter, ids};

// ---- columns and lists --------------------------------------------------------------------------------

/// Which Trello list belongs to which column of a project's board.
#[derive(Debug, Default, Clone)]
pub struct Mapping {
    by_column: HashMap<String, String>,
    by_list: HashMap<String, String>,
}

impl Mapping {
    /// A column the project's link names a list for uses that list (an empty one means "don't sync this column");
    /// every other column takes the list with the same name, ignoring case. A list belongs to one column.
    pub fn new(columns: &[Column], lists: &[List], overrides: &BTreeMap<String, String>) -> Mapping {
        let mut map = Mapping::default();
        let mut chosen: HashSet<&str> = HashSet::new();
        for column in columns {
            let Some(choice) = overrides.get(&column.id) else { continue };
            chosen.insert(column.id.as_str());
            if !choice.is_empty() && lists.iter().any(|l| l.id == *choice) && !map.by_list.contains_key(choice) {
                map.by_column.insert(column.id.clone(), choice.clone());
                map.by_list.insert(choice.clone(), column.id.clone());
            }
        }
        for column in columns.iter().filter(|c| !chosen.contains(c.id.as_str())) {
            if let Some(list) = lists.iter().find(|l| !map.by_list.contains_key(&l.id) && same_name(&l.name, &column.name)) {
                map.by_column.insert(column.id.clone(), list.id.clone());
                map.by_list.insert(list.id.clone(), column.id.clone());
            }
        }
        map
    }

    pub fn list_for(&self, column: &str) -> Option<&str> {
        self.by_column.get(column).map(String::as_str)
    }

    pub fn column_for(&self, list: &str) -> Option<&str> {
        self.by_list.get(list).map(String::as_str)
    }
}

// ---- what a round did ---------------------------------------------------------------------------------

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub cards_created: usize,
    pub cards_updated: usize,
    pub items_created: usize,
    pub items_updated: usize,
    pub converted: usize,
    /// Items that just stopped syncing because their card is gone or no longer the person's.
    pub detached: usize,
    pub conflicts: usize,
    /// Tasks without a list and cards without a column: left out, and said so.
    pub unmatched: usize,
    /// Changes held back because the item was edited while the round ran; the next round does them.
    pub skipped: usize,
    /// Items that are not syncing (now or earlier) because their card is gone.
    pub unlinked: usize,
    /// How many changes were written into the vault.
    pub vault_writes: usize,
    pub notes: Vec<String>,
}

impl Report {
    pub fn vault_changed(&self) -> bool {
        self.vault_writes > 0
    }

    pub fn summary(&self) -> String {
        let plural = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let mut parts = vec![];
        if self.cards_created > 0 {
            parts.push(format!("created {}", plural(self.cards_created, "card", "cards")));
        }
        if self.cards_updated > 0 {
            parts.push(format!("updated {}", plural(self.cards_updated, "card", "cards")));
        }
        if self.items_created > 0 {
            parts.push(format!("added {} from Trello", plural(self.items_created, "item", "items")));
        }
        if self.items_updated > 0 {
            parts.push(format!("updated {} from Trello", plural(self.items_updated, "item", "items")));
        }
        if self.converted > 0 {
            parts.push(format!("changed the kind of {}", plural(self.converted, "item", "items")));
        }
        if self.detached > 0 {
            parts.push(format!("stopped syncing {}", plural(self.detached, "item", "items")));
        }
        if self.conflicts > 0 {
            parts.push(format!("{} (the older edit was saved)", plural(self.conflicts, "conflict", "conflicts")));
        }
        if self.unmatched > 0 {
            parts.push(format!("{} left out (no matching column or list)", self.unmatched));
        }
        if self.skipped > 0 {
            parts.push(format!("{} to do next time", self.skipped));
        }
        if parts.is_empty() { "Up to date".to_string() } else { capitalize(&parts.join(", ")) }
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map_or_else(String::new, |c| c.to_uppercase().collect::<String>() + chars.as_str())
}

// ---- the round ----------------------------------------------------------------------------------------

/// Something that changes only when a Trello sync would have something to do: the part of the snapshot, and of the
/// settings, that a round looks at. The background worker skips a round that follows an edit of anything else.
pub fn digest(snapshot: &Snapshot, settings: &TrelloSettings) -> u64 {
    let mut hasher = DefaultHasher::new();
    settings.member.as_ref().map(|m| &m.id).hash(&mut hasher);
    for data in &snapshot.projects {
        let Some(link) = settings.projects.get(&data.slug) else { continue };
        (&data.slug, &link.board, &link.columns, &data.backlog).hash(&mut hasher);
        for column in &data.columns {
            (&column.id, &column.name).hash(&mut hasher);
        }
        for t in &data.tasks {
            (&t.id, &t.title, &t.body, &t.column_id, &t.trello_card, &t.trello_unlinked, &t.updated_at).hash(&mut hasher);
        }
        for s in &data.stories {
            (&s.id, &s.summary, &s.role, &s.want, &s.reason, &s.notes, &s.trello_card, &s.trello_unlinked, &s.updated_at).hash(&mut hasher);
            for c in &s.criteria {
                (&c.text, c.done).hash(&mut hasher);
            }
        }
    }
    hasher.finish()
}

/// One round: reconcile every linked project with its board. `snapshot` is the vault as it was read for this round.
/// Progress is kept even when the round stops early (the rate limit, a rejected token).
pub fn run(root: &Path, trello: &dyn Trello, ledger: &dyn Ledger, settings: &TrelloSettings, snapshot: &Snapshot) -> TResult<Report> {
    let Some(member) = settings.member.clone() else {
        return Err(TrelloError::message("Choose whose cards to sync first."));
    };
    let mut run = Run {
        root,
        trello,
        ledger,
        member,
        state: read_state(root),
        report: Report::default(),
        stamp: ids::now().replace(':', "-"),
        claimed: HashSet::new(),
    };
    let result = run.all(settings, snapshot);
    write_state(root, &run.state)?;
    result?;
    Ok(run.report)
}

struct Run<'a> {
    root: &'a Path,
    trello: &'a dyn Trello,
    ledger: &'a dyn Ledger,
    member: Member,
    state: SyncState,
    report: Report,
    /// The folder name, under `.ledger/conflicts/`, for what this round sets aside.
    stamp: String,
    /// Every card id some item in the vault is linked to, in any project.
    claimed: HashSet<String>,
}

#[derive(Clone, Copy)]
enum Count {
    Nothing,
    ItemCreated,
    ItemUpdated,
    Converted,
    Detached,
}

/// A change to the vault, and what to remember once it is made.
struct Step {
    change: Change,
    label: String,
    track: Option<(String, Tracked)>,
    untrack: Option<String>,
    /// If the change can't be made, this card has no item: leave it alone from now on.
    orphan: Option<String>,
    count: Count,
}

impl Step {
    fn new(change: Change, label: String, count: Count) -> Step {
        Step { change, label, track: None, untrack: None, orphan: None, count }
    }
}

/// One project's board as this round sees it.
struct Board<'a> {
    id: &'a str,
    mapping: Mapping,
    labels: KindLabels,
    cards: HashMap<&'a str, &'a Card>,
}

/// An item as the vault holds it.
struct Local {
    kind: Kind,
    id: String,
    card: String,
    updated_at: String,
    content: Content,
    /// What goes in a note about the item.
    title: String,
    /// The description as the vault holds it (the body, or the story's whole text), to set aside if it loses.
    text: String,
}

enum Located {
    Open(Card),
    Gone(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fate {
    Agree,
    /// The vault's version goes to the card.
    Push,
    /// The card's version goes to the vault.
    Pull,
}

/// Three-way: what each side has now against what they last agreed on. When both changed it differently, the
/// newer wins; the flag says that was the case.
fn pick<T: PartialEq>(ledger: &T, card: &T, base: Option<&T>, ledger_newer: bool) -> (Fate, bool) {
    if ledger == card {
        (Fate::Agree, false)
    } else if base == Some(ledger) {
        (Fate::Pull, false)
    } else if base == Some(card) {
        (Fate::Push, false)
    } else if ledger_newer {
        (Fate::Push, true)
    } else {
        (Fate::Pull, true)
    }
}

#[derive(Default)]
struct Merge {
    /// What both sides hold once the merge is done: the next base.
    content: Content,
    push: CardUpdate,
    pull_title: Option<String>,
    pull_desc: Option<String>,
    pull_list: Option<String>,
    /// A field both sides had changed went the other side's way, so this side's version needs setting aside.
    ledger_lost: bool,
    trello_lost: bool,
    unmatched: bool,
}

impl Merge {
    fn pulls(&self) -> bool {
        self.pull_title.is_some() || self.pull_desc.is_some() || self.pull_list.is_some()
    }
}

fn merge(kind: Kind, l: &Content, t: &Content, b: Option<&Content>, ledger_newer: bool, mapping: &Mapping) -> Merge {
    let mut m = Merge::default();

    let (fate, conflict) = pick(&l.title, &t.title, b.map(|b| &b.title), ledger_newer);
    m.content.title = match fate {
        Fate::Agree | Fate::Push => l.title.clone(),
        Fate::Pull => t.title.clone(),
    };
    match fate {
        Fate::Push => m.push.name = Some(l.title.clone()),
        Fate::Pull => m.pull_title = Some(t.title.clone()),
        Fate::Agree => {}
    }
    m.ledger_lost |= conflict && fate == Fate::Pull;
    m.trello_lost |= conflict && fate == Fate::Push;

    let (fate, conflict) = pick(&l.desc, &t.desc, b.map(|b| &b.desc), ledger_newer);
    m.content.desc = match fate {
        Fate::Agree | Fate::Push => l.desc.clone(),
        Fate::Pull => t.desc.clone(),
    };
    match fate {
        Fate::Push => m.push.desc = Some(with_footer(&l.desc)),
        Fate::Pull => m.pull_desc = Some(t.desc.clone()),
        Fate::Agree => {}
    }
    m.ledger_lost |= conflict && fate == Fate::Pull;
    m.trello_lost |= conflict && fate == Fate::Push;

    if kind == Kind::Task {
        m.content.list = t.list.clone();
        match (&l.list, &t.list) {
            (Some(ours), Some(theirs)) => {
                let (fate, _) = pick(ours, theirs, b.and_then(|b| b.list.as_ref()), ledger_newer);
                match fate {
                    Fate::Agree => m.content.list = Some(ours.clone()),
                    Fate::Push => {
                        m.push.list = Some(ours.clone());
                        m.content.list = Some(ours.clone());
                    }
                    Fate::Pull if mapping.column_for(theirs).is_some() => {
                        m.pull_list = Some(theirs.clone());
                        m.content.list = Some(theirs.clone());
                    }
                    // The card is in a list with no column: the task stays where it is.
                    Fate::Pull => {
                        m.unmatched = true;
                        m.content.list = Some(ours.clone());
                    }
                }
            }
            (None, _) => m.unmatched = true,
            _ => {}
        }
    }
    m
}

impl Run<'_> {
    fn all(&mut self, settings: &TrelloSettings, snapshot: &Snapshot) -> TResult<()> {
        let duplicates = self.take_stock(snapshot);
        let mut worked = 0;
        let mut first_failure: Option<TrelloError> = None;
        for data in &snapshot.projects {
            let Some(link) = settings.projects.get(&data.slug).filter(|l| !l.board.is_empty()) else { continue };
            match self.project(data, link, &duplicates) {
                Ok(()) => worked += 1,
                Err(e @ (TrelloError::RateLimited { .. } | TrelloError::Unauthorized | TrelloError::Ledger(_))) => return Err(e),
                Err(e) => {
                    // One project that can't be reached is a note, and the others carry on.
                    let failure = format!("{}: {e}", data.name);
                    self.report.notes.push(failure.clone());
                    first_failure.get_or_insert(TrelloError::Message(failure));
                }
            }
            write_state(self.root, &self.state)?;
        }
        // If none could be reached, the round has failed (and is tried again soon) rather than being "up to date".
        match first_failure {
            Some(failure) if worked == 0 => Err(failure),
            _ => Ok(()),
        }
    }

    /// Looks over the whole vault before any board is touched: which cards are claimed, which links have lost
    /// their item, which items claim a card another item already has. Returns the items that must stop syncing
    /// because of the last.
    fn take_stock(&mut self, snapshot: &Snapshot) -> HashSet<(String, Kind, String)> {
        // (project, kind, id) → the item's card and whether it is detached.
        let mut live: HashMap<(String, Kind, String), (Option<String>, bool)> = HashMap::new();
        let mut claims: HashMap<String, Vec<(String, Kind, String, String)>> = HashMap::new();
        for data in &snapshot.projects {
            for task in &data.tasks {
                live.insert((data.slug.clone(), Kind::Task, task.id.to_lowercase()), (task.trello_card.clone(), task.trello_unlinked.is_some()));
                if task.trello_unlinked.is_some() {
                    self.report.unlinked += 1;
                }
                if let Some(card) = &task.trello_card {
                    claims.entry(card.clone()).or_default().push((data.slug.clone(), Kind::Task, task.id.clone(), task.created_at.clone()));
                }
            }
            for story in &data.stories {
                live.insert((data.slug.clone(), Kind::Story, story.id.to_lowercase()), (story.trello_card.clone(), story.trello_unlinked.is_some()));
                if story.trello_unlinked.is_some() {
                    self.report.unlinked += 1;
                }
                if let Some(card) = &story.trello_card {
                    claims.entry(card.clone()).or_default().push((data.slug.clone(), Kind::Story, story.id.clone(), story.created_at.clone()));
                }
            }
        }

        // An item that is gone (or now has another card) leaves its card behind: it must not come back as a new item.
        for (card, tracked) in self.state.items.clone() {
            match live.get(&(tracked.project.clone(), tracked.kind, tracked.id.to_lowercase())) {
                Some((Some(current), false)) if *current == card => {}
                Some((_, true)) => {
                    // Detached elsewhere (another machine); it is not ours to sync any more.
                    self.state.items.remove(&card);
                }
                _ => {
                    self.state.items.remove(&card);
                    self.state.released.insert(card);
                }
            }
        }

        self.claimed = claims.keys().cloned().collect();
        self.state.released.retain(|card| !self.claimed.contains(card));

        let mut duplicates = HashSet::new();
        for (card, mut owners) in claims {
            if owners.len() < 2 {
                continue;
            }
            // The item that was already syncing with the card keeps it; failing that, the oldest.
            let tracked = self.state.items.get(&card).map(|t| (t.project.clone(), t.kind, t.id.to_lowercase()));
            owners.sort_by(|a, b| {
                let a_tracked = tracked.as_ref() == Some(&(a.0.clone(), a.1, a.2.to_lowercase()));
                let b_tracked = tracked.as_ref() == Some(&(b.0.clone(), b.1, b.2.to_lowercase()));
                b_tracked.cmp(&a_tracked).then_with(|| (&a.3, &a.2).cmp(&(&b.3, &b.2)))
            });
            for (project, kind, id, _) in owners.into_iter().skip(1) {
                duplicates.insert((project, kind, id.to_lowercase()));
            }
        }
        duplicates
    }

    fn project(&mut self, data: &ProjectData, link: &ProjectLink, duplicates: &HashSet<(String, Kind, String)>) -> TResult<()> {
        let trello = self.trello;
        let members = trello.board_members(&link.board)?;
        if !members.iter().any(|m| m.id == self.member.id) {
            self.report.notes.push(format!(
                "{}: {} is not a member of the Trello board {}, so nothing was synced. Add them to the board, or link the project to another board.",
                data.name,
                self.member.display(),
                link.board_name
            ));
            return Ok(());
        }
        let lists = trello.lists(&link.board)?;
        let labels = self.kind_labels(&link.board)?;
        let cards = trello.cards(&link.board)?;
        let board = Board {
            id: &link.board,
            mapping: Mapping::new(&data.columns, &lists, &link.columns),
            labels,
            cards: cards.iter().map(|c| (c.id.as_str(), c)).collect(),
        };

        let mut unmatched_columns: BTreeSet<String> = BTreeSet::new();

        // 1. Items that have a card.
        let mut steps = vec![];
        for task in &data.tasks {
            let (Some(card), None) = (&task.trello_card, &task.trello_unlinked) else { continue };
            if duplicates.contains(&(data.slug.clone(), Kind::Task, task.id.to_lowercase())) {
                steps.push(self.duplicate(&data.slug, Kind::Task, &task.id, &task.title));
                continue;
            }
            let local = Local {
                kind: Kind::Task,
                id: task.id.clone(),
                card: card.clone(),
                updated_at: task.updated_at.clone(),
                content: task_content(task, board.mapping.list_for(&task.column_id)),
                title: task.title.clone(),
                text: task.body.clone(),
            };
            if local.content.list.is_none() {
                unmatched_columns.insert(column_name(&data.columns, &task.column_id));
            }
            self.linked(data, &board, &local, &mut steps)?;
        }
        for story in &data.stories {
            let (Some(card), None) = (&story.trello_card, &story.trello_unlinked) else { continue };
            if duplicates.contains(&(data.slug.clone(), Kind::Story, story.id.to_lowercase())) {
                steps.push(self.duplicate(&data.slug, Kind::Story, &story.id, &story.summary));
                continue;
            }
            let local = Local {
                kind: Kind::Story,
                id: story.id.clone(),
                card: card.clone(),
                updated_at: story.updated_at.clone(),
                content: story_content(story),
                title: story.summary.clone(),
                text: render_story(&story_text(story)),
            };
            self.linked(data, &board, &local, &mut steps)?;
        }
        self.commit(steps)?;

        // 2. Items that don't have a card yet.
        let mut steps = vec![];
        for task in &data.tasks {
            if task.trello_card.is_some() || task.trello_unlinked.is_some() {
                continue;
            }
            let Some(list) = board.mapping.list_for(&task.column_id) else {
                unmatched_columns.insert(column_name(&data.columns, &task.column_id));
                self.report.unmatched += 1;
                continue;
            };
            let content = task_content(task, Some(list));
            let label = format!("task “{}”", task.title);
            let created = self.create_card(&board, list, Kind::Task, &content, &label)?;
            if let Some(card) = created {
                let change = Change::Link { project: data.slug.clone(), kind: Kind::Task, id: task.id.clone(), card: card.id.clone() };
                steps.push(self.after_create(change, label, &data.slug, Kind::Task, &task.id, card, content));
            }
        }
        for story in &data.stories {
            if story.trello_card.is_some() || story.trello_unlinked.is_some() {
                continue;
            }
            let Some(list) = board.mapping.list_for(&data.backlog) else {
                unmatched_columns.insert(column_name(&data.columns, &data.backlog));
                self.report.unmatched += 1;
                continue;
            };
            let content = story_content(story);
            let label = format!("user story “{}”", story.summary);
            let created = self.create_card(&board, list, Kind::Story, &content, &label)?;
            if let Some(card) = created {
                let change = Change::Link { project: data.slug.clone(), kind: Kind::Story, id: story.id.clone(), card: card.id.clone() };
                steps.push(self.after_create(change, label, &data.slug, Kind::Story, &story.id, card, content));
            }
        }
        self.commit(steps)?;

        // 3. The person's cards that have no item.
        let mut steps = vec![];
        let mut unmatched_lists: BTreeSet<String> = BTreeSet::new();
        let mut ledger_made = 0;
        let mut taken_stories: HashSet<String> = data.stories.iter().map(|s| s.id.to_lowercase()).collect();
        for card in &cards {
            if !card.id_members.contains(&self.member.id) || self.claimed.contains(&card.id) || self.state.released.contains(&card.id) {
                continue;
            }
            // A card Ledger made belongs to an item that is on its way here from another machine, or that was deleted.
            if made_by_ledger(card) {
                ledger_made += 1;
                continue;
            }
            let stamp = if card.date_last_activity.is_empty() { ids::now() } else { card.date_last_activity.clone() };
            let created_at = card_created_at(&card.id).unwrap_or_else(|| stamp.clone());
            let step = match board.labels.new_kind(card) {
                Kind::Task => {
                    let Some(column) = board.mapping.column_for(&card.id_list) else {
                        unmatched_lists.insert(lists.iter().find(|l| l.id == card.id_list).map_or_else(|| card.id_list.clone(), |l| l.name.clone()));
                        self.report.unmatched += 1;
                        continue;
                    };
                    let id = format!("trello-{}", card.id);
                    if data.tasks.iter().any(|t| t.id == id) {
                        continue;
                    }
                    let content = card_task_content(card);
                    let change = Change::CreateTask {
                        project: data.slug.clone(),
                        id: id.clone(),
                        title: content.title.clone(),
                        body: content.desc.clone(),
                        column: column.to_string(),
                        card: card.id.clone(),
                        created_at,
                        updated_at: stamp,
                    };
                    let mut step = Step::new(change, format!("card “{}”", card.name), Count::ItemCreated);
                    step.track = Some((card.id.clone(), Tracked { project: data.slug.clone(), kind: Kind::Task, id, base: content }));
                    step
                }
                Kind::Story => {
                    let Some(id) = story_id_for(card, &taken_stories) else { continue };
                    taken_stories.insert(id.to_lowercase());
                    let content = card_story_content(card);
                    let change = Change::CreateStory {
                        project: data.slug.clone(),
                        id: id.clone(),
                        summary: content.title.clone(),
                        text: parse_story(&card_text(card)),
                        card: card.id.clone(),
                        created_at,
                        updated_at: stamp,
                    };
                    let mut step = Step::new(change, format!("card “{}”", card.name), Count::ItemCreated);
                    step.track = Some((card.id.clone(), Tracked { project: data.slug.clone(), kind: Kind::Story, id, base: content }));
                    step
                }
            };
            self.claimed.insert(card.id.clone());
            steps.push(step);
        }
        self.commit(steps)?;

        if ledger_made > 0 {
            self.report.notes.push(format!(
                "{}: {ledger_made} card(s) made by Ledger have no item in this vault, so they weren't added: their items may still be on their way from another machine, or were deleted here. To add one anyway, remove the “Synced with Ledger” lines from the end of its description.",
                data.name
            ));
        }
        if !unmatched_columns.is_empty() {
            self.report.notes.push(format!(
                "{}: tasks in {} have no list with a matching name on the Trello board, so they aren't moved there (choose a list for the column in the project's settings).",
                data.name,
                quoted(&unmatched_columns)
            ));
        }
        if !unmatched_lists.is_empty() {
            self.report.notes.push(format!(
                "{}: cards in {} have no column with a matching name, so they weren't added (choose a list for a column, or add a column with that name).",
                data.name,
                quoted(&unmatched_lists)
            ));
        }
        Ok(())
    }

    /// The board's "User story" and "Task" labels, made when they are missing.
    fn kind_labels(&mut self, board: &str) -> TResult<KindLabels> {
        let trello = self.trello;
        let mut existing = trello.labels(board)?;
        let mut find = |name: &str, color: &str| -> TResult<String> {
            if let Some(label) = existing.iter().find(|l| same_name(&l.name, name)) {
                return Ok(label.id.clone());
            }
            let made = trello.create_label(board, name, color)?;
            existing.push(made.clone());
            Ok(made.id)
        };
        Ok(KindLabels { story: find(STORY_LABEL, STORY_COLOR)?, task: find(TASK_LABEL, TASK_COLOR)? })
    }

    /// Where an item's card is now, and whether it is still the person's.
    fn locate(&self, board: &Board, card_id: &str) -> TResult<Located> {
        let card = match board.cards.get(card_id) {
            Some(card) => (*card).clone(),
            // Not among the board's open cards: find out what became of it, rather than assume.
            None => match self.trello.card(card_id)? {
                None => return Ok(Located::Gone("The card no longer exists on Trello.".into())),
                Some(card) if card.closed => return Ok(Located::Gone("The card was archived on Trello.".into())),
                Some(card) if card.id_board != board.id => return Ok(Located::Gone("The card was moved to another Trello board.".into())),
                Some(card) => card,
            },
        };
        if card.id_members.contains(&self.member.id) {
            Ok(Located::Open(card))
        } else {
            Ok(Located::Gone(format!("{} is no longer a member of the card on Trello.", self.member.display())))
        }
    }

    fn duplicate(&mut self, project: &str, kind: Kind, id: &str, title: &str) -> Step {
        Step::new(
            Change::Unlink {
                project: project.to_string(),
                kind,
                id: id.to_string(),
                reason: "Another item is linked to the same Trello card, so this one stopped syncing.".into(),
            },
            format!("{} “{title}”", kind.word()),
            Count::Detached,
        )
    }

    /// One linked item, against its card.
    fn linked(&mut self, data: &ProjectData, board: &Board, local: &Local, steps: &mut Vec<Step>) -> TResult<()> {
        let card = match self.locate(board, &local.card)? {
            Located::Open(card) => card,
            Located::Gone(reason) => {
                let mut step = Step::new(
                    Change::Unlink { project: data.slug.clone(), kind: local.kind, id: local.id.clone(), reason },
                    format!("{} “{}”", local.kind.word(), local.title),
                    Count::Detached,
                );
                step.untrack = Some(local.card.clone());
                steps.push(step);
                return Ok(());
            }
        };
        let label = format!("{} “{}”", local.kind.word(), local.title);
        let stamp = if card.date_last_activity.is_empty() { ids::now() } else { card.date_last_activity.clone() };

        // The card's label says what the item is.
        let wanted = board.labels.kind_of(&card, local.kind);
        if wanted != local.kind {
            let theirs = match wanted {
                Kind::Story => card_story_content(&card),
                Kind::Task => card_task_content(&card),
            };
            if local.content.title != theirs.title || norm(&local.text) != card_text(&card) {
                self.keep(Side::Ledger, &data.slug, local.kind, &local.id, &local.card, &local.title, &local.text)?;
                self.report.conflicts += 1;
            }
            let (change, tracked) = match wanted {
                Kind::Story => {
                    let story = story_id_for(&card, &data.stories.iter().map(|s| s.id.to_lowercase()).collect()).ok_or_else(|| {
                        TrelloError::message(format!("Could not find a free id for the story made from the card “{}”.", card.name))
                    })?;
                    (
                        Change::TaskToStory {
                            project: data.slug.clone(),
                            task: local.id.clone(),
                            expect: local.updated_at.clone(),
                            story: story.clone(),
                            summary: theirs.title.clone(),
                            text: parse_story(&card_text(&card)),
                            card: card.id.clone(),
                            updated_at: stamp,
                        },
                        Tracked { project: data.slug.clone(), kind: Kind::Story, id: story, base: theirs },
                    )
                }
                Kind::Task => {
                    let task = format!("trello-{}", card.id);
                    let column = board.mapping.column_for(&card.id_list).unwrap_or(&data.backlog).to_string();
                    (
                        Change::StoryToTask {
                            project: data.slug.clone(),
                            story: local.id.clone(),
                            expect: local.updated_at.clone(),
                            task: task.clone(),
                            title: theirs.title.clone(),
                            body: theirs.desc.clone(),
                            column,
                            card: card.id.clone(),
                            updated_at: stamp,
                        },
                        Tracked { project: data.slug.clone(), kind: Kind::Task, id: task, base: theirs },
                    )
                }
            };
            let mut step = Step::new(change, label, Count::Converted);
            step.track = Some((card.id.clone(), tracked));
            steps.push(step);
            return Ok(());
        }

        let theirs = match local.kind {
            Kind::Task => card_task_content(&card),
            Kind::Story => card_story_content(&card),
        };
        let base = self.state.items.get(&card.id).filter(|t| t.project == data.slug && t.kind == local.kind && t.id.eq_ignore_ascii_case(&local.id)).map(|t| t.base.clone());
        let ledger_newer = card.date_last_activity.is_empty() || local.updated_at >= card.date_last_activity;
        let merged = merge(local.kind, &local.content, &theirs, base.as_ref(), ledger_newer, &board.mapping);
        if merged.unmatched {
            self.report.unmatched += 1;
        }
        let tracked = Tracked { project: data.slug.clone(), kind: local.kind, id: local.id.clone(), base: merged.content.clone() };

        // What this side has that the card lacks.
        if !merged.push.is_empty() {
            let result = self.trello.update_card(&card.id, &merged.push);
            if self.soft(&label, result)?.is_none() {
                return Ok(());
            }
            self.report.cards_updated += 1;
        }

        if merged.ledger_lost || merged.trello_lost {
            self.report.conflicts += 1;
            if merged.ledger_lost {
                self.keep(Side::Ledger, &data.slug, local.kind, &local.id, &local.card, &local.title, &local.text)?;
            }
            if merged.trello_lost {
                self.keep(Side::Trello, &data.slug, local.kind, &local.id, &card.id, &card.name, &card.desc)?;
            }
            self.report.notes.push(format!(
                "{}: {label} changed on both sides. The newer edit ({}) was kept; the other is saved under .ledger/conflicts/{}.",
                data.name,
                if merged.ledger_lost { "from Trello" } else { "from Ledger" },
                self.stamp
            ));
        }

        // What the card has that this side lacks.
        if merged.pulls() {
            let change = match local.kind {
                Kind::Task => Change::UpdateTask {
                    project: data.slug.clone(),
                    id: local.id.clone(),
                    expect: local.updated_at.clone(),
                    title: merged.pull_title.clone(),
                    body: merged.pull_desc.clone(),
                    column: merged.pull_list.as_deref().and_then(|list| board.mapping.column_for(list)).map(str::to_string),
                    stamp,
                },
                Kind::Story => Change::UpdateStory {
                    project: data.slug.clone(),
                    id: local.id.clone(),
                    expect: local.updated_at.clone(),
                    summary: merged.pull_title.clone(),
                    text: merged.pull_desc.as_ref().map(|_| pulled_story(local, &card)),
                    stamp,
                },
            };
            let mut step = Step::new(change, label, Count::ItemUpdated);
            step.track = Some((card.id.clone(), tracked));
            steps.push(step);
        } else if base.as_ref() != Some(&tracked.base) {
            self.state.items.insert(card.id.clone(), tracked);
        }
        Ok(())
    }

    fn create_card(&mut self, board: &Board, list: &str, kind: Kind, content: &Content, label: &str) -> TResult<Option<Card>> {
        let new = NewCard {
            list: list.to_string(),
            name: content.title.clone(),
            desc: with_footer(&content.desc),
            // The person is assigned automatically, so the card is in scope from the start.
            member: self.member.id.clone(),
            labels: vec![board.labels.id(kind).to_string()],
        };
        let result = self.trello.create_card(&new);
        let created = self.soft(label, result)?;
        if let Some(card) = &created {
            self.claimed.insert(card.id.clone());
            self.report.cards_created += 1;
        }
        Ok(created)
    }

    #[allow(clippy::too_many_arguments)]
    fn after_create(&mut self, change: Change, label: String, project: &str, kind: Kind, id: &str, card: Card, base: Content) -> Step {
        let mut step = Step::new(change, label, Count::Nothing);
        step.orphan = Some(card.id.clone());
        step.track = Some((card.id, Tracked { project: project.to_string(), kind, id: id.to_string(), base }));
        step
    }

    /// Makes the changes in the vault, and remembers what both sides now agree on for each that worked.
    fn commit(&mut self, steps: Vec<Step>) -> TResult<()> {
        if steps.is_empty() {
            return Ok(());
        }
        let (changes, rest): (Vec<Change>, Vec<_>) = steps.into_iter().map(|s| (s.change, (s.label, s.track, s.untrack, s.orphan, s.count))).unzip();
        let results = self.ledger.apply(changes)?;
        for ((label, track, untrack, orphan, count), result) in rest.into_iter().zip(results) {
            match result {
                Applied::Done => {
                    self.report.vault_writes += 1;
                    match count {
                        Count::Nothing => {}
                        Count::ItemCreated => self.report.items_created += 1,
                        Count::ItemUpdated => self.report.items_updated += 1,
                        Count::Converted => self.report.converted += 1,
                        Count::Detached => {
                            self.report.detached += 1;
                            self.report.unlinked += 1;
                        }
                    }
                    if let Some((card, tracked)) = track {
                        self.state.items.insert(card, tracked);
                    }
                    if let Some(card) = untrack {
                        self.state.items.remove(&card);
                    }
                }
                Applied::Skipped | Applied::Failed(_) => {
                    if let Applied::Failed(message) = &result {
                        self.report.notes.push(format!("{label}: {message}"));
                    } else {
                        self.report.skipped += 1;
                    }
                    if let Some(card) = orphan {
                        self.state.released.insert(card);
                    }
                }
            }
        }
        Ok(())
    }

    /// A failure of one card is a note and the round carries on; one that would hit every card ends the round.
    fn soft<T>(&mut self, what: &str, result: TResult<T>) -> TResult<Option<T>> {
        match result {
            Ok(value) => Ok(Some(value)),
            Err(e @ (TrelloError::RateLimited { .. } | TrelloError::Unauthorized | TrelloError::Ledger(_))) => Err(e),
            Err(e) => {
                self.report.notes.push(format!("{what}: {e}"));
                Ok(None)
            }
        }
    }

    /// Saves one side's version of an item that lost a conflict, where the GitHub sync keeps its own.
    fn keep(&self, side: Side, project: &str, kind: Kind, id: &str, card: &str, title: &str, text: &str) -> TResult<()> {
        let mut meta = Meta::new();
        frontmatter::set(&mut meta, "kept", side.word());
        frontmatter::set(&mut meta, "project", project);
        frontmatter::set(&mut meta, "kind", kind.word());
        frontmatter::set(&mut meta, "item", id);
        frontmatter::set(&mut meta, "card", card);
        frontmatter::set(&mut meta, "title", title);
        let file = self
            .root
            .join(crate::settings::LEDGER_DIR)
            .join("conflicts")
            .join(&self.stamp)
            .join("trello")
            .join(project)
            .join(format!("{}-{id}.{}.md", kind.folder(), side.word()));
        if let Some(dir) = file.parent() {
            fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
        }
        fs::write(&file, frontmatter::render(&meta, text)).map_err(|e| Error::io(&file, e))?;
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Side {
    Ledger,
    Trello,
}

impl Side {
    fn word(self) -> &'static str {
        match self {
            Side::Ledger => "ledger",
            Side::Trello => "trello",
        }
    }
}

/// The story's text after taking what the card says. A description in Ledger's format says everything, so the role,
/// goal, reason and criteria follow it (and are cleared if it leaves them out). A description someone wrote freely
/// in Trello has none of that: it replaces the notes, and the story keeps its role, goal, reason and criteria, which
/// the next round then writes back into the description.
fn pulled_story(local: &Local, card: &Card) -> StoryText {
    let (theirs, marked) = parse_story_marked(&card_text(card));
    if marked { theirs } else { StoryText { notes: theirs.notes, ..parse_story(&local.content.desc) } }
}

fn column_name(columns: &[Column], id: &str) -> String {
    columns.iter().find(|c| c.id == id).map_or_else(|| id.to_string(), |c| c.name.clone())
}

fn quoted(names: &BTreeSet<String>) -> String {
    names.iter().map(|n| format!("“{n}”")).collect::<Vec<_>>().join(", ")
}

/// The id of the story a card becomes: `TR-<card number>`, made from the card so every machine picks the same.
/// `None` if that is taken even with the card's own id added.
fn story_id_for(card: &Card, taken: &HashSet<String>) -> Option<String> {
    let tail = &card.id[card.id.len().saturating_sub(4)..];
    let plain = if card.id_short > 0 { format!("TR-{}", card.id_short) } else { format!("TR-{tail}") };
    [plain.clone(), format!("{plain}-{tail}")].into_iter().find(|id| !taken.contains(&id.to_lowercase()))
}
