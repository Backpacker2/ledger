//! User stories are what the planboard shows. A story records which threads it is linked to — many-to-many,
//! and a document reaches a story through the thread it is linked to.

use std::collections::HashMap;

use rusqlite::{Connection, params};

use crate::db::{query_all, query_opt};
use crate::error::{Error, Result};
use crate::models::{Criterion, LinkedThread, StoryDraft, StoryUpdate, StoryView};
use crate::{ids, planboard, threads};

const MAX_ID_LENGTH: usize = 40;

/// Letters, digits, '-' and '_', starting with a letter or digit (e.g. MM01).
fn valid_id(id: &str) -> bool {
    id.len() <= MAX_ID_LENGTH
        && id.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Oldest first, which is the order cards sit in within a column.
pub fn list(conn: &Connection) -> Result<Vec<StoryView>> {
    let mut criteria: HashMap<String, Vec<Criterion>> = HashMap::new();
    for (story_id, criterion) in query_all(
        conn,
        "SELECT story_id, text, done FROM acceptance_criteria ORDER BY story_id, position",
        [],
        |r| Ok((r.get::<_, String>(0)?, Criterion { text: r.get(1)?, done: r.get(2)? })),
    )? {
        criteria.entry(story_id.to_lowercase()).or_default().push(criterion);
    }

    let mut linked: HashMap<String, Vec<LinkedThread>> = HashMap::new();
    for (story_id, thread) in query_all(
        conn,
        "SELECT s.story_id, t.slug, t.title
         FROM story_threads s JOIN threads t ON t.slug = s.thread_slug
         ORDER BY t.title COLLATE NOCASE, t.slug",
        [],
        |r| Ok((r.get::<_, String>(0)?, LinkedThread { slug: r.get(1)?, title: r.get(2)? })),
    )? {
        linked.entry(story_id.to_lowercase()).or_default().push(thread);
    }

    query_all(
        conn,
        "SELECT id, summary, role, want, reason, column_id, created_at, updated_at
         FROM user_stories ORDER BY created_at, id COLLATE NOCASE",
        [],
        |r| {
            Ok(StoryView {
                id: r.get(0)?,
                summary: r.get(1)?,
                role: r.get(2)?,
                want: r.get(3)?,
                reason: r.get(4)?,
                criteria: vec![],
                column_id: r.get(5)?,
                threads: vec![],
                created_at: r.get(6)?,
                updated_at: r.get(7)?,
            })
        },
    )
    .map(|mut stories| {
        for story in &mut stories {
            let key = story.id.to_lowercase();
            story.criteria = criteria.remove(&key).unwrap_or_default();
            story.threads = linked.remove(&key).unwrap_or_default();
        }
        stories
    })
}

pub fn list_for_thread(conn: &Connection, slug: &str) -> Result<Vec<StoryView>> {
    Ok(list(conn)?.into_iter().filter(|s| s.threads.iter().any(|t| t.slug == slug)).collect())
}

pub fn get(conn: &Connection, id: &str) -> Result<StoryView> {
    list(conn)?
        .into_iter()
        .find(|s| s.id.eq_ignore_ascii_case(id))
        .ok_or_else(|| Error::not_found("User story not found"))
}

/// A new story lands in the backlog column, optionally already linked to threads.
pub fn create(conn: &Connection, draft: StoryDraft) -> Result<StoryView> {
    let id = draft.id.trim();
    if id.is_empty() {
        return Err(Error::invalid("ID is required"));
    }
    if !valid_id(id) {
        return Err(Error::invalid("ID may only contain letters, digits, '-' and '_', and must start with a letter or digit (e.g. MM01)"));
    }
    let summary = require_summary(&draft.summary)?;
    for slug in &draft.thread_slugs {
        threads::require(conn, slug.trim())?;
    }
    if exists(conn, id)? {
        return Err(Error::invalid(format!("A user story with ID {id} already exists")));
    }

    let column_id = planboard::backlog_column_id(conn)?;
    let now = ids::now();
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO user_stories (id, summary, role, want, reason, column_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
        params![id, summary, draft.role.trim(), draft.want.trim(), draft.reason.trim(), column_id, now],
    )?;
    write_criteria(&tx, id, &draft.criteria)?;
    for slug in &draft.thread_slugs {
        tx.execute("INSERT OR IGNORE INTO story_threads (story_id, thread_slug) VALUES (?1, ?2)", [id, slug.trim()])?;
    }
    tx.commit()?;
    get(conn, id)
}

pub fn update(conn: &Connection, id: &str, updates: StoryUpdate) -> Result<StoryView> {
    let id = get(conn, id)?.id; // canonical spelling; not found if it does not exist

    let tx = conn.unchecked_transaction()?;
    if let Some(summary) = &updates.summary {
        tx.execute("UPDATE user_stories SET summary = ?2 WHERE id = ?1", [id.as_str(), require_summary(summary)?.as_str()])?;
    }
    if let Some(role) = &updates.role {
        tx.execute("UPDATE user_stories SET role = ?2 WHERE id = ?1", [id.as_str(), role.trim()])?;
    }
    if let Some(want) = &updates.want {
        tx.execute("UPDATE user_stories SET want = ?2 WHERE id = ?1", [id.as_str(), want.trim()])?;
    }
    if let Some(reason) = &updates.reason {
        tx.execute("UPDATE user_stories SET reason = ?2 WHERE id = ?1", [id.as_str(), reason.trim()])?;
    }
    if let Some(criteria) = &updates.criteria {
        write_criteria(&tx, &id, criteria)?;
    }
    if let Some(column_id) = &updates.column_id {
        if !planboard::column_exists(&tx, column_id)? {
            return Err(Error::not_found("Column not found"));
        }
        tx.execute("UPDATE user_stories SET column_id = ?2 WHERE id = ?1", [id.as_str(), column_id.as_str()])?;
    }
    tx.execute("UPDATE user_stories SET updated_at = ?2 WHERE id = ?1", [id.as_str(), ids::now().as_str()])?;
    tx.commit()?;
    get(conn, &id)
}

pub fn delete(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("DELETE FROM user_stories WHERE id = ?1", [id])?;
    Ok(())
}

pub fn add_thread(conn: &Connection, id: &str, slug: &str) -> Result<StoryView> {
    let id = get(conn, id)?.id;
    let slug = slug.trim();
    threads::require(conn, slug)?;
    conn.execute("INSERT OR IGNORE INTO story_threads (story_id, thread_slug) VALUES (?1, ?2)", [id.as_str(), slug])?;
    touch(conn, &id)?;
    get(conn, &id)
}

pub fn remove_thread(conn: &Connection, id: &str, slug: &str) -> Result<StoryView> {
    let id = get(conn, id)?.id;
    conn.execute("DELETE FROM story_threads WHERE story_id = ?1 AND thread_slug = ?2", [id.as_str(), slug])?;
    touch(conn, &id)?;
    get(conn, &id)
}

pub fn count_in_column(conn: &Connection, column_id: &str) -> Result<i64> {
    Ok(conn.query_row("SELECT COUNT(*) FROM user_stories WHERE column_id = ?1", [column_id], |r| r.get(0))?)
}

fn exists(conn: &Connection, id: &str) -> Result<bool> {
    Ok(query_opt(conn, "SELECT 1 FROM user_stories WHERE id = ?1", [id], |_| Ok(()))?.is_some())
}

fn touch(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("UPDATE user_stories SET updated_at = ?2 WHERE id = ?1", [id, ids::now().as_str()])?;
    Ok(())
}

fn require_summary(summary: &str) -> Result<String> {
    let summary = summary.trim();
    if summary.is_empty() { Err(Error::invalid("Summary is required")) } else { Ok(summary.to_string()) }
}

/// Criteria with no text are dropped; the order the caller gave is kept.
fn write_criteria(conn: &Connection, id: &str, criteria: &[Criterion]) -> Result<()> {
    conn.execute("DELETE FROM acceptance_criteria WHERE story_id = ?1", [id])?;
    let cleaned = criteria.iter().filter(|c| !c.text.trim().is_empty());
    for (position, criterion) in cleaned.enumerate() {
        conn.execute(
            "INSERT INTO acceptance_criteria (story_id, position, text, done) VALUES (?1, ?2, ?3, ?4)",
            params![id, position as i64, criterion.text.trim(), criterion.done],
        )?;
    }
    Ok(())
}
