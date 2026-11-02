//! The in-memory database. It is not where anything is kept: the vault (a folder of markdown files, mirrored to
//! GitHub) is. At startup the vault is loaded into this database; reads are answered from it, and every write
//! goes through to the files (see `vault`). Using SQL for the working copy keeps the rules — unique ids, links
//! that can't dangle, guards on deleting things in use — in one declarative place.

use rusqlite::{Connection, OptionalExtension, Params, Row};

use crate::error::Result;

const SCHEMA: &str = include_str!("schema.sql");

/// A fresh database with the schema and the default planboard columns, ready to be filled from the vault.
pub fn open_in_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

pub fn query_all<T, P: Params>(
    conn: &Connection,
    sql: &str,
    params: P,
    map: impl FnMut(&Row<'_>) -> rusqlite::Result<T>,
) -> Result<Vec<T>> {
    let mut stmt = conn.prepare_cached(sql)?;
    let rows = stmt.query_map(params, map)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn query_opt<T, P: Params>(
    conn: &Connection,
    sql: &str,
    params: P,
    map: impl FnOnce(&Row<'_>) -> rusqlite::Result<T>,
) -> Result<Option<T>> {
    Ok(conn.query_row(sql, params, map).optional()?)
}
