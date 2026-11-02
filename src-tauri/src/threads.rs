use std::collections::HashMap;
use std::path::Path;

use rusqlite::{Connection, params};

use crate::db::{query_all, query_opt};
use crate::error::{Error, Result};
use crate::models::{LinkedThread, ThreadSummary, ThreadView};
use crate::{feedback, ids, posts, projects, prompts, stories, uploads};

const SUMMARY_SQL: &str = "
    SELECT t.project_slug, t.slug, t.title, t.created_at,
           (SELECT COUNT(*) FROM posts x WHERE x.project_slug = t.project_slug AND x.thread_slug = t.slug),
           (SELECT COUNT(*) FROM prompts x WHERE x.project_slug = t.project_slug AND x.thread_slug = t.slug),
           (SELECT COUNT(*) FROM feedback x WHERE x.project_slug = t.project_slug AND x.thread_slug = t.slug),
           (SELECT COUNT(*) FROM uploads x WHERE x.project_slug = t.project_slug AND x.thread_slug = t.slug),
           (SELECT COUNT(*) FROM story_threads x WHERE x.project_slug = t.project_slug AND x.thread_slug = t.slug),
           MAX(t.created_at,
               COALESCE((SELECT MAX(created_at) FROM posts x WHERE x.project_slug = t.project_slug AND x.thread_slug = t.slug), ''),
               COALESCE((SELECT MAX(created_at) FROM prompts x WHERE x.project_slug = t.project_slug AND x.thread_slug = t.slug), ''),
               COALESCE((SELECT MAX(created_at) FROM feedback x WHERE x.project_slug = t.project_slug AND x.thread_slug = t.slug), ''),
               COALESCE((SELECT MAX(uploaded_at) FROM uploads x WHERE x.project_slug = t.project_slug AND x.thread_slug = t.slug), ''))
    FROM threads t
    WHERE t.project_slug = ?1 AND (?2 IS NULL OR t.slug = ?2)
    ORDER BY 10 DESC, t.slug";

pub fn exists(conn: &Connection, project: &str, slug: &str) -> Result<bool> {
    Ok(query_opt(conn, "SELECT 1 FROM threads WHERE project_slug = ?1 AND slug = ?2", [project, slug], |_| Ok(()))?.is_some())
}

pub fn require(conn: &Connection, project: &str, slug: &str) -> Result<()> {
    if exists(conn, project, slug)? { Ok(()) } else { Err(Error::not_found("Thread not found")) }
}

/// A project's threads, newest activity first.
pub fn list(conn: &Connection, project: &str) -> Result<Vec<ThreadSummary>> {
    summaries(conn, project, None)
}

fn summary(conn: &Connection, project: &str, slug: &str) -> Result<ThreadSummary> {
    summaries(conn, project, Some(slug))?.pop().ok_or_else(|| Error::not_found("Thread not found"))
}

fn summaries(conn: &Connection, project: &str, only: Option<&str>) -> Result<Vec<ThreadSummary>> {
    let mut links: HashMap<String, Vec<String>> = HashMap::new();
    for (a, b) in query_all(conn, "SELECT a, b FROM thread_links WHERE project_slug = ?1 ORDER BY a, b", [project], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })? {
        links.entry(a.clone()).or_default().push(b.clone());
        links.entry(b).or_default().push(a);
    }

    let mut threads = query_all(conn, SUMMARY_SQL, params![project, only], |r| {
        Ok(ThreadSummary {
            project_slug: r.get(0)?,
            slug: r.get(1)?,
            title: r.get(2)?,
            created_at: r.get(3)?,
            linked_thread_slugs: vec![],
            linked_count: 0,
            post_count: r.get(4)?,
            prompt_count: r.get(5)?,
            feedback_count: r.get(6)?,
            upload_count: r.get(7)?,
            story_count: r.get(8)?,
            last_activity: r.get(9)?,
        })
    })?;
    for thread in &mut threads {
        thread.linked_thread_slugs = links.remove(&thread.slug).unwrap_or_default();
        thread.linked_count = thread.linked_thread_slugs.len() as i64;
    }
    Ok(threads)
}

pub fn create(conn: &Connection, project: &str, title: &str) -> Result<ThreadSummary> {
    projects::require(conn, project)?;
    let title = title.trim();
    if title.is_empty() {
        return Err(Error::invalid("Title is required"));
    }
    let slug = unique_slug(conn, project, title)?;
    conn.execute(
        "INSERT INTO threads (project_slug, slug, title, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![project, slug, title, ids::now()],
    )?;
    summary(conn, project, &slug)
}

pub fn rename(conn: &Connection, project: &str, slug: &str, title: &str) -> Result<ThreadSummary> {
    let title = title.trim();
    if title.is_empty() {
        return Err(Error::invalid("Title is required"));
    }
    if conn.execute("UPDATE threads SET title = ?3 WHERE project_slug = ?1 AND slug = ?2", params![project, slug, title])? == 0 {
        return Err(Error::not_found("Thread not found"));
    }
    summary(conn, project, slug)
}

/// Everything in a thread, for its timeline.
pub fn get(conn: &Connection, root: &Path, project: &str, slug: &str) -> Result<ThreadView> {
    let (title, description, created_at): (String, String, String) = query_opt(
        conn,
        "SELECT title, description, created_at FROM threads WHERE project_slug = ?1 AND slug = ?2",
        [project, slug],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?
    .ok_or_else(|| Error::not_found("Thread not found"))?;

    Ok(ThreadView {
        project_slug: project.to_string(),
        slug: slug.to_string(),
        title,
        description,
        created_at,
        linked_threads: linked_threads(conn, project, slug)?,
        posts: posts::list(conn, project, slug)?,
        prompts: prompts::list(conn, project, slug)?,
        feedback: feedback::list_for_thread(conn, project, slug)?,
        uploads: uploads::list(conn, root, project, slug)?,
        stories: stories::list_for_thread(conn, project, slug)?,
    })
}

/// The thread goes, with its posts, prompts, feedback, upload manifests and links. Attachments nothing else
/// refers to go with it; tasks that pointed at it just lose that link.
pub fn delete(conn: &Connection, root: &Path, project: &str, slug: &str) -> Result<()> {
    let files = uploads::files_of_thread(conn, project, slug)?;
    let tx = conn.unchecked_transaction()?;
    tx.execute("UPDATE tasks SET thread_slug = NULL WHERE project_slug = ?1 AND thread_slug = ?2", [project, slug])?;
    tx.execute("DELETE FROM threads WHERE project_slug = ?1 AND slug = ?2", [project, slug])?;
    tx.commit()?;
    uploads::remove_unreferenced(conn, root, project, &files);
    Ok(())
}

pub fn linked_threads(conn: &Connection, project: &str, slug: &str) -> Result<Vec<LinkedThread>> {
    query_all(
        conn,
        "SELECT t.slug, t.title
         FROM thread_links l
         JOIN threads t ON t.project_slug = l.project_slug AND t.slug = CASE WHEN l.a = ?2 THEN l.b ELSE l.a END
         WHERE l.project_slug = ?1 AND (l.a = ?2 OR l.b = ?2)
         ORDER BY t.title COLLATE NOCASE, t.slug",
        [project, slug],
        |r| Ok(LinkedThread { slug: r.get(0)?, title: r.get(1)? }),
    )
}

/// Links are bidirectional: one row serves both sides.
pub fn link(conn: &Connection, project: &str, slug: &str, other: &str) -> Result<()> {
    if slug == other {
        return Err(Error::invalid("A thread can't link to itself"));
    }
    require(conn, project, slug)?;
    require(conn, project, other)?;
    let (a, b) = if slug < other { (slug, other) } else { (other, slug) };
    conn.execute("INSERT OR IGNORE INTO thread_links (project_slug, a, b) VALUES (?1, ?2, ?3)", [project, a, b])?;
    Ok(())
}

pub fn unlink(conn: &Connection, project: &str, slug: &str, other: &str) -> Result<()> {
    let (a, b) = if slug < other { (slug, other) } else { (other, slug) };
    conn.execute("DELETE FROM thread_links WHERE project_slug = ?1 AND a = ?2 AND b = ?3", [project, a, b])?;
    Ok(())
}

fn unique_slug(conn: &Connection, project: &str, title: &str) -> Result<String> {
    let base = ids::slugify(title);
    let mut slug = base.clone();
    let mut n = 2;
    while exists(conn, project, &slug)? {
        slug = format!("{base}-{n}");
        n += 1;
    }
    Ok(slug)
}
