use rusqlite::Connection;

use crate::db::query_all;
use crate::error::Result;
use crate::models::SearchHit;

const SNIPPET_CONTEXT: usize = 60;
const MAX_HITS: usize = 200;

/// Case-insensitive substring search across every post and every live document, newest first.
pub fn search(conn: &Connection, query: &str) -> Result<Vec<SearchHit>> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(vec![]);
    }

    let mut hits = query_all(
        conn,
        "SELECT p.id, t.title, t.slug, p.created_at, p.body
         FROM posts p JOIN threads t ON t.slug = p.thread_slug",
        [],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        },
    )?
    .into_iter()
    .filter_map(|(id, thread_title, thread_slug, created_at, body)| {
        snippet(&body, query).map(|snippet| SearchHit {
            kind: "post".into(),
            id,
            title: thread_title.clone(),
            thread_slug: Some(thread_slug),
            thread_title: Some(thread_title),
            created_at,
            snippet,
        })
    })
    .collect::<Vec<_>>();

    let documents = query_all(
        conn,
        "SELECT d.id, d.title, d.thread_slug, th.title, d.updated_at, d.body
         FROM documents d LEFT JOIN threads th ON th.slug = d.thread_slug",
        [],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
            ))
        },
    )?;
    for (id, title, thread_slug, thread_title, updated_at, body) in documents {
        // Prefer a body match for the snippet; a title-only match shows the title.
        if let Some(snippet) = snippet(&body, query).or_else(|| snippet(&title, query)) {
            hits.push(SearchHit { kind: "document".into(), id, title, thread_slug, thread_title, created_at: updated_at, snippet });
        }
    }

    hits.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    hits.truncate(MAX_HITS);
    Ok(hits)
}

/// An excerpt around the first case-insensitive match, with "…" where it was cut. Works on characters, not
/// bytes, so multi-byte text never splits mid-character.
pub fn snippet(text: &str, query: &str) -> Option<String> {
    let fold = |c: char| c.to_lowercase().next().unwrap_or(c);

    let original: Vec<char> = text.chars().collect();
    let folded: Vec<char> = original.iter().map(|&c| fold(c)).collect();
    let needle: Vec<char> = query.chars().map(fold).collect();
    if needle.is_empty() {
        return None;
    }

    let start = folded.windows(needle.len()).position(|window| window == needle.as_slice())?;
    let from = start.saturating_sub(SNIPPET_CONTEXT);
    let to = (start + needle.len() + SNIPPET_CONTEXT).min(original.len());

    let excerpt = original[from..to].iter().collect::<String>();
    let excerpt = excerpt.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(format!("{}{}{}", if from > 0 { "…" } else { "" }, excerpt, if to < original.len() { "…" } else { "" }))
}
