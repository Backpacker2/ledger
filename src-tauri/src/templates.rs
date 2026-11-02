//! Templates shape deliverables: a name, a "use when…" hint, guiding questions to answer first, and the skeleton
//! of the deliverable itself. A vault-wide template (`templates/<id>.md`, scope "") is available in every project;
//! a project's own (`projects/<p>/templates/<id>.md`) is available there, and replaces a vault-wide one with the
//! same id.

use rusqlite::{Connection, params};

use crate::db::{query_all, query_opt};
use crate::error::{Error, Result};
use crate::models::TemplateView;
use crate::{ids, projects};

const SELECT: &str = "SELECT scope, id, name, \"trigger\", body, created_at, updated_at FROM templates";

fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<TemplateView> {
    Ok(TemplateView {
        scope: r.get(0)?,
        id: r.get(1)?,
        name: r.get(2)?,
        trigger: r.get(3)?,
        body: r.get(4)?,
        created_at: r.get(5)?,
        updated_at: r.get(6)?,
        questions: vec![],
        overridden: false,
    })
}

fn with_questions(conn: &Connection, mut templates: Vec<TemplateView>) -> Result<Vec<TemplateView>> {
    for t in &mut templates {
        t.questions = questions_of(conn, &t.scope, &t.id)?;
    }
    Ok(templates)
}

pub fn questions_of(conn: &Connection, scope: &str, id: &str) -> Result<Vec<String>> {
    query_all(conn, "SELECT question FROM template_questions WHERE scope = ?1 AND template_id = ?2 ORDER BY position", [scope, id], |r| r.get(0))
}

/// The templates of exactly this scope ("" for the vault-wide ones), by name.
pub fn list(conn: &Connection, scope: &str) -> Result<Vec<TemplateView>> {
    let rows = query_all(conn, &format!("{SELECT} WHERE scope = ?1 ORDER BY name COLLATE NOCASE, id"), [scope], from_row)?;
    with_questions(conn, rows)
}

/// What the project's templates screen shows: its own, then the vault-wide ones (marked if the project replaced them).
pub fn for_project(conn: &Connection, project: &str) -> Result<Vec<TemplateView>> {
    let own = list(conn, project)?;
    let mut inherited = list(conn, "")?;
    for t in &mut inherited {
        t.overridden = own.iter().any(|o| o.id == t.id);
    }
    Ok(own.into_iter().chain(inherited).collect())
}

/// What a prompt in the project can start from: its own templates and the vault-wide ones it hasn't replaced.
pub fn effective(conn: &Connection, project: &str) -> Result<Vec<TemplateView>> {
    let mut all: Vec<TemplateView> = for_project(conn, project)?.into_iter().filter(|t| !t.overridden).collect();
    all.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(all)
}

pub fn get(conn: &Connection, scope: &str, id: &str) -> Result<TemplateView> {
    let row = query_opt(conn, &format!("{SELECT} WHERE scope = ?1 AND id = ?2"), [scope, id], from_row)?
        .ok_or_else(|| Error::not_found("Template not found"))?;
    Ok(with_questions(conn, vec![row])?.remove(0))
}

/// The template a project's prompt means by `id`: its own, else the vault-wide one.
pub fn find(conn: &Connection, project: &str, id: &str) -> Result<TemplateView> {
    get(conn, project, id).or_else(|_| get(conn, "", id))
}

pub fn name_of(conn: &Connection, project: &str, id: &str) -> Result<Option<String>> {
    Ok(find(conn, project, id).ok().map(|t| t.name))
}

pub fn create(conn: &Connection, scope: &str, name: &str, trigger: &str, questions: &[String], body: &str) -> Result<TemplateView> {
    if !scope.is_empty() {
        projects::require(conn, scope)?;
    }
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::invalid("Name is required"));
    }
    let id = unique_id(conn, scope, name)?;
    let now = ids::now();

    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO templates (scope, id, name, \"trigger\", body, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
        params![scope, id, name, trigger.trim(), body.trim_end(), now],
    )?;
    write_questions(&tx, scope, &id, questions)?;
    tx.commit()?;
    get(conn, scope, &id)
}

/// Copies a vault-wide template into the project under the same id, so the project can reword it for itself.
pub fn override_global(conn: &Connection, project: &str, id: &str) -> Result<TemplateView> {
    projects::require(conn, project)?;
    let global = get(conn, "", id)?;
    if get(conn, project, id).is_ok() {
        return Err(Error::invalid("This project already has its own version of that template"));
    }
    let now = ids::now();

    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO templates (scope, id, name, \"trigger\", body, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
        params![project, id, global.name, global.trigger, global.body, now],
    )?;
    write_questions(&tx, project, id, &global.questions)?;
    tx.commit()?;
    get(conn, project, id)
}

pub fn update(
    conn: &Connection,
    scope: &str,
    id: &str,
    name: Option<&str>,
    trigger: Option<&str>,
    questions: Option<&[String]>,
    body: Option<&str>,
) -> Result<TemplateView> {
    get(conn, scope, id)?; // not found if it does not exist

    let tx = conn.unchecked_transaction()?;
    if let Some(name) = name {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::invalid("Name is required"));
        }
        tx.execute("UPDATE templates SET name = ?3 WHERE scope = ?1 AND id = ?2", [scope, id, name])?;
    }
    if let Some(trigger) = trigger {
        tx.execute("UPDATE templates SET \"trigger\" = ?3 WHERE scope = ?1 AND id = ?2", [scope, id, trigger.trim()])?;
    }
    if let Some(body) = body {
        tx.execute("UPDATE templates SET body = ?3 WHERE scope = ?1 AND id = ?2", [scope, id, body.trim_end()])?;
    }
    if let Some(questions) = questions {
        write_questions(&tx, scope, id, questions)?;
    }
    tx.execute("UPDATE templates SET updated_at = ?3 WHERE scope = ?1 AND id = ?2", [scope, id, &ids::now()])?;
    tx.commit()?;
    get(conn, scope, id)
}

/// Prompts that were started from a template keep what they were given, so a template can always be deleted.
pub fn delete(conn: &Connection, scope: &str, id: &str) -> Result<()> {
    conn.execute("DELETE FROM templates WHERE scope = ?1 AND id = ?2", [scope, id])?;
    Ok(())
}

/// Blank questions are dropped; the order the caller gave is kept.
fn write_questions(conn: &Connection, scope: &str, id: &str, questions: &[String]) -> Result<()> {
    conn.execute("DELETE FROM template_questions WHERE scope = ?1 AND template_id = ?2", [scope, id])?;
    let cleaned = questions.iter().map(|q| q.trim()).filter(|q| !q.is_empty());
    for (position, question) in cleaned.enumerate() {
        conn.execute(
            "INSERT INTO template_questions (scope, template_id, position, question) VALUES (?1, ?2, ?3, ?4)",
            params![scope, id, position as i64, question],
        )?;
    }
    Ok(())
}

fn unique_id(conn: &Connection, scope: &str, name: &str) -> Result<String> {
    let base = ids::slugify_as(name, "template");
    let mut id = base.clone();
    let mut n = 2;
    while query_opt(conn, "SELECT 1 FROM templates WHERE scope = ?1 AND id = ?2", [scope, &id], |_| Ok(()))?.is_some() {
        id = format!("{base}-{n}");
        n += 1;
    }
    Ok(id)
}
