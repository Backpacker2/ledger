//! Documents are standalone entities: the live draft sits in `documents`, and every published version is
//! an immutable snapshot in `document_versions`. A document links to at most one thread (the place its
//! guiding questions get answered) and starts out with none.

use rusqlite::{Connection, Row, params};

use crate::db::{query_all, query_opt};
use crate::error::{Error, Result};
use crate::models::{Bump, DocumentSummary, DocumentView};
use crate::posts::{self, AddOptions};
use crate::{document_types, ids, markdown, threads};

const SUMMARY_SQL: &str = "
    SELECT d.id, d.document_type_slug, COALESCE(t.name, d.document_type_slug), d.title,
           d.thread_slug, th.title, d.version, d.created_at, d.updated_at
    FROM documents d
    LEFT JOIN document_types t ON t.slug = d.document_type_slug
    LEFT JOIN threads th ON th.slug = d.thread_slug";

fn summary_from_row(r: &Row<'_>) -> rusqlite::Result<DocumentSummary> {
    Ok(DocumentSummary {
        id: r.get(0)?,
        document_type_slug: r.get(1)?,
        document_type_name: r.get(2)?,
        title: r.get(3)?,
        thread_slug: r.get(4)?,
        thread_title: r.get(5)?,
        version: r.get(6)?,
        created_at: r.get(7)?,
        updated_at: r.get(8)?,
    })
}

// ---- reading ----------------------------------------------------------------------------------------

/// Most recently touched first.
pub fn list_all(conn: &Connection) -> Result<Vec<DocumentSummary>> {
    query_all(conn, &format!("{SUMMARY_SQL} ORDER BY d.updated_at DESC, d.id"), [], summary_from_row)
}

pub fn list_for_thread(conn: &Connection, slug: &str) -> Result<Vec<DocumentSummary>> {
    query_all(conn, &format!("{SUMMARY_SQL} WHERE d.thread_slug = ?1 ORDER BY d.created_at, d.id"), [slug], summary_from_row)
}

fn summary(conn: &Connection, id: &str) -> Result<DocumentSummary> {
    query_opt(conn, &format!("{SUMMARY_SQL} WHERE d.id = ?1"), [id], summary_from_row)?
        .ok_or_else(|| Error::not_found("Document not found"))
}

fn view(conn: &Connection, s: DocumentSummary, body: String) -> Result<DocumentView> {
    Ok(DocumentView {
        versions: versions(conn, &s.id)?,
        html: markdown::render(&body),
        body,
        id: s.id,
        document_type_slug: s.document_type_slug,
        document_type_name: s.document_type_name,
        title: s.title,
        thread_slug: s.thread_slug,
        thread_title: s.thread_title,
        version: s.version,
        created_at: s.created_at,
        updated_at: s.updated_at,
        snapshot: false,
        deleted: false,
    })
}

pub fn get(conn: &Connection, id: &str) -> Result<DocumentView> {
    let s = summary(conn, id)?;
    let body: String = conn.query_row("SELECT body FROM documents WHERE id = ?1", [id], |r| r.get(0))?;
    view(conn, s, body)
}

/// A published version, read-only. Still works after the draft has been deleted.
pub fn get_version(conn: &Connection, id: &str, version: &str) -> Result<DocumentView> {
    let (type_slug, title, body, published_at): (String, String, String, String) = query_opt(
        conn,
        "SELECT document_type_slug, title, body, published_at FROM document_versions WHERE document_id = ?1 AND version = ?2",
        [id, version],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?
    .ok_or_else(|| Error::not_found("Version not found"))?;

    let live = summary(conn, id).ok();
    let type_name = query_opt(conn, "SELECT name FROM document_types WHERE slug = ?1", [&type_slug], |r| r.get::<_, String>(0))?;
    let s = DocumentSummary {
        id: id.to_string(),
        document_type_name: type_name.unwrap_or_else(|| type_slug.clone()),
        document_type_slug: type_slug,
        title,
        thread_slug: live.as_ref().and_then(|d| d.thread_slug.clone()),
        thread_title: live.as_ref().and_then(|d| d.thread_title.clone()),
        version: Some(version.to_string()),
        created_at: live.as_ref().map_or_else(|| published_at.clone(), |d| d.created_at.clone()),
        updated_at: published_at,
    };
    let mut doc = view(conn, s, body)?;
    doc.snapshot = true;
    doc.deleted = live.is_none();
    Ok(doc)
}

/// Published versions, oldest first ("0.1" before "0.10" before "1.0").
pub fn versions(conn: &Connection, id: &str) -> Result<Vec<String>> {
    query_all(conn, "SELECT version FROM document_versions WHERE document_id = ?1 ORDER BY major, minor", [id], |r| r.get(0))
}

// ---- writing ----------------------------------------------------------------------------------------

/// With a thread the document is created inside it: linked, and the type's guiding questions seeded there.
/// Without one it starts unlinked and `start_drafting` links it later.
pub fn create(conn: &Connection, type_slug: &str, title: &str, thread_slug: Option<&str>) -> Result<DocumentView> {
    let title = title.trim();
    if title.is_empty() {
        return Err(Error::invalid("Title is required"));
    }
    if !document_types::exists(conn, type_slug)? {
        return Err(Error::invalid("Document type not found"));
    }
    let thread_slug = thread_slug.map(str::trim).filter(|s| !s.is_empty());
    if let Some(slug) = thread_slug {
        threads::require(conn, slug)?;
    }

    let id = ids::new_id();
    let now = ids::now();
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO documents (id, document_type_slug, title, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)",
        params![id, type_slug, title, now],
    )?;
    if let Some(slug) = thread_slug {
        link_thread(&tx, &id, type_slug, slug)?;
    }
    tx.commit()?;
    get(conn, &id)
}

pub fn update(conn: &Connection, id: &str, title: Option<&str>, body: Option<&str>) -> Result<DocumentView> {
    summary(conn, id)?; // not found if it does not exist

    let tx = conn.unchecked_transaction()?;
    if let Some(title) = title {
        let title = title.trim();
        if title.is_empty() {
            return Err(Error::invalid("Title is required"));
        }
        tx.execute("UPDATE documents SET title = ?2 WHERE id = ?1", [id, title])?;
    }
    if let Some(body) = body {
        tx.execute("UPDATE documents SET body = ?2 WHERE id = ?1", [id, body])?;
    }
    tx.execute("UPDATE documents SET updated_at = ?2 WHERE id = ?1", [id, &ids::now()])?;
    tx.commit()?;
    get(conn, id)
}

/// Snapshots the draft as a new version, and drops a note in the originating thread so the publish shows
/// up there too, clickable back to that exact snapshot.
pub fn publish(conn: &Connection, id: &str, bump: Bump, explicit: Option<&str>) -> Result<DocumentView> {
    let doc = get(conn, id)?;

    let (version, major, minor) = match explicit.map(str::trim).filter(|v| !v.is_empty()) {
        Some(v) => {
            let (major, minor) = parse_version(v).ok_or_else(|| Error::invalid("Version must be in M.m format, e.g. 1.4"))?;
            (format!("{major}.{minor}"), major, minor)
        }
        None => {
            // Bump from the highest published version, so a custom version earlier on never collides.
            let highest = doc.versions.last().map(String::as_str);
            let next = bump_version(highest, bump);
            let (major, minor) = parse_version(&next).unwrap_or((0, 1));
            (next, major, minor)
        }
    };
    if doc.versions.contains(&version) {
        return Err(Error::invalid(format!("Version {version} is already published")));
    }

    let now = ids::now();
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO document_versions (document_id, version, major, minor, document_type_slug, title, body, published_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![id, version, major, minor, doc.document_type_slug, doc.title, doc.body, now],
    )?;
    tx.execute("UPDATE documents SET version = ?2, updated_at = ?3 WHERE id = ?1", params![id, version, now])?;
    if let Some(slug) = &doc.thread_slug {
        posts::add(
            &tx,
            slug,
            &format!("Version {version} of **{}** was just published.", doc.title),
            AddOptions {
                version_published: true,
                document_id: Some(id.to_string()),
                document_version: Some(version.clone()),
                ..Default::default()
            },
        )?;
    }
    tx.commit()?;
    get(conn, id)
}

/// Links (or creates) the thread that will host this document's guiding-question Q&A and seeds those
/// questions into it. The questions are a one-time snapshot: later edits to the type never reach back into
/// documents already started.
pub fn start_drafting(conn: &Connection, id: &str, thread_slug: Option<&str>, new_thread_title: Option<&str>) -> Result<DocumentView> {
    let doc = summary(conn, id)?;
    if doc.thread_slug.is_some() {
        return Err(Error::invalid("Document is already linked to a thread"));
    }

    let tx = conn.unchecked_transaction()?;
    let target = match (thread_slug.filter(|s| !s.is_empty()), new_thread_title.map(str::trim).filter(|t| !t.is_empty())) {
        (Some(slug), _) => {
            threads::require(&tx, slug)?;
            slug.to_string()
        }
        (None, Some(title)) => threads::create(&tx, title)?.slug,
        (None, None) => return Err(Error::invalid("Provide a thread to link, or a title for a new thread")),
    };
    link_thread(&tx, id, &doc.document_type_slug, &target)?;
    tx.commit()?;
    get(conn, id)
}

/// Deletes the live document. Published version snapshots are kept.
pub fn delete(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("DELETE FROM documents WHERE id = ?1", [id])?;
    Ok(())
}

fn link_thread(conn: &Connection, id: &str, type_slug: &str, thread_slug: &str) -> Result<()> {
    // The type may have been deleted since the document was made; it then just starts with no questions.
    let type_name = query_opt(conn, "SELECT name FROM document_types WHERE slug = ?1", [type_slug], |r| r.get::<_, String>(0))?;
    if let Some(name) = &type_name {
        for question in document_types::questions_of(conn, type_slug)? {
            posts::add(
                conn,
                thread_slug,
                &format!("**Guiding question ({name}):** {question}"),
                AddOptions { guiding_question: true, ..Default::default() },
            )?;
        }
    }
    conn.execute("UPDATE documents SET thread_slug = ?2, updated_at = ?3 WHERE id = ?1", params![id, thread_slug, ids::now()])?;
    Ok(())
}

// ---- versions ---------------------------------------------------------------------------------------

/// "M.m" with both parts plain digits.
pub fn parse_version(version: &str) -> Option<(u32, u32)> {
    let (major, minor) = version.split_once('.')?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !digits(major) || !digits(minor) {
        return None;
    }
    Some((major.parse().ok()?, minor.parse().ok()?))
}

/// Minor goes 1.0 → 1.1, major goes 1.4 → 2.0; the first publish of anything is 0.1 or 1.0.
pub fn bump_version(current: Option<&str>, bump: Bump) -> String {
    let (major, minor) = current.and_then(parse_version).unwrap_or((0, 0));
    match bump {
        Bump::Major => format!("{}.0", major + 1),
        Bump::Minor => format!("{major}.{}", minor + 1),
    }
}
