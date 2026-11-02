//! The single global planboard: an ordered list of columns plus which of them is the "backlog" target.
//! Every user story is a card on it.

use rusqlite::{Connection, params};

use crate::db::{query_all, query_opt};
use crate::error::{Error, Result};
use crate::models::{Column, Planboard};
use crate::{ids, stories};

pub fn get(conn: &Connection) -> Result<Planboard> {
    Ok(Planboard {
        columns: columns(conn)?,
        backlog_column_id: backlog_column_id(conn)?,
        user_stories: stories::list(conn)?,
    })
}

pub fn columns(conn: &Connection) -> Result<Vec<Column>> {
    query_all(conn, "SELECT id, name FROM planboard_columns ORDER BY position, id", [], |r| {
        Ok(Column { id: r.get(0)?, name: r.get(1)? })
    })
}

pub fn column_exists(conn: &Connection, id: &str) -> Result<bool> {
    Ok(query_opt(conn, "SELECT 1 FROM planboard_columns WHERE id = ?1", [id], |_| Ok(()))?.is_some())
}

/// Falls back to the first column if the configured one was since deleted (ids are stable across renames,
/// so a rename never triggers this).
pub fn backlog_column_id(conn: &Connection) -> Result<String> {
    let configured = query_opt(conn, "SELECT value FROM settings WHERE key = 'backlog_column_id'", [], |r| r.get::<_, String>(0))?;
    if let Some(id) = configured.filter(|id| column_exists(conn, id).unwrap_or(false)) {
        return Ok(id);
    }
    columns(conn)?
        .into_iter()
        .next()
        .map(|c| c.id)
        .ok_or_else(|| Error::invalid("Planboard has no columns configured"))
}

pub fn add_column(conn: &Connection, name: &str) -> Result<Vec<Column>> {
    let name = require_name(name)?;
    let position: i64 = conn.query_row("SELECT COALESCE(MAX(position) + 1, 0) FROM planboard_columns", [], |r| r.get(0))?;
    conn.execute("INSERT INTO planboard_columns (id, name, position) VALUES (?1, ?2, ?3)", params![ids::new_id(), name, position])?;
    columns(conn)
}

pub fn rename_column(conn: &Connection, id: &str, name: &str) -> Result<Vec<Column>> {
    let name = require_name(name)?;
    if conn.execute("UPDATE planboard_columns SET name = ?2 WHERE id = ?1", [id, name.as_str()])? == 0 {
        return Err(Error::not_found("Column not found"));
    }
    columns(conn)
}

/// A column that still holds user stories can't be deleted, and the board always keeps at least one.
pub fn delete_column(conn: &Connection, id: &str) -> Result<Vec<Column>> {
    if !column_exists(conn, id)? {
        return Err(Error::not_found("Column not found"));
    }
    if stories::count_in_column(conn, id)? > 0 {
        return Err(Error::invalid("Move or delete this column's user stories first"));
    }
    if columns(conn)?.len() <= 1 {
        return Err(Error::invalid("The planboard needs at least one column"));
    }
    conn.execute("DELETE FROM planboard_columns WHERE id = ?1", [id])?;
    columns(conn)
}

fn require_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() { Err(Error::invalid("Column name is required")) } else { Ok(name.to_string()) }
}
