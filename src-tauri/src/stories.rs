//! User stories belong to a project. A story records which of its project's threads it is linked to (many-to-many);
//! tasks on the board can point at a story.

use std::collections::HashMap;

use rusqlite::{Connection, params};

use crate::db::{query_all, query_opt};
use crate::error::{Error, Result};
use crate::models::{Criterion, LinkedThread, StoryDraft, StoryUpdate, StoryView};
use crate::{ids, projects, threads};

const MAX_ID_LENGTH: usize = 40;

/// Letters, digits, '-' and '_', starting with a letter or digit (e.g. MM01).
fn valid_id(id: &str) -> bool {
    id.len() <= MAX_ID_LENGTH
        && id.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Oldest first.
pub fn list(conn: &Connection, project: &str) -> Result<Vec<StoryView>> {
    let mut criteria: HashMap<String, Vec<Criterion>> = HashMap::new();
    for (story_id, criterion) in query_all(
        conn,
        "SELECT story_id, text, done FROM acceptance_criteria WHERE project_slug = ?1 ORDER BY story_id, position",
        [project],
        |r| Ok((r.get::<_, String>(0)?, Criterion { text: r.get(1)?, done: r.get(2)? })),
    )? {
        criteria.entry(story_id.to_lowercase()).or_default().push(criterion);
    }

    let mut linked: HashMap<String, Vec<LinkedThread>> = HashMap::new();
    for (story_id, thread) in query_all(
        conn,
        "SELECT s.story_id, t.slug, t.title
         FROM story_threads s JOIN threads t ON t.project_slug = s.project_slug AND t.slug = s.thread_slug
         WHERE s.project_slug = ?1 ORDER BY t.title COLLATE NOCASE, t.slug",
        [project],
        |r| Ok((r.get::<_, String>(0)?, LinkedThread { slug: r.get(1)?, title: r.get(2)? })),
    )? {
        linked.entry(story_id.to_lowercase()).or_default().push(thread);
    }

    let mut task_counts: HashMap<String, i64> = HashMap::new();
    for (story_id, count) in query_all(
        conn,
        "SELECT story_id, COUNT(*) FROM tasks WHERE project_slug = ?1 AND story_id IS NOT NULL GROUP BY story_id",
        [project],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
    )? {
        task_counts.insert(story_id.to_lowercase(), count);
    }

    let mut stories = query_all(
        conn,
        "SELECT project_slug, id, summary, role, want, reason, notes, created_at, updated_at, trello_card, trello_unlinked
         FROM user_stories WHERE project_slug = ?1 ORDER BY created_at, id COLLATE NOCASE",
        [project],
        |r| {
            Ok(StoryView {
                project_slug: r.get(0)?,
                id: r.get(1)?,
                summary: r.get(2)?,
                role: r.get(3)?,
                want: r.get(4)?,
                reason: r.get(5)?,
                notes: r.get(6)?,
                criteria: vec![],
                threads: vec![],
                task_count: 0,
                created_at: r.get(7)?,
                updated_at: r.get(8)?,
                trello_card: r.get(9)?,
                trello_unlinked: r.get(10)?,
            })
        },
    )?;
    for story in &mut stories {
        let key = story.id.to_lowercase();
        story.criteria = criteria.remove(&key).unwrap_or_default();
        story.threads = linked.remove(&key).unwrap_or_default();
        story.task_count = task_counts.get(&key).copied().unwrap_or(0);
    }
    Ok(stories)
}

pub fn list_for_thread(conn: &Connection, project: &str, thread: &str) -> Result<Vec<StoryView>> {
    Ok(list(conn, project)?.into_iter().filter(|s| s.threads.iter().any(|t| t.slug == thread)).collect())
}

pub fn get(conn: &Connection, project: &str, id: &str) -> Result<StoryView> {
    list(conn, project)?
        .into_iter()
        .find(|s| s.id.eq_ignore_ascii_case(id))
        .ok_or_else(|| Error::not_found("User story not found"))
}

pub fn create(conn: &Connection, project: &str, draft: StoryDraft) -> Result<StoryView> {
    projects::require(conn, project)?;
    let id = draft.id.trim();
    if id.is_empty() {
        return Err(Error::invalid("ID is required"));
    }
    if !valid_id(id) {
        return Err(Error::invalid("ID may only contain letters, digits, '-' and '_', and must start with a letter or digit (e.g. MM01)"));
    }
    let summary = require_summary(&draft.summary)?;
    for slug in &draft.thread_slugs {
        threads::require(conn, project, slug.trim())?;
    }
    if exists(conn, project, id)? {
        return Err(Error::invalid(format!("A user story with ID {id} already exists")));
    }

    let now = ids::now();
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO user_stories (project_slug, id, summary, role, want, reason, notes, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
        params![project, id, summary, draft.role.trim(), draft.want.trim(), draft.reason.trim(), draft.notes.trim(), now],
    )?;
    write_criteria(&tx, project, id, &draft.criteria)?;
    for slug in &draft.thread_slugs {
        tx.execute(
            "INSERT OR IGNORE INTO story_threads (project_slug, story_id, thread_slug) VALUES (?1, ?2, ?3)",
            [project, id, slug.trim()],
        )?;
    }
    tx.commit()?;
    get(conn, project, id)
}

pub fn update(conn: &Connection, project: &str, id: &str, updates: StoryUpdate) -> Result<StoryView> {
    let id = get(conn, project, id)?.id; // canonical spelling; not found if it does not exist

    let tx = conn.unchecked_transaction()?;
    let set = |column: &str, value: &str| -> Result<()> {
        tx.execute(&format!("UPDATE user_stories SET {column} = ?3 WHERE project_slug = ?1 AND id = ?2"), [project, id.as_str(), value])?;
        Ok(())
    };
    if let Some(summary) = &updates.summary {
        set("summary", &require_summary(summary)?)?;
    }
    if let Some(role) = &updates.role {
        set("role", role.trim())?;
    }
    if let Some(want) = &updates.want {
        set("want", want.trim())?;
    }
    if let Some(reason) = &updates.reason {
        set("reason", reason.trim())?;
    }
    if let Some(notes) = &updates.notes {
        set("notes", notes.trim())?;
    }
    if let Some(criteria) = &updates.criteria {
        write_criteria(&tx, project, &id, criteria)?;
    }
    set("updated_at", &ids::now())?;
    tx.commit()?;
    get(conn, project, &id)
}

/// Tasks that pointed at the story just lose that link.
pub fn delete(conn: &Connection, project: &str, id: &str) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("UPDATE tasks SET story_id = NULL WHERE project_slug = ?1 AND story_id = ?2", [project, id])?;
    tx.execute("DELETE FROM user_stories WHERE project_slug = ?1 AND id = ?2", [project, id])?;
    tx.commit()?;
    Ok(())
}

pub fn add_thread(conn: &Connection, project: &str, id: &str, slug: &str) -> Result<StoryView> {
    let id = get(conn, project, id)?.id;
    let slug = slug.trim();
    threads::require(conn, project, slug)?;
    conn.execute("INSERT OR IGNORE INTO story_threads (project_slug, story_id, thread_slug) VALUES (?1, ?2, ?3)", [project, id.as_str(), slug])?;
    touch(conn, project, &id)?;
    get(conn, project, &id)
}

pub fn remove_thread(conn: &Connection, project: &str, id: &str, slug: &str) -> Result<StoryView> {
    let id = get(conn, project, id)?.id;
    conn.execute("DELETE FROM story_threads WHERE project_slug = ?1 AND story_id = ?2 AND thread_slug = ?3", [project, id.as_str(), slug])?;
    touch(conn, project, &id)?;
    get(conn, project, &id)
}

pub fn exists(conn: &Connection, project: &str, id: &str) -> Result<bool> {
    Ok(query_opt(conn, "SELECT 1 FROM user_stories WHERE project_slug = ?1 AND id = ?2", [project, id], |_| Ok(()))?.is_some())
}

fn touch(conn: &Connection, project: &str, id: &str) -> Result<()> {
    conn.execute("UPDATE user_stories SET updated_at = ?3 WHERE project_slug = ?1 AND id = ?2", [project, id, ids::now().as_str()])?;
    Ok(())
}

fn require_summary(summary: &str) -> Result<String> {
    let summary = summary.trim();
    if summary.is_empty() { Err(Error::invalid("Summary is required")) } else { Ok(summary.to_string()) }
}

/// Criteria with no text are dropped; the order the caller gave is kept.
fn write_criteria(conn: &Connection, project: &str, id: &str, criteria: &[Criterion]) -> Result<()> {
    conn.execute("DELETE FROM acceptance_criteria WHERE project_slug = ?1 AND story_id = ?2", [project, id])?;
    let cleaned = criteria.iter().filter(|c| !c.text.trim().is_empty());
    for (position, criterion) in cleaned.enumerate() {
        conn.execute(
            "INSERT INTO acceptance_criteria (project_slug, story_id, position, text, done) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![project, id, position as i64, criterion.text.trim(), criterion.done],
        )?;
    }
    Ok(())
}
