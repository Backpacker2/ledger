use rusqlite::Connection;

use crate::db::query_all;
use crate::deliverables;
use crate::error::Result;
use crate::frontmatter;
use crate::models::SearchHit;

const SNIPPET_CONTEXT: usize = 60;
const MAX_HITS: usize = 200;

/// Case-insensitive substring search across every post, prompt, piece of feedback and markdown deliverable in
/// every project, newest first.
pub fn search(conn: &Connection, query: &str) -> Result<Vec<SearchHit>> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(vec![]);
    }
    let mut hits = vec![];

    // (kind, id, project, project name, thread slug, thread title, created, text)
    type Row = (String, String, String, String, Option<String>, Option<String>, String, String);
    let rows: Vec<Row> = query_all(
        conn,
        "SELECT 'post', x.id, x.project_slug, p.name, x.thread_slug, t.title, x.created_at, x.body
         FROM posts x JOIN threads t ON t.project_slug = x.project_slug AND t.slug = x.thread_slug JOIN projects p ON p.slug = x.project_slug
         UNION ALL
         SELECT 'prompt', x.id, x.project_slug, p.name, x.thread_slug, t.title, x.created_at, x.title || ' ' || x.body
         FROM prompts x JOIN threads t ON t.project_slug = x.project_slug AND t.slug = x.thread_slug JOIN projects p ON p.slug = x.project_slug
         UNION ALL
         SELECT 'feedback', x.id, x.project_slug, p.name, x.thread_slug, t.title, x.created_at, x.body
         FROM feedback x JOIN threads t ON t.project_slug = x.project_slug AND t.slug = x.thread_slug JOIN projects p ON p.slug = x.project_slug
         UNION ALL
         SELECT 'deliverable', x.path, x.project_slug, p.name, NULL, NULL, x.modified, x.body
         FROM deliverables x JOIN projects p ON p.slug = x.project_slug WHERE x.body IS NOT NULL",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?)),
    )?;

    for (kind, id, project_slug, project_name, thread_slug, thread_title, created_at, text) in rows {
        if kind == "deliverable" && deliverables::split_version(&id).1.is_some() {
            continue; // the live file is what is searched, not every frozen copy of it
        }
        let text = if kind == "deliverable" { frontmatter::parse(&text).map_or(text.clone(), |p| p.body) } else { text };
        let title = if kind == "deliverable" { deliverables::file_name(&id).to_string() } else { thread_title.clone().unwrap_or_default() };
        let found = snippet(&text, query).or_else(|| (kind == "deliverable").then(|| snippet(&title, query)).flatten());
        if let Some(snippet) = found {
            hits.push(SearchHit { kind, id, project_slug, project_name, title, thread_slug, created_at, snippet });
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
