//! A prompt is a request for a deliverable, written in a thread. Whatever produces the deliverable (a person, or
//! an assistant reading the vault) leaves the file in the project's deliverables folder; the prompt keeps the
//! list of the deliverables that came out of it.

use std::collections::HashSet;

use rusqlite::{Connection, Row, params};

use crate::claude_code::records;
use crate::db::{query_all, query_opt};
use crate::error::{Error, Result};
use crate::models::{DeliverablePrompt, PromptDeliverable, PromptView};
use crate::posts::{self, AddOptions};
use crate::{deliverables, ids, markdown, templates, threads};

const COLUMNS: &str = "id, thread_slug, title, template_id, created_at, updated_at, body";

fn from_row(r: &Row<'_>) -> rusqlite::Result<PromptView> {
    let body: String = r.get(6)?;
    Ok(PromptView {
        id: r.get(0)?,
        thread_slug: r.get(1)?,
        title: r.get(2)?,
        template_id: r.get(3)?,
        template_name: None,
        created_at: r.get(4)?,
        updated_at: r.get(5)?,
        html: markdown::render(&body),
        body,
        deliverables: vec![],
        runs: vec![],
    })
}

/// Fills in what lives in other tables: the template's name and the deliverables.
fn complete(conn: &Connection, project: &str, mut prompt: PromptView) -> Result<PromptView> {
    prompt.template_name = prompt.template_id.as_deref().and_then(|id| templates::name_of(conn, project, id).ok().flatten());
    let paths: Vec<String> = query_all(conn, "SELECT path FROM prompt_deliverables WHERE prompt_id = ?1 ORDER BY position", [&prompt.id], |r| r.get(0))?;
    for path in paths {
        let exists = deliverables::exists(conn, project, &path)?;
        prompt.deliverables.push(PromptDeliverable { path, exists });
    }
    prompt.runs = records::for_prompt(conn, project, &prompt.thread_slug, &prompt.id)?;
    Ok(prompt)
}

/// Oldest first.
pub fn list(conn: &Connection, project: &str, thread: &str) -> Result<Vec<PromptView>> {
    let rows = query_all(
        conn,
        &format!("SELECT {COLUMNS} FROM prompts WHERE project_slug = ?1 AND thread_slug = ?2 ORDER BY created_at, rowid"),
        [project, thread],
        from_row,
    )?;
    rows.into_iter().map(|p| complete(conn, project, p)).collect()
}

pub fn get(conn: &Connection, project: &str, thread: &str, id: &str) -> Result<PromptView> {
    let prompt = query_opt(
        conn,
        &format!("SELECT {COLUMNS} FROM prompts WHERE project_slug = ?1 AND thread_slug = ?2 AND id = ?3"),
        [project, thread, id],
        from_row,
    )?
    .ok_or_else(|| Error::not_found("Prompt not found"))?;
    complete(conn, project, prompt)
}

/// Writes the request. Started from a template, the template's guiding questions are added to the thread as posts
/// to be answered — a one-time snapshot, so editing the template later never reaches back into the thread.
pub fn create(conn: &Connection, project: &str, thread: &str, title: &str, body: &str, template_id: Option<&str>) -> Result<PromptView> {
    threads::require(conn, project, thread)?;
    let body = body.trim();
    if body.is_empty() {
        return Err(Error::invalid("Say what you want delivered"));
    }
    let template = match template_id.map(str::trim).filter(|t| !t.is_empty()) {
        Some(id) => Some(templates::find(conn, project, id).map_err(|_| Error::invalid("Template not found"))?),
        None => None,
    };

    let tx = conn.unchecked_transaction()?;
    if let Some(template) = &template {
        for question in &template.questions {
            posts::add(
                &tx,
                project,
                thread,
                &format!("**Guiding question ({}):** {question}", template.name),
                AddOptions { guiding_question: true, ..Default::default() },
            )?;
        }
    }
    let ids::Stamp { id, at } = ids::new_stamp();
    tx.execute(
        "INSERT INTO prompts (id, project_slug, thread_slug, title, template_id, created_at, updated_at, body)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7)",
        params![id, project, thread, title.trim(), template.as_ref().map(|t| t.id.as_str()), at, body],
    )?;
    tx.commit()?;
    get(conn, project, thread, &id)
}

/// Writes the request, and when `deliverable` is given links that existing deliverable to it. A deliverable that isn't
/// there is refused before anything is written, so no prompt is left half made. With none, the prompt asks for a new
/// deliverable: nothing is linked until one is produced.
pub fn create_linked(
    conn: &Connection,
    project: &str,
    thread: &str,
    title: &str,
    body: &str,
    template_id: Option<&str>,
    deliverable: Option<&str>,
) -> Result<PromptView> {
    let deliverable = deliverable.map(str::trim).filter(|d| !d.is_empty());
    if let Some(path) = deliverable
        && !deliverables::exists(conn, project, path)?
    {
        return Err(Error::not_found("Deliverable not found"));
    }
    let prompt = create(conn, project, thread, title, body, template_id)?;
    match deliverable {
        Some(path) => link_deliverable(conn, project, thread, &prompt.id, path),
        None => Ok(prompt),
    }
}

pub fn update(conn: &Connection, project: &str, thread: &str, id: &str, title: Option<&str>, body: Option<&str>) -> Result<PromptView> {
    get(conn, project, thread, id)?; // not found if it does not exist
    if let Some(title) = title {
        conn.execute("UPDATE prompts SET title = ?2 WHERE id = ?1", [id, title.trim()])?;
    }
    if let Some(body) = body {
        let body = body.trim();
        if body.is_empty() {
            return Err(Error::invalid("Say what you want delivered"));
        }
        conn.execute("UPDATE prompts SET body = ?2 WHERE id = ?1", [id, body])?;
    }
    conn.execute("UPDATE prompts SET updated_at = ?2 WHERE id = ?1", [id, &ids::now()])?;
    get(conn, project, thread, id)
}

pub fn delete(conn: &Connection, project: &str, thread: &str, id: &str) -> Result<()> {
    conn.execute("DELETE FROM prompts WHERE project_slug = ?1 AND thread_slug = ?2 AND id = ?3", [project, thread, id])?;
    Ok(())
}

/// Records that `path` (a deliverable that exists) came out of this prompt.
pub fn link_deliverable(conn: &Connection, project: &str, thread: &str, id: &str, path: &str) -> Result<PromptView> {
    get(conn, project, thread, id)?;
    if !deliverables::exists(conn, project, path)? {
        return Err(Error::not_found("Deliverable not found"));
    }
    conn.execute(
        "INSERT OR IGNORE INTO prompt_deliverables (prompt_id, position, path)
         VALUES (?1, (SELECT COALESCE(MAX(position) + 1, 0) FROM prompt_deliverables WHERE prompt_id = ?1), ?2)",
        [id, path],
    )?;
    get(conn, project, thread, id)
}

pub fn unlink_deliverable(conn: &Connection, project: &str, thread: &str, id: &str, path: &str) -> Result<PromptView> {
    conn.execute("DELETE FROM prompt_deliverables WHERE prompt_id = ?1 AND path = ?2", [id, path])?;
    get(conn, project, thread, id)
}

/// The prompts that produced a deliverable (given by its live path), oldest first. A prompt counts when it is linked
/// to the file or to one of its published versions: a run links the version it published.
pub fn producing(conn: &Connection, project: &str, path: &str) -> Result<Vec<DeliverablePrompt>> {
    let linked = query_all(
        conn,
        "SELECT p.thread_slug, t.title, p.id, p.title, d.path
         FROM prompt_deliverables d
         JOIN prompts p ON p.id = d.prompt_id
         JOIN threads t ON t.project_slug = p.project_slug AND t.slug = p.thread_slug
         WHERE p.project_slug = ?1
         ORDER BY p.created_at, p.rowid",
        [project],
        |r| Ok((DeliverablePrompt { thread_slug: r.get(0)?, thread_title: r.get(1)?, id: r.get(2)?, title: r.get(3)? }, r.get::<_, String>(4)?)),
    )?;
    let mut seen = HashSet::new();
    Ok(linked.into_iter().filter(|(prompt, linked)| deliverables::split_version(linked).0 == path && seen.insert(prompt.id.clone())).map(|(prompt, _)| prompt).collect())
}
