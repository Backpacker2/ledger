use rusqlite::{Connection, params};

use crate::db::{query_all, query_opt};
use crate::error::{Error, Result};
use crate::models::{ProjectSummary, ProjectView};
use crate::{board, deliverables, ids, markdown, threads};

pub fn exists(conn: &Connection, slug: &str) -> Result<bool> {
    Ok(query_opt(conn, "SELECT 1 FROM projects WHERE slug = ?1", [slug], |_| Ok(()))?.is_some())
}

pub fn require(conn: &Connection, slug: &str) -> Result<()> {
    if exists(conn, slug)? { Ok(()) } else { Err(Error::not_found("Project not found")) }
}

/// The reading guide a new project starts with.
pub fn starter_readme(name: &str) -> String {
    format!(
        "# {name}\n\nStart here: this is the reading guide for the project. Say what it is for, how its threads, \
         deliverables and stories fit together, and where to look first.\n"
    )
}

/// Every project, most recently touched first, each with its threads.
pub fn list(conn: &Connection) -> Result<Vec<ProjectSummary>> {
    let mut projects = query_all(
        conn,
        "SELECT slug, name, created_at, updated_at,
                (SELECT COUNT(*) FROM user_stories s WHERE s.project_slug = p.slug),
                (SELECT COUNT(*) FROM tasks t WHERE t.project_slug = p.slug)
         FROM projects p ORDER BY name COLLATE NOCASE, slug",
        [],
        |r| {
            Ok(ProjectSummary {
                slug: r.get(0)?,
                name: r.get(1)?,
                created_at: r.get(2)?,
                updated_at: r.get(3)?,
                threads: vec![],
                deliverable_count: 0,
                story_count: r.get(4)?,
                task_count: r.get(5)?,
            })
        },
    )?;
    for project in &mut projects {
        project.threads = threads::list(conn, &project.slug)?;
        project.deliverable_count = deliverables::count_live(conn, &project.slug)?;
    }
    Ok(projects)
}

fn summary(conn: &Connection, slug: &str) -> Result<ProjectSummary> {
    list(conn)?.into_iter().find(|p| p.slug == slug).ok_or_else(|| Error::not_found("Project not found"))
}

pub fn create(conn: &Connection, name: &str) -> Result<ProjectSummary> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::invalid("Name is required"));
    }
    let slug = unique_slug(conn, name)?;
    let now = ids::now();

    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO projects (slug, name, readme, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)",
        params![slug, name, starter_readme(name), now],
    )?;
    board::seed_columns(&tx, &slug)?;
    tx.commit()?;
    summary(conn, &slug)
}

pub fn rename(conn: &Connection, slug: &str, name: &str) -> Result<ProjectSummary> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::invalid("Name is required"));
    }
    if conn.execute("UPDATE projects SET name = ?2, updated_at = ?3 WHERE slug = ?1", params![slug, name, ids::now()])? == 0 {
        return Err(Error::not_found("Project not found"));
    }
    summary(conn, slug)
}

pub fn get(conn: &Connection, slug: &str) -> Result<ProjectView> {
    let (name, description, readme) = query_opt(conn, "SELECT name, description, readme FROM projects WHERE slug = ?1", [slug], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
    })?
    .ok_or_else(|| Error::not_found("Project not found"))?;

    let count = |sql: &str| -> Result<i64> { Ok(conn.query_row(sql, [slug], |r| r.get(0))?) };
    let backlog_done = board::open_task_count(conn, slug)?;
    Ok(ProjectView {
        slug: slug.to_string(),
        name,
        description,
        readme_html: markdown::render(&readme),
        readme,
        thread_count: count("SELECT COUNT(*) FROM threads WHERE project_slug = ?1")?,
        deliverable_count: deliverables::count_live(conn, slug)?,
        story_count: count("SELECT COUNT(*) FROM user_stories WHERE project_slug = ?1")?,
        task_count: count("SELECT COUNT(*) FROM tasks WHERE project_slug = ?1")?,
        open_task_count: backlog_done,
    })
}

/// Replaces the reading guide, verbatim.
pub fn save_readme(conn: &Connection, slug: &str, text: &str) -> Result<ProjectView> {
    if conn.execute("UPDATE projects SET readme = ?2, updated_at = ?3 WHERE slug = ?1", params![slug, text, ids::now()])? == 0 {
        return Err(Error::not_found("Project not found"));
    }
    get(conn, slug)
}

/// Deletes the project with everything in it. The folder's other files (attachments, non-markdown deliverables)
/// are the caller's to clear.
pub fn delete(conn: &Connection, slug: &str) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM templates WHERE scope = ?1", [slug])?;
    tx.execute("DELETE FROM projects WHERE slug = ?1", [slug])?;
    tx.commit()?;
    Ok(())
}

fn unique_slug(conn: &Connection, name: &str) -> Result<String> {
    let base = ids::slugify_as(name, "project");
    let mut slug = base.clone();
    let mut n = 2;
    while exists(conn, &slug)? {
        slug = format!("{base}-{n}");
        n += 1;
    }
    Ok(slug)
}
