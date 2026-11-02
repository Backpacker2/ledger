//! The Ledger side of the Trello sync: reading the tasks and stories, and writing what Trello changed back into
//! the vault.
//!
//! The sync engine works against the [`Ledger`] trait. The app's version, [`CoreLedger`], takes the vault lock
//! only for the moment it reads or writes (never while talking to Trello), changes the in-memory database
//! through the same stores the screens use, and writes the difference through to the files, which the GitHub
//! sync then notices like any other edit. Each change says what the item looked like when it was read
//! (`expect`), and is left undone if it has been edited since: the next round deals with it.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use rusqlite::{Connection, params};

use crate::db::query_all;
use crate::error::{Error, Result};
use crate::models::{Column, Criterion, StoryDraft, StoryUpdate, StoryView, TaskUpdate, TaskView};
use crate::state::Core;
use crate::trello_card::{Kind, StoryText};
use crate::{board, stories};

// ---- what the engine sees -----------------------------------------------------------------------------

pub struct ProjectData {
    pub slug: String,
    pub name: String,
    pub columns: Vec<Column>,
    /// Where a new task lands: also where the card of a new story is put.
    pub backlog: String,
    pub tasks: Vec<TaskView>,
    pub stories: Vec<StoryView>,
}

#[derive(Default)]
pub struct Snapshot {
    pub projects: Vec<ProjectData>,
}

/// One change to make to the vault.
#[derive(Debug, Clone)]
pub enum Change {
    /// A card was made for the item: remember its id. Doesn't count as an edit.
    Link { project: String, kind: Kind, id: String, card: String },
    /// The item stops syncing, and says why. Its card id stays, so it never gets a second card.
    Unlink { project: String, kind: Kind, id: String, reason: String },
    UpdateTask { project: String, id: String, expect: String, title: Option<String>, body: Option<String>, column: Option<String>, stamp: String },
    UpdateStory { project: String, id: String, expect: String, summary: Option<String>, text: Option<StoryText>, stamp: String },
    CreateTask { project: String, id: String, title: String, body: String, column: String, card: String, created_at: String, updated_at: String },
    CreateStory { project: String, id: String, summary: String, text: StoryText, card: String, created_at: String, updated_at: String },
    /// The card's label now says "user story": the task becomes a story (the task is removed, the story keeps the card).
    TaskToStory { project: String, task: String, expect: String, story: String, summary: String, text: StoryText, card: String, updated_at: String },
    /// The card's label now says "task".
    StoryToTask { project: String, story: String, expect: String, task: String, title: String, body: String, column: String, card: String, updated_at: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    Done,
    /// Left alone: the item is gone or was edited since it was read.
    Skipped,
    Failed(String),
}

pub trait Ledger: Sync {
    fn snapshot(&self) -> Result<Snapshot>;
    /// Applies the changes in order; one answer for each.
    fn apply(&self, changes: Vec<Change>) -> Result<Vec<Applied>>;
}

// ---- the app's ledger ---------------------------------------------------------------------------------

pub struct CoreLedger {
    pub core: Arc<Mutex<Core>>,
    /// Called after the vault's files were written, so the GitHub sync picks the change up.
    pub changed: Arc<dyn Fn() + Send + Sync>,
}

fn lock(core: &Mutex<Core>) -> MutexGuard<'_, Core> {
    core.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Ledger for CoreLedger {
    fn snapshot(&self) -> Result<Snapshot> {
        snapshot(&lock(&self.core).conn)
    }

    fn apply(&self, changes: Vec<Change>) -> Result<Vec<Applied>> {
        let (results, wrote) = {
            let mut guard = lock(&self.core);
            let core = &mut *guard;
            let results: Vec<Applied> = changes
                .iter()
                .map(|change| apply_one(&core.conn, change).unwrap_or_else(|e| Applied::Failed(e.to_string())))
                .collect();
            let wrote = results.contains(&Applied::Done);
            if wrote {
                core.vault.persist(&core.conn)?;
            }
            (results, wrote)
        };
        if wrote {
            (self.changed)();
        }
        Ok(results)
    }
}

pub fn snapshot(conn: &Connection) -> Result<Snapshot> {
    let mut projects = vec![];
    for (slug, name) in query_all(conn, "SELECT slug, name FROM projects ORDER BY slug", [], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        projects.push(ProjectData {
            columns: board::columns(conn, &slug)?,
            backlog: board::backlog_column_id(conn, &slug)?,
            tasks: board::tasks(conn, &slug)?,
            stories: stories::list(conn, &slug)?,
            slug,
            name,
        });
    }
    Ok(Snapshot { projects })
}

fn table(kind: Kind) -> &'static str {
    match kind {
        Kind::Task => "tasks",
        Kind::Story => "user_stories",
    }
}

/// Records the card id on an item without counting it as an edit (its `updated_at` stays as it was). Returns
/// whether the item exists.
pub fn set_link(conn: &Connection, project: &str, kind: Kind, id: &str, card: Option<&str>, unlinked: Option<&str>) -> Result<bool> {
    let sql = format!("UPDATE {} SET trello_card = ?3, trello_unlinked = ?4 WHERE project_slug = ?1 AND id = ?2", table(kind));
    Ok(conn.execute(&sql, params![project, id, card, unlinked])? > 0)
}

fn stamp_item(conn: &Connection, project: &str, kind: Kind, id: &str, created_at: Option<&str>, updated_at: &str) -> Result<()> {
    let sql = match (kind, created_at.is_some()) {
        (Kind::Task, true) => "UPDATE tasks SET created_at = ?3, updated_at = ?4 WHERE project_slug = ?1 AND id = ?2",
        (Kind::Task, false) => "UPDATE tasks SET updated_at = ?4 WHERE project_slug = ?1 AND id = ?2",
        (Kind::Story, true) => "UPDATE user_stories SET created_at = ?3, updated_at = ?4 WHERE project_slug = ?1 AND id = ?2",
        (Kind::Story, false) => "UPDATE user_stories SET updated_at = ?4 WHERE project_slug = ?1 AND id = ?2",
    };
    conn.execute(sql, params![project, id, created_at, updated_at])?;
    Ok(())
}

fn criteria(text: &StoryText) -> Vec<Criterion> {
    text.criteria.iter().map(|c| Criterion { text: c.text.clone(), done: c.done }).collect()
}

fn insert_task(conn: &Connection, project: &str, id: &str, title: &str, body: &str, column: &str, card: &str, created_at: &str, updated_at: &str) -> Result<Applied> {
    if title.trim().is_empty() {
        return Ok(Applied::Failed("the card has no title".into()));
    }
    if !board::column_exists(conn, project, column)? {
        return Ok(Applied::Failed(format!("the column {column} no longer exists")));
    }
    let inserted = conn.execute(
        "INSERT OR IGNORE INTO tasks (id, project_slug, title, body, column_id, story_id, thread_slug, created_at, updated_at, trello_card)
         VALUES (?1, ?2, ?3, ?4, ?5, NULL, NULL, ?6, ?7, ?8)",
        params![id, project, title.trim(), body.trim(), column, created_at, updated_at, card],
    )?;
    Ok(if inserted == 0 { Applied::Skipped } else { Applied::Done })
}

fn insert_story(conn: &Connection, project: &str, id: &str, summary: &str, text: &StoryText, threads: Vec<String>, card: &str, created_at: &str, updated_at: &str) -> Result<Applied> {
    if stories::exists(conn, project, id)? {
        return Ok(Applied::Skipped);
    }
    let draft = StoryDraft {
        id: id.to_string(),
        summary: summary.to_string(),
        role: text.role.clone(),
        want: text.want.clone(),
        reason: text.reason.clone(),
        notes: text.notes.clone(),
        criteria: criteria(text),
        thread_slugs: threads,
    };
    let created = stories::create(conn, project, draft)?;
    set_link(conn, project, Kind::Story, &created.id, Some(card), None)?;
    stamp_item(conn, project, Kind::Story, &created.id, Some(created_at), updated_at)?;
    Ok(Applied::Done)
}

fn apply_one(conn: &Connection, change: &Change) -> Result<Applied> {
    Ok(match change {
        Change::Link { project, kind, id, card } => {
            if set_link(conn, project, *kind, id, Some(card), None)? { Applied::Done } else { Applied::Skipped }
        }
        Change::Unlink { project, kind, id, reason } => {
            let card = match kind {
                Kind::Task => board::task(conn, project, id).ok().and_then(|t| t.trello_card),
                Kind::Story => stories::get(conn, project, id).ok().and_then(|s| s.trello_card),
            };
            match card {
                Some(card) if set_link(conn, project, *kind, id, Some(&card), Some(reason))? => Applied::Done,
                _ => Applied::Skipped,
            }
        }
        Change::UpdateTask { project, id, expect, title, body, column, stamp } => {
            match board::task(conn, project, id) {
                Ok(task) if task.updated_at == *expect => {}
                _ => return Ok(Applied::Skipped),
            }
            board::update_task(conn, project, id, TaskUpdate { title: title.clone(), body: body.clone(), column_id: column.clone(), ..TaskUpdate::default() })?;
            stamp_item(conn, project, Kind::Task, id, None, stamp)?;
            Applied::Done
        }
        Change::UpdateStory { project, id, expect, summary, text, stamp } => {
            let story = match stories::get(conn, project, id) {
                Ok(story) if story.updated_at == *expect => story,
                _ => return Ok(Applied::Skipped),
            };
            let mut update = StoryUpdate { summary: summary.clone(), ..StoryUpdate::default() };
            if let Some(text) = text {
                update.role = Some(text.role.clone());
                update.want = Some(text.want.clone());
                update.reason = Some(text.reason.clone());
                update.notes = Some(text.notes.clone());
                update.criteria = Some(criteria(text));
            }
            stories::update(conn, project, &story.id, update)?;
            stamp_item(conn, project, Kind::Story, &story.id, None, stamp)?;
            Applied::Done
        }
        Change::CreateTask { project, id, title, body, column, card, created_at, updated_at } => {
            insert_task(conn, project, id, title, body, column, card, created_at, updated_at)?
        }
        Change::CreateStory { project, id, summary, text, card, created_at, updated_at } => {
            insert_story(conn, project, id, summary, text, vec![], card, created_at, updated_at)?
        }
        Change::TaskToStory { project, task, expect, story, summary, text, card, updated_at } => {
            let old = match board::task(conn, project, task) {
                Ok(old) if old.updated_at == *expect => old,
                _ => return Ok(Applied::Skipped),
            };
            let threads = old.thread_slug.clone().into_iter().collect();
            // The story is made first: if that fails, nothing has changed.
            let made = insert_story(conn, project, story, summary, text, threads, card, &old.created_at, updated_at)?;
            if made == Applied::Done {
                board::delete_task(conn, project, task)?;
            }
            made
        }
        Change::StoryToTask { project, story, expect, task, title, body, column, card, updated_at } => {
            let old = match stories::get(conn, project, story) {
                Ok(old) if old.updated_at == *expect => old,
                _ => return Ok(Applied::Skipped),
            };
            let made = insert_task(conn, project, task, title, body, column, card, &old.created_at, updated_at)?;
            if made == Applied::Done {
                stories::delete(conn, project, &old.id)?;
            }
            made
        }
    })
}

/// Gives an item a new card: its link is cleared, so the next sync makes one. Returns the card it had.
pub fn clear_link(conn: &Connection, project: &str, kind: Kind, id: &str) -> Result<Option<String>> {
    let card = match kind {
        Kind::Task => board::task(conn, project, id)?.trello_card,
        Kind::Story => stories::get(conn, project, id)?.trello_card,
    };
    let canonical = match kind {
        Kind::Task => id.to_string(),
        Kind::Story => stories::get(conn, project, id)?.id,
    };
    if !set_link(conn, project, kind, &canonical, None, None)? {
        return Err(Error::not_found("Item not found"));
    }
    Ok(card)
}
