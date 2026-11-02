use rusqlite::{Connection, Row, params};

use crate::db::{query_all, query_opt};
use crate::error::{Error, Result};
use crate::models::PostView;
use crate::{ids, markdown, threads};

const COLUMNS: &str =
    "id, parent_id, guiding_question, version_published, document_id, document_version, created_at, updated_at, body";

#[derive(Debug, Default)]
pub struct AddOptions {
    pub reply_to_id: Option<String>,
    pub guiding_question: bool,
    pub version_published: bool,
    pub document_id: Option<String>,
    pub document_version: Option<String>,
}

fn from_row(r: &Row<'_>) -> rusqlite::Result<PostView> {
    let body: String = r.get(8)?;
    Ok(PostView {
        id: r.get(0)?,
        parent_id: r.get(1)?,
        guiding_question: r.get(2)?,
        version_published: r.get(3)?,
        document_id: r.get(4)?,
        document_version: r.get(5)?,
        created_at: r.get(6)?,
        updated_at: r.get(7)?,
        html: markdown::render(&body),
        body,
    })
}

/// Oldest first. Posts created in the same millisecond keep the order they were inserted in.
pub fn list(conn: &Connection, slug: &str) -> Result<Vec<PostView>> {
    query_all(
        conn,
        &format!("SELECT {COLUMNS} FROM posts WHERE thread_slug = ?1 ORDER BY created_at, rowid"),
        [slug],
        from_row,
    )
}

fn get(conn: &Connection, slug: &str, id: &str) -> Result<PostView> {
    query_opt(conn, &format!("SELECT {COLUMNS} FROM posts WHERE thread_slug = ?1 AND id = ?2"), [slug, id], from_row)?
        .ok_or_else(|| Error::not_found("Post not found"))
}

pub fn add(conn: &Connection, slug: &str, body: &str, options: AddOptions) -> Result<PostView> {
    let body = body.trim();
    if body.is_empty() {
        return Err(Error::invalid("Post body is required"));
    }
    threads::require(conn, slug)?;

    let parent_id = match options.reply_to_id.as_deref().filter(|id| !id.is_empty()) {
        None => None,
        Some(parent) => {
            let found = query_opt(conn, "SELECT 1 FROM posts WHERE thread_slug = ?1 AND id = ?2", [slug, parent], |_| Ok(()))?;
            if found.is_none() {
                return Err(Error::not_found("Post being replied to was not found"));
            }
            Some(parent.to_string())
        }
    };

    let id = ids::new_id();
    let now = ids::now();
    conn.execute(
        "INSERT INTO posts (id, thread_slug, parent_id, guiding_question, version_published, document_id,
                            document_version, created_at, updated_at, body)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?9)",
        params![
            id,
            slug,
            parent_id,
            options.guiding_question,
            options.version_published,
            options.document_id,
            options.document_version,
            now,
            body
        ],
    )?;
    get(conn, slug, &id)
}

pub fn update(conn: &Connection, slug: &str, id: &str, body: &str) -> Result<PostView> {
    let body = body.trim();
    if body.is_empty() {
        return Err(Error::invalid("Post body is required"));
    }
    let changed = conn.execute(
        "UPDATE posts SET body = ?3, updated_at = ?4 WHERE thread_slug = ?1 AND id = ?2",
        params![slug, id, body, ids::now()],
    )?;
    if changed == 0 {
        return Err(Error::not_found("Post not found"));
    }
    get(conn, slug, id)
}

/// Replies to a deleted post stay; they just lose the thing they were quoting.
pub fn delete(conn: &Connection, slug: &str, id: &str) -> Result<()> {
    conn.execute("DELETE FROM posts WHERE thread_slug = ?1 AND id = ?2", [slug, id])?;
    Ok(())
}
