use rusqlite::{Connection, params};

use crate::db::{query_all, query_opt};
use crate::error::{Error, Result};
use crate::ids;
use crate::models::DocumentType;

const SELECT: &str = "
    SELECT t.slug, t.name, t.\"trigger\", t.created_at, t.updated_at,
           (SELECT COUNT(*) FROM documents d WHERE d.document_type_slug = t.slug)
    FROM document_types t";

fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<DocumentType> {
    Ok(DocumentType {
        slug: r.get(0)?,
        name: r.get(1)?,
        trigger: r.get(2)?,
        questions: vec![],
        created_at: r.get(3)?,
        updated_at: r.get(4)?,
        document_count: r.get(5)?,
    })
}

pub fn list(conn: &Connection) -> Result<Vec<DocumentType>> {
    let mut types = query_all(conn, &format!("{SELECT} ORDER BY t.name COLLATE NOCASE, t.slug"), [], from_row)?;
    for t in &mut types {
        t.questions = questions_of(conn, &t.slug)?;
    }
    Ok(types)
}

pub fn get(conn: &Connection, slug: &str) -> Result<DocumentType> {
    let mut t = query_opt(conn, &format!("{SELECT} WHERE t.slug = ?1"), [slug], from_row)?
        .ok_or_else(|| Error::not_found("Document type not found"))?;
    t.questions = questions_of(conn, slug)?;
    Ok(t)
}

pub fn exists(conn: &Connection, slug: &str) -> Result<bool> {
    Ok(query_opt(conn, "SELECT 1 FROM document_types WHERE slug = ?1", [slug], |_| Ok(()))?.is_some())
}

pub fn questions_of(conn: &Connection, slug: &str) -> Result<Vec<String>> {
    query_all(conn, "SELECT question FROM document_type_questions WHERE type_slug = ?1 ORDER BY position", [slug], |r| r.get(0))
}

pub fn create(conn: &Connection, name: &str, trigger: Option<&str>, questions: Option<&[String]>) -> Result<DocumentType> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::invalid("Name is required"));
    }

    let slug = unique_slug(conn, name)?;
    let now = ids::now();
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO document_types (slug, name, \"trigger\", created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)",
        params![slug, name, trigger.unwrap_or("").trim(), now],
    )?;
    write_questions(&tx, &slug, questions.unwrap_or_default())?;
    tx.commit()?;
    get(conn, &slug)
}

pub fn update(
    conn: &Connection,
    slug: &str,
    name: Option<&str>,
    trigger: Option<&str>,
    questions: Option<&[String]>,
) -> Result<DocumentType> {
    get(conn, slug)?; // not found if it does not exist

    let tx = conn.unchecked_transaction()?;
    if let Some(name) = name {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::invalid("Name is required"));
        }
        tx.execute("UPDATE document_types SET name = ?2 WHERE slug = ?1", [slug, name])?;
    }
    if let Some(trigger) = trigger {
        tx.execute("UPDATE document_types SET \"trigger\" = ?2 WHERE slug = ?1", [slug, trigger.trim()])?;
    }
    if let Some(questions) = questions {
        write_questions(&tx, slug, questions)?;
    }
    tx.execute("UPDATE document_types SET updated_at = ?2 WHERE slug = ?1", [slug, &ids::now()])?;
    tx.commit()?;
    get(conn, slug)
}

/// A type can't be deleted while documents still use it.
pub fn delete(conn: &Connection, slug: &str) -> Result<()> {
    let in_use: i64 = conn.query_row("SELECT COUNT(*) FROM documents WHERE document_type_slug = ?1", [slug], |r| r.get(0))?;
    if in_use > 0 {
        return Err(Error::invalid(format!("{in_use} document(s) still use this type — delete them first")));
    }
    conn.execute("DELETE FROM document_types WHERE slug = ?1", [slug])?;
    Ok(())
}

/// Blank questions are dropped; the order the caller gave is kept.
fn write_questions(conn: &Connection, slug: &str, questions: &[String]) -> Result<()> {
    conn.execute("DELETE FROM document_type_questions WHERE type_slug = ?1", [slug])?;
    let cleaned = questions.iter().map(|q| q.trim()).filter(|q| !q.is_empty());
    for (position, question) in cleaned.enumerate() {
        conn.execute(
            "INSERT INTO document_type_questions (type_slug, position, question) VALUES (?1, ?2, ?3)",
            params![slug, position as i64, question],
        )?;
    }
    Ok(())
}

fn unique_slug(conn: &Connection, name: &str) -> Result<String> {
    let base = ids::slugify(name);
    let mut slug = base.clone();
    let mut n = 2;
    while exists(conn, &slug)? {
        slug = format!("{base}-{n}");
        n += 1;
    }
    Ok(slug)
}
