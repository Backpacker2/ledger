use rusqlite::{Connection, Row, params};

use crate::db::{query_all, query_opt};
use crate::error::{Error, Result};
use crate::models::PostView;
use crate::{ids, markdown, prompts, threads};

const COLUMNS: &str =
    "id, parent_id, guiding_question, version_published, deliverable_path, deliverable_version, prompt_id, created_at, updated_at, body";

#[derive(Debug, Default)]
pub struct AddOptions {
    pub reply_to_id: Option<String>,
    pub guiding_question: bool,
    pub version_published: bool,
    pub deliverable_path: Option<String>,
    pub deliverable_version: Option<String>,
    /// The prompt that caused the version a "version published" note announces.
    pub prompt_id: Option<String>,
}

fn from_row(r: &Row<'_>) -> rusqlite::Result<PostView> {
    let body: String = r.get(9)?;
    Ok(PostView {
        id: r.get(0)?,
        parent_id: r.get(1)?,
        guiding_question: r.get(2)?,
        version_published: r.get(3)?,
        deliverable_path: r.get(4)?,
        deliverable_version: r.get(5)?,
        prompt_id: r.get(6)?,
        prompt_title: None,
        prompt_thread: None,
        created_at: r.get(7)?,
        updated_at: r.get(8)?,
        html: markdown::render(&body),
        body,
    })
}

/// Names the prompt a note points at, when it still exists.
fn complete(conn: &Connection, project: &str, mut post: PostView) -> Result<PostView> {
    if let Some(id) = &post.prompt_id {
        if let Some((thread, title)) = prompts::locate(conn, project, id)? {
            post.prompt_thread = Some(thread);
            post.prompt_title = Some(title);
        }
    }
    Ok(post)
}

/// Oldest first. Posts created in the same millisecond keep the order they were inserted in.
pub fn list(conn: &Connection, project: &str, thread: &str) -> Result<Vec<PostView>> {
    let rows = query_all(
        conn,
        &format!("SELECT {COLUMNS} FROM posts WHERE project_slug = ?1 AND thread_slug = ?2 ORDER BY created_at, rowid"),
        [project, thread],
        from_row,
    )?;
    rows.into_iter().map(|p| complete(conn, project, p)).collect()
}

fn get(conn: &Connection, project: &str, thread: &str, id: &str) -> Result<PostView> {
    let post = query_opt(
        conn,
        &format!("SELECT {COLUMNS} FROM posts WHERE project_slug = ?1 AND thread_slug = ?2 AND id = ?3"),
        [project, thread, id],
        from_row,
    )?
    .ok_or_else(|| Error::not_found("Post not found"))?;
    complete(conn, project, post)
}

pub fn add(conn: &Connection, project: &str, thread: &str, body: &str, options: AddOptions) -> Result<PostView> {
    let body = body.trim();
    if body.is_empty() {
        return Err(Error::invalid("Post body is required"));
    }
    threads::require(conn, project, thread)?;

    let parent_id = match options.reply_to_id.as_deref().filter(|id| !id.is_empty()) {
        None => None,
        Some(parent) => {
            let found = query_opt(
                conn,
                "SELECT 1 FROM posts WHERE project_slug = ?1 AND thread_slug = ?2 AND id = ?3",
                [project, thread, parent],
                |_| Ok(()),
            )?;
            if found.is_none() {
                return Err(Error::not_found("Post being replied to was not found"));
            }
            Some(parent.to_string())
        }
    };

    let ids::Stamp { id, at: now } = ids::new_stamp();
    conn.execute(
        "INSERT INTO posts (id, project_slug, thread_slug, parent_id, guiding_question, version_published,
                            deliverable_path, deliverable_version, prompt_id, created_at, updated_at, body)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, ?11)",
        params![
            id,
            project,
            thread,
            parent_id,
            options.guiding_question,
            options.version_published,
            options.deliverable_path,
            options.deliverable_version,
            options.prompt_id,
            now,
            body
        ],
    )?;
    get(conn, project, thread, &id)
}

pub fn update(conn: &Connection, project: &str, thread: &str, id: &str, body: &str) -> Result<PostView> {
    let body = body.trim();
    if body.is_empty() {
        return Err(Error::invalid("Post body is required"));
    }
    let changed = conn.execute(
        "UPDATE posts SET body = ?4, updated_at = ?5 WHERE project_slug = ?1 AND thread_slug = ?2 AND id = ?3",
        params![project, thread, id, body, ids::now()],
    )?;
    if changed == 0 {
        return Err(Error::not_found("Post not found"));
    }
    get(conn, project, thread, id)
}

/// Replies to a deleted post stay; they just lose the thing they were quoting.
pub fn delete(conn: &Connection, project: &str, thread: &str, id: &str) -> Result<()> {
    conn.execute("DELETE FROM posts WHERE project_slug = ?1 AND thread_slug = ?2 AND id = ?3", [project, thread, id])?;
    Ok(())
}
