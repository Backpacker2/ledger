//! A project's board: an ordered list of columns (Backlog → Doing → Done by default) and the tasks sitting in them.

use rusqlite::{Connection, params};

use crate::db::{query_all, query_opt};
use crate::error::{Error, Result};
use crate::models::{Board, Column, StorySummary, TaskUpdate, TaskView};
use crate::{ids, projects, stories, threads};

pub fn seed_columns(conn: &Connection, project: &str) -> Result<()> {
    for (position, (id, name)) in [("backlog", "Backlog"), ("doing", "Doing"), ("done", "Done")].into_iter().enumerate() {
        conn.execute(
            "INSERT INTO project_columns (project_slug, id, name, position) VALUES (?1, ?2, ?3, ?4)",
            params![project, id, name, position as i64],
        )?;
    }
    conn.execute("UPDATE projects SET backlog_column_id = 'backlog' WHERE slug = ?1", [project])?;
    Ok(())
}

// ---- columns ----------------------------------------------------------------------------------------

pub fn columns(conn: &Connection, project: &str) -> Result<Vec<Column>> {
    query_all(conn, "SELECT id, name FROM project_columns WHERE project_slug = ?1 ORDER BY position, id", [project], |r| {
        Ok(Column { id: r.get(0)?, name: r.get(1)? })
    })
}

pub fn column_exists(conn: &Connection, project: &str, id: &str) -> Result<bool> {
    Ok(query_opt(conn, "SELECT 1 FROM project_columns WHERE project_slug = ?1 AND id = ?2", [project, id], |_| Ok(()))?.is_some())
}

/// Falls back to the first column if the configured one was since deleted (ids are stable across renames, so a
/// rename never triggers this).
pub fn backlog_column_id(conn: &Connection, project: &str) -> Result<String> {
    let configured = query_opt(conn, "SELECT backlog_column_id FROM projects WHERE slug = ?1", [project], |r| r.get::<_, String>(0))?
        .ok_or_else(|| Error::not_found("Project not found"))?;
    if column_exists(conn, project, &configured)? {
        return Ok(configured);
    }
    columns(conn, project)?
        .into_iter()
        .next()
        .map(|c| c.id)
        .ok_or_else(|| Error::invalid("The board has no columns"))
}

pub fn add_column(conn: &Connection, project: &str, name: &str) -> Result<Vec<Column>> {
    projects::require(conn, project)?;
    let name = require_name(name)?;
    let position: i64 = conn.query_row(
        "SELECT COALESCE(MAX(position) + 1, 0) FROM project_columns WHERE project_slug = ?1",
        [project],
        |r| r.get(0),
    )?;
    conn.execute(
        "INSERT INTO project_columns (project_slug, id, name, position) VALUES (?1, ?2, ?3, ?4)",
        params![project, ids::new_id(), name, position],
    )?;
    columns(conn, project)
}

pub fn rename_column(conn: &Connection, project: &str, id: &str, name: &str) -> Result<Vec<Column>> {
    let name = require_name(name)?;
    if conn.execute("UPDATE project_columns SET name = ?3 WHERE project_slug = ?1 AND id = ?2", [project, id, name.as_str()])? == 0 {
        return Err(Error::not_found("Column not found"));
    }
    columns(conn, project)
}

/// A column that still holds tasks can't be deleted, and the board always keeps at least one.
pub fn delete_column(conn: &Connection, project: &str, id: &str) -> Result<Vec<Column>> {
    if !column_exists(conn, project, id)? {
        return Err(Error::not_found("Column not found"));
    }
    let tasks: i64 = conn.query_row("SELECT COUNT(*) FROM tasks WHERE project_slug = ?1 AND column_id = ?2", [project, id], |r| r.get(0))?;
    if tasks > 0 {
        return Err(Error::invalid("Move or delete this column's tasks first"));
    }
    if columns(conn, project)?.len() <= 1 {
        return Err(Error::invalid("The board needs at least one column"));
    }
    conn.execute("DELETE FROM project_columns WHERE project_slug = ?1 AND id = ?2", [project, id])?;
    columns(conn, project)
}

fn require_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() { Err(Error::invalid("Column name is required")) } else { Ok(name.to_string()) }
}

// ---- tasks ------------------------------------------------------------------------------------------

const TASK_SQL: &str = "
    SELECT t.id, t.title, t.body, t.column_id, s.id, s.summary, t.thread_slug, th.title, t.created_at, t.updated_at
    FROM tasks t
    LEFT JOIN user_stories s ON s.project_slug = t.project_slug AND s.id = t.story_id
    LEFT JOIN threads th ON th.project_slug = t.project_slug AND th.slug = t.thread_slug
    WHERE t.project_slug = ?1";

fn task_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<TaskView> {
    Ok(TaskView {
        id: r.get(0)?,
        title: r.get(1)?,
        body: r.get(2)?,
        column_id: r.get(3)?,
        story_id: r.get(4)?,
        story_summary: r.get(5)?,
        thread_slug: r.get(6)?,
        thread_title: r.get(7)?,
        created_at: r.get(8)?,
        updated_at: r.get(9)?,
    })
}

/// Oldest first, which is the order cards sit in within a column.
pub fn tasks(conn: &Connection, project: &str) -> Result<Vec<TaskView>> {
    query_all(conn, &format!("{TASK_SQL} ORDER BY t.created_at, t.id"), [project], task_from_row)
}

pub fn task(conn: &Connection, project: &str, id: &str) -> Result<TaskView> {
    query_opt(conn, &format!("{TASK_SQL} AND t.id = ?2"), [project, id], task_from_row)?.ok_or_else(|| Error::not_found("Task not found"))
}

/// Tasks not in the board's last column.
pub fn open_task_count(conn: &Connection, project: &str) -> Result<i64> {
    let last = columns(conn, project)?.pop().map(|c| c.id).unwrap_or_default();
    Ok(conn.query_row("SELECT COUNT(*) FROM tasks WHERE project_slug = ?1 AND column_id <> ?2", [project, last.as_str()], |r| r.get(0))?)
}

pub fn get(conn: &Connection, project: &str) -> Result<Board> {
    projects::require(conn, project)?;
    Ok(Board {
        columns: columns(conn, project)?,
        backlog_column_id: backlog_column_id(conn, project)?,
        tasks: tasks(conn, project)?,
        stories: stories::list(conn, project)?.into_iter().map(|s| StorySummary { id: s.id, summary: s.summary }).collect(),
    })
}

/// A new task lands in the backlog column unless told otherwise.
pub fn create_task(conn: &Connection, project: &str, title: &str, body: &str, story: Option<&str>, thread: Option<&str>) -> Result<TaskView> {
    projects::require(conn, project)?;
    let title = require_title(title)?;
    let story = canonical_story(conn, project, story)?;
    let thread = checked_thread(conn, project, thread)?;

    let ids::Stamp { id, at } = ids::new_stamp();
    conn.execute(
        "INSERT INTO tasks (id, project_slug, title, body, column_id, story_id, thread_slug, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
        params![id, project, title, body.trim(), backlog_column_id(conn, project)?, story, thread, at],
    )?;
    task(conn, project, &id)
}

pub fn update_task(conn: &Connection, project: &str, id: &str, updates: TaskUpdate) -> Result<TaskView> {
    task(conn, project, id)?; // not found if it does not exist

    let tx = conn.unchecked_transaction()?;
    if let Some(title) = &updates.title {
        tx.execute("UPDATE tasks SET title = ?2 WHERE id = ?1", [id, require_title(title)?.as_str()])?;
    }
    if let Some(body) = &updates.body {
        tx.execute("UPDATE tasks SET body = ?2 WHERE id = ?1", [id, body.trim()])?;
    }
    if let Some(column) = &updates.column_id {
        if !column_exists(&tx, project, column)? {
            return Err(Error::not_found("Column not found"));
        }
        tx.execute("UPDATE tasks SET column_id = ?2 WHERE id = ?1", [id, column.as_str()])?;
    }
    if updates.story_id.is_some() {
        let story = canonical_story(&tx, project, updates.story_id.as_deref())?;
        tx.execute("UPDATE tasks SET story_id = ?2 WHERE id = ?1", params![id, story])?;
    }
    if updates.thread_slug.is_some() {
        let thread = checked_thread(&tx, project, updates.thread_slug.as_deref())?;
        tx.execute("UPDATE tasks SET thread_slug = ?2 WHERE id = ?1", params![id, thread])?;
    }
    tx.execute("UPDATE tasks SET updated_at = ?2 WHERE id = ?1", [id, &ids::now()])?;
    tx.commit()?;
    task(conn, project, id)
}

pub fn delete_task(conn: &Connection, project: &str, id: &str) -> Result<()> {
    conn.execute("DELETE FROM tasks WHERE project_slug = ?1 AND id = ?2", [project, id])?;
    Ok(())
}

fn require_title(title: &str) -> Result<String> {
    let title = title.trim();
    if title.is_empty() { Err(Error::invalid("Title is required")) } else { Ok(title.to_string()) }
}

/// The story's canonical id, `None` for "no story" (absent or empty).
fn canonical_story(conn: &Connection, project: &str, story: Option<&str>) -> Result<Option<String>> {
    match story.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(id) => Ok(Some(stories::get(conn, project, id)?.id)),
    }
}

fn checked_thread(conn: &Connection, project: &str, thread: Option<&str>) -> Result<Option<String>> {
    match thread.map(str::trim).filter(|t| !t.is_empty()) {
        None => Ok(None),
        Some(slug) => {
            threads::require(conn, project, slug)?;
            Ok(Some(slug.to_string()))
        }
    }
}
