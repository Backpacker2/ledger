use std::collections::HashMap;

use rusqlite::{Connection, params};

use crate::db::{query_all, query_opt};
use crate::error::{Error, Result};
use crate::models::{LinkedThread, ThreadSummary, ThreadView};
use crate::{documents, ids, posts, stories};

const SUMMARY_SQL: &str = "
    SELECT t.slug, t.title, t.created_at,
           (SELECT COUNT(*) FROM posts p WHERE p.thread_slug = t.slug),
           COALESCE((SELECT MAX(p.created_at) FROM posts p WHERE p.thread_slug = t.slug), t.created_at),
           (SELECT COUNT(*) FROM documents d WHERE d.thread_slug = t.slug),
           (SELECT COUNT(*) FROM story_threads s WHERE s.thread_slug = t.slug)
    FROM threads t
    WHERE ?1 IS NULL OR t.slug = ?1
    ORDER BY 5 DESC, t.slug";

pub fn exists(conn: &Connection, slug: &str) -> Result<bool> {
    Ok(query_opt(conn, "SELECT 1 FROM threads WHERE slug = ?1", [slug], |_| Ok(()))?.is_some())
}

pub fn require(conn: &Connection, slug: &str) -> Result<()> {
    if exists(conn, slug)? { Ok(()) } else { Err(Error::not_found("Thread not found")) }
}

/// Newest activity first.
pub fn list(conn: &Connection) -> Result<Vec<ThreadSummary>> {
    summaries(conn, None)
}

fn summary(conn: &Connection, slug: &str) -> Result<ThreadSummary> {
    summaries(conn, Some(slug))?.pop().ok_or_else(|| Error::not_found("Thread not found"))
}

fn summaries(conn: &Connection, only: Option<&str>) -> Result<Vec<ThreadSummary>> {
    let mut links: HashMap<String, Vec<String>> = HashMap::new();
    for (a, b) in query_all(conn, "SELECT a, b FROM thread_links ORDER BY a, b", [], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        links.entry(a.clone()).or_default().push(b.clone());
        links.entry(b).or_default().push(a);
    }

    let mut threads = query_all(conn, SUMMARY_SQL, [only], |r| {
        Ok(ThreadSummary {
            slug: r.get(0)?,
            title: r.get(1)?,
            created_at: r.get(2)?,
            linked_thread_slugs: vec![],
            post_count: r.get(3)?,
            linked_count: 0,
            last_activity: r.get(4)?,
            document_count: r.get(5)?,
            story_count: r.get(6)?,
        })
    })?;
    for thread in &mut threads {
        thread.linked_thread_slugs = links.remove(&thread.slug).unwrap_or_default();
        thread.linked_count = thread.linked_thread_slugs.len() as i64;
    }
    Ok(threads)
}

pub fn create(conn: &Connection, title: &str) -> Result<ThreadSummary> {
    let title = title.trim();
    if title.is_empty() {
        return Err(Error::invalid("Title is required"));
    }
    let slug = unique_slug(conn, title)?;
    conn.execute("INSERT INTO threads (slug, title, created_at) VALUES (?1, ?2, ?3)", params![slug, title, ids::now()])?;
    summary(conn, &slug)
}

pub fn rename(conn: &Connection, slug: &str, title: &str) -> Result<ThreadSummary> {
    let title = title.trim();
    if title.is_empty() {
        return Err(Error::invalid("Title is required"));
    }
    if conn.execute("UPDATE threads SET title = ?2 WHERE slug = ?1", params![slug, title])? == 0 {
        return Err(Error::not_found("Thread not found"));
    }
    summary(conn, slug)
}

pub fn get(conn: &Connection, slug: &str) -> Result<ThreadView> {
    let (title, created_at): (String, String) =
        query_opt(conn, "SELECT title, created_at FROM threads WHERE slug = ?1", [slug], |r| Ok((r.get(0)?, r.get(1)?)))?
            .ok_or_else(|| Error::not_found("Thread not found"))?;

    Ok(ThreadView {
        slug: slug.to_string(),
        title,
        created_at,
        linked_threads: linked_threads(conn, slug)?,
        posts: posts::list(conn, slug)?,
        documents: documents::list_for_thread(conn, slug)?,
        stories: stories::list_for_thread(conn, slug)?,
    })
}

/// The thread goes, with its posts, links and story links. Its documents are standalone entities and are
/// kept, just unlinked, so drafting progress is not lost to an unrelated thread deletion.
pub fn delete(conn: &Connection, slug: &str) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("UPDATE documents SET thread_slug = NULL, updated_at = ?2 WHERE thread_slug = ?1", params![slug, ids::now()])?;
    tx.execute("DELETE FROM threads WHERE slug = ?1", [slug])?;
    tx.commit()?;
    Ok(())
}

pub fn linked_threads(conn: &Connection, slug: &str) -> Result<Vec<LinkedThread>> {
    query_all(
        conn,
        "SELECT t.slug, t.title
         FROM thread_links l
         JOIN threads t ON t.slug = CASE WHEN l.a = ?1 THEN l.b ELSE l.a END
         WHERE l.a = ?1 OR l.b = ?1
         ORDER BY t.title COLLATE NOCASE, t.slug",
        [slug],
        |r| Ok(LinkedThread { slug: r.get(0)?, title: r.get(1)? }),
    )
}

/// Links are bidirectional: one row serves both sides.
pub fn link(conn: &Connection, slug: &str, other: &str) -> Result<()> {
    if slug == other {
        return Err(Error::invalid("A thread can't link to itself"));
    }
    require(conn, slug)?;
    require(conn, other)?;
    let (a, b) = if slug < other { (slug, other) } else { (other, slug) };
    conn.execute("INSERT OR IGNORE INTO thread_links (a, b) VALUES (?1, ?2)", [a, b])?;
    Ok(())
}

pub fn unlink(conn: &Connection, slug: &str, other: &str) -> Result<()> {
    let (a, b) = if slug < other { (slug, other) } else { (other, slug) };
    conn.execute("DELETE FROM thread_links WHERE a = ?1 AND b = ?2", [a, b])?;
    Ok(())
}

fn unique_slug(conn: &Connection, title: &str) -> Result<String> {
    let base = ids::slugify(title);
    let mut slug = base.clone();
    let mut n = 2;
    while exists(conn, &slug)? {
        slug = format!("{base}-{n}");
        n += 1;
    }
    Ok(slug)
}
