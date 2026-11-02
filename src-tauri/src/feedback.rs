//! Feedback is written in a thread and lands on a deliverable (or one of its published versions).

use rusqlite::{Connection, Row, params};

use crate::db::{query_all, query_opt};
use crate::error::{Error, Result};
use crate::models::FeedbackView;
use crate::{deliverables, ids, markdown, threads};

const COLUMNS: &str = "id, thread_slug, deliverable_path, deliverable_version, status, created_at, updated_at, body";

fn from_row(r: &Row<'_>) -> rusqlite::Result<FeedbackView> {
    let body: String = r.get(7)?;
    Ok(FeedbackView {
        id: r.get(0)?,
        thread_slug: r.get(1)?,
        deliverable_path: r.get(2)?,
        deliverable_version: r.get(3)?,
        status: r.get(4)?,
        created_at: r.get(5)?,
        updated_at: r.get(6)?,
        html: markdown::render(&body),
        body,
    })
}

/// Oldest first.
pub fn list_for_thread(conn: &Connection, project: &str, thread: &str) -> Result<Vec<FeedbackView>> {
    query_all(
        conn,
        &format!("SELECT {COLUMNS} FROM feedback WHERE project_slug = ?1 AND thread_slug = ?2 ORDER BY created_at, rowid"),
        [project, thread],
        from_row,
    )
}

/// All feedback on a deliverable and on its published versions, from every thread, oldest first.
pub fn list_for_deliverable(conn: &Connection, project: &str, base_path: &str) -> Result<Vec<FeedbackView>> {
    let all = query_all(
        conn,
        &format!("SELECT {COLUMNS} FROM feedback WHERE project_slug = ?1 ORDER BY created_at, rowid"),
        [project],
        from_row,
    )?;
    Ok(all.into_iter().filter(|f| deliverables::split_version(&f.deliverable_path).0 == base_path).collect())
}

fn get(conn: &Connection, project: &str, thread: &str, id: &str) -> Result<FeedbackView> {
    query_opt(
        conn,
        &format!("SELECT {COLUMNS} FROM feedback WHERE project_slug = ?1 AND thread_slug = ?2 AND id = ?3"),
        [project, thread, id],
        from_row,
    )?
    .ok_or_else(|| Error::not_found("Feedback not found"))
}

/// Feedback on `deliverable_path`, which must exist (live, or a published "<name>@v1.0.<ext>").
pub fn add(conn: &Connection, project: &str, thread: &str, deliverable_path: &str, body: &str) -> Result<FeedbackView> {
    threads::require(conn, project, thread)?;
    let body = body.trim();
    if body.is_empty() {
        return Err(Error::invalid("Write the feedback first"));
    }
    if !deliverables::exists(conn, project, deliverable_path)? {
        return Err(Error::not_found("Deliverable not found"));
    }
    let version = deliverables::split_version(deliverable_path).1;

    let ids::Stamp { id, at } = ids::new_stamp();
    conn.execute(
        "INSERT INTO feedback (id, project_slug, thread_slug, deliverable_path, deliverable_version, created_at, updated_at, body)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7)",
        params![id, project, thread, deliverable_path, version, at, body],
    )?;
    get(conn, project, thread, &id)
}

pub fn set_status(conn: &Connection, project: &str, thread: &str, id: &str, status: &str) -> Result<FeedbackView> {
    if status != "open" && status != "resolved" {
        return Err(Error::invalid("Feedback is either open or resolved"));
    }
    let changed = conn.execute(
        "UPDATE feedback SET status = ?4, updated_at = ?5 WHERE project_slug = ?1 AND thread_slug = ?2 AND id = ?3",
        params![project, thread, id, status, ids::now()],
    )?;
    if changed == 0 {
        return Err(Error::not_found("Feedback not found"));
    }
    get(conn, project, thread, id)
}

pub fn update(conn: &Connection, project: &str, thread: &str, id: &str, body: &str) -> Result<FeedbackView> {
    let body = body.trim();
    if body.is_empty() {
        return Err(Error::invalid("Write the feedback first"));
    }
    let changed = conn.execute(
        "UPDATE feedback SET body = ?4, updated_at = ?5 WHERE project_slug = ?1 AND thread_slug = ?2 AND id = ?3",
        params![project, thread, id, body, ids::now()],
    )?;
    if changed == 0 {
        return Err(Error::not_found("Feedback not found"));
    }
    get(conn, project, thread, id)
}

pub fn delete(conn: &Connection, project: &str, thread: &str, id: &str) -> Result<()> {
    conn.execute("DELETE FROM feedback WHERE project_slug = ?1 AND thread_slug = ?2 AND id = ?3", [project, thread, id])?;
    Ok(())
}
