//! How a published version came to be: who made the change (a person who typed it, a Claude Code run, or AI the person
//! says they used elsewhere), the thread it is filed under, the prompt and run behind it, and the AI-use label
//! (MMM, https://mmmlabel.tech) declared for the product at that version.
//!
//! A version is a bare file in `deliverables/` and must stay one (its bytes are compared with the live file, and
//! anything next to it would show up as a deliverable), so this lives in a record of its own: `versions/<id>.md` in the
//! project, one per published version. A version published before records existed has none; what can be proven from
//! the runs and the "version published" notes is worked out when it is shown ([`resolve`]), and nothing is written for
//! it until the person marks it.
//!
//! The AI-use label is declared, never derived: Ledger knows who made each change, not how much human effort went
//! into it. A person can also set a label by hand on any version.

use std::collections::{BTreeMap, HashMap, HashSet};

use rusqlite::{Connection, params};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::db::{query_all, query_opt};
use crate::deliverables::{self, split_version};
use crate::error::{Error, Result};
use crate::models::{PublishChoices, VersionUpdate};
use crate::{ids, prompts, threads};

pub const AI: &str = "ai";
pub const HUMAN: &str = "human";

/// The five MMM levels, from the most AI to the least.
pub const MMM_LEVELS: [&str; 5] = ["lazy-prompter", "piggybacker", "cyborg", "handyman", "man-made"];

/// A level as stored, or `None` for text that is no level (a hand-written file may say anything).
pub fn level(text: &str) -> Option<&'static str> {
    let text = text.trim().to_lowercase();
    MMM_LEVELS.iter().copied().find(|l| *l == text)
}

/// "ai", "human", or "" for anything else.
pub fn author_of(text: &str) -> &'static str {
    match text.trim().to_lowercase().as_str() {
        "ai" => AI,
        "human" => HUMAN,
        _ => "",
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

// ---- what is recorded when a version is published ---------------------------------------------------

/// What caused a version, which decides how the note in the thread reads.
#[derive(Debug, Clone)]
pub enum Cause {
    /// Published from the page.
    Manual,
    /// A Claude Code run published what it wrote.
    Run,
    /// Edits that no version held, published just before the prompt `running` (its id) was run.
    Held { running: String },
}

#[derive(Debug, Clone)]
pub struct Origin {
    pub cause: Cause,
    /// Where the note goes and the panel files the version; `None` records it without a thread.
    pub thread: Option<String>,
    pub ai: bool,
    /// The prompt behind the change, when AI made it.
    pub prompt: Option<String>,
    pub run: Option<String>,
    pub mmm: Option<String>,
    pub note: String,
}

impl Origin {
    /// A Claude Code run published what it wrote in `thread`.
    pub fn run(thread: &str, prompt: &str, run: Option<&str>) -> Origin {
        Origin {
            cause: Cause::Run,
            thread: Some(thread.to_string()),
            ai: true,
            prompt: Some(prompt.to_string()),
            run: run.map(str::to_string),
            mmm: None,
            note: String::new(),
        }
    }

    /// The person's own edits, kept before the run of `running`; or, when `stopped` is the run that left them in the
    /// file, that run's.
    pub fn held(thread: &str, running: &str, stopped: Option<&StoppedRun>) -> Origin {
        Origin {
            cause: Cause::Held { running: running.to_string() },
            thread: Some(thread.to_string()),
            ai: stopped.is_some(),
            prompt: stopped.map(|s| s.prompt_id.clone()),
            run: stopped.map(|s| s.id.clone()),
            mmm: None,
            note: String::new(),
        }
    }
}

/// Checks what the person chose in the publish dialog and fills in the rest.
pub fn manual_origin(conn: &Connection, project: &str, base: &str, choices: &PublishChoices) -> Result<Origin> {
    let thread = match choices.thread.as_deref().map(str::trim) {
        None => default_thread(conn, project, base)?,
        Some("") => None,
        Some(slug) => {
            threads::require(conn, project, slug)?;
            Some(slug.to_string())
        }
    };
    let mmm = match choices.mmm.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
        None => None,
        Some(text) => Some(level(text).ok_or_else(|| Error::invalid("That is not one of the AI-use labels"))?.to_string()),
    };
    let named = |value: &Option<String>| value.as_deref().map(str::trim).filter(|v| !v.is_empty()).map(str::to_string);
    let prompt = named(&choices.prompt);
    if let Some(id) = &prompt
        && query_opt(conn, "SELECT 1 FROM prompts WHERE project_slug = ?1 AND id = ?2", [project, id], |_| Ok(()))?.is_none()
    {
        return Err(Error::not_found("Prompt not found"));
    }
    let run = named(&choices.run);
    if let Some(id) = &run
        && query_opt(conn, "SELECT 1 FROM claude_runs WHERE project_slug = ?1 AND id = ?2", [project, id], |_| Ok(()))?.is_none()
    {
        return Err(Error::not_found("Run not found"));
    }
    // A prompt or a run only explains a change that AI made.
    let ai = choices.ai || run.is_some();
    Ok(Origin {
        cause: Cause::Manual,
        thread,
        ai,
        prompt: prompt.filter(|_| ai),
        run,
        mmm,
        note: choices.note.as_deref().map(str::trim).unwrap_or_default().to_string(),
    })
}

/// A name as it reads inside a note's bold text.
fn plain(text: &str) -> String {
    text.chars().filter(|c| !matches!(c, '*' | '`' | '[' | ']')).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The sentence the thread's "version published" note says.
pub fn announcement(conn: &Connection, project: &str, name: &str, version: &str, origin: &Origin) -> Result<String> {
    let title = |id: &Option<String>| -> Result<Option<String>> {
        match id {
            Some(id) => Ok(prompts::display_title_of(conn, project, id)?.map(|t| plain(&t))),
            None => Ok(None),
        }
    };
    let mut text = match &origin.cause {
        Cause::Run => match title(&origin.prompt)? {
            Some(prompt) => format!("Version {version} of **{name}** was published from the prompt **{prompt}**."),
            None => format!("Version {version} of **{name}** was published from a prompt."),
        },
        Cause::Held { running } => {
            let running = title(&Some(running.clone()))?.map_or_else(|| "a prompt".to_string(), |t| format!("the prompt **{t}**"));
            match (origin.ai, title(&origin.prompt)?) {
                (true, Some(earlier)) => format!(
                    "Version {version} of **{name}** keeps what an earlier run of the prompt **{earlier}** left in the file, published just before {running} ran."
                ),
                (true, None) => format!("Version {version} of **{name}** keeps what an earlier run left in the file, published just before {running} ran."),
                (false, _) => format!("Version {version} of **{name}** keeps your earlier edits, published just before {running} ran."),
            }
        }
        Cause::Manual => match title(&origin.prompt)? {
            Some(prompt) => format!("Version {version} of **{name}** was published from the prompt **{prompt}**."),
            None => format!("Version {version} of **{name}** was just published."),
        },
    };
    if origin.ai && matches!(origin.cause, Cause::Manual) {
        text.push_str(" It was made with generative AI.");
    }
    // A changelog is what a person wrote about the change, so the thread quotes it as theirs. A run writes none.
    let changelog = origin.note.trim();
    if !changelog.is_empty() {
        text.push_str(&format!("

from changelog: {changelog}"));
    }
    Ok(text)
}

/// Writes the record of a version that has just been published.
pub fn insert_record(conn: &Connection, project: &str, base: &str, version: &str, origin: &Origin, sha256: &str, at: &str) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO version_records
            (id, project_slug, deliverable_path, version, thread_slug, prompt_id, run_id, author, mmm, sha256, note, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            ids::new_id(),
            project,
            base,
            version,
            origin.thread,
            origin.prompt,
            origin.run,
            if origin.ai { AI } else { HUMAN },
            origin.mmm,
            sha256,
            origin.note,
            at
        ],
    )?;
    Ok(())
}

/// The thread a version published without a choice goes to: where the deliverable was last filed, else the thread of the
/// prompt that first asked for it.
pub fn default_thread(conn: &Connection, project: &str, base: &str) -> Result<Option<String>> {
    let recorded = query_opt(
        conn,
        "SELECT r.thread_slug FROM version_records r
         JOIN threads t ON t.project_slug = r.project_slug AND t.slug = r.thread_slug
         WHERE r.project_slug = ?1 AND r.deliverable_path = ?2
         ORDER BY r.created_at DESC, r.rowid DESC LIMIT 1",
        [project, base],
        |r| r.get::<_, String>(0),
    )?;
    if recorded.is_some() {
        return Ok(recorded);
    }
    Ok(prompts::producing(conn, project, base)?.first().map(|p| p.thread_slug.clone()))
}

// ---- what the files and notes prove -----------------------------------------------------------------

/// One published version with everything that is known about how it came to be.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub version: String,
    pub path: String,
    pub at: String,
    pub thread: Option<String>,
    pub prompt: Option<String>,
    pub run: Option<String>,
    /// "ai", "human", or "" when nothing says.
    pub author: &'static str,
    pub mmm: Option<String>,
    pub note: String,
    pub sha256: Option<String>,
    pub record: Option<String>,
    /// The "version published" note: thread and id.
    pub post: Option<(String, String)>,
    pub size: i64,
}

struct Stored {
    id: String,
    thread: Option<String>,
    prompt: Option<String>,
    run: Option<String>,
    author: &'static str,
    mmm: Option<String>,
    sha256: Option<String>,
    note: String,
    created_at: String,
}

/// The versions of the deliverable at `base`, oldest first. A version with a record is described by it; one without is
/// described by what the runs and notes prove (see the module docs).
pub fn resolve(conn: &Connection, project: &str, base: &str) -> Result<Vec<Resolved>> {
    let versions = deliverables::versions_of(conn, project, base)?;
    if versions.is_empty() {
        return Ok(vec![]);
    }

    let mut stored: HashMap<String, Stored> = HashMap::new();
    for (version, s) in query_all(
        conn,
        "SELECT version, id, thread_slug, prompt_id, run_id, author, mmm, sha256, note, created_at
         FROM version_records WHERE project_slug = ?1 AND deliverable_path = ?2 ORDER BY created_at, rowid",
        [project, base],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                Stored {
                    id: r.get(1)?,
                    thread: r.get(2)?,
                    prompt: r.get(3)?,
                    run: r.get(4)?,
                    author: author_of(&r.get::<_, String>(5)?),
                    mmm: r.get::<_, Option<String>>(6)?.and_then(|m| level(&m).map(str::to_string)),
                    sha256: r.get(7)?,
                    note: r.get(8)?,
                    created_at: r.get(9)?,
                },
            ))
        },
    )? {
        stored.entry(version).or_insert(s);
    }

    // The note that announced each version: the first one wins.
    let mut notes: HashMap<String, (String, String, String)> = HashMap::new();
    for (version, thread, id, at) in query_all(
        conn,
        "SELECT deliverable_version, thread_slug, id, created_at FROM posts
         WHERE project_slug = ?1 AND version_published = 1 AND deliverable_path = ?2 AND deliverable_version IS NOT NULL
         ORDER BY created_at, rowid",
        [project, base],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?)),
    )? {
        notes.entry(version).or_insert((thread, id, at));
    }

    // A run lists the versions it published and linked: that proves the version came from it.
    let mut by_run: HashMap<String, (String, String, String, String)> = HashMap::new();
    for (run, prompt, thread, finished, path) in query_all(
        conn,
        "SELECT r.id, r.prompt_id, r.thread_slug, r.finished_at, f.path FROM claude_runs r
         JOIN claude_run_files f ON f.run_id = r.id WHERE r.project_slug = ?1 ORDER BY r.started_at, r.rowid",
        [project],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?)),
    )? {
        if let (b, Some(version)) = split_version(&path)
            && b == base
        {
            by_run.entry(version).or_insert((run, prompt, thread, finished));
        }
    }

    let mut out = Vec::with_capacity(versions.len());
    for version in versions {
        let path = deliverables::snapshot_path(base, &version);
        let (size, modified, body) = query_opt(
            conn,
            "SELECT size, modified, body FROM deliverables WHERE project_slug = ?1 AND path = ?2",
            [project, path.as_str()],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?)),
        )?
        .unwrap_or_default();
        let post = notes.get(&version).map(|(thread, id, _)| (thread.clone(), id.clone()));

        let resolved = match stored.remove(&version) {
            Some(s) => Resolved {
                version: version.clone(),
                path,
                at: s.created_at,
                thread: s.thread,
                prompt: s.prompt,
                run: s.run,
                author: s.author,
                mmm: s.mmm,
                note: s.note,
                sha256: s.sha256.or_else(|| body.as_deref().map(|b| sha256_hex(b.as_bytes()))),
                record: Some(s.id),
                post,
                size,
            },
            None => {
                let sha256 = body.as_deref().map(|b| sha256_hex(b.as_bytes()));
                let blank = Resolved {
                    version: version.clone(),
                    path,
                    at: modified,
                    thread: None,
                    prompt: None,
                    run: None,
                    author: "",
                    mmm: None,
                    note: String::new(),
                    sha256,
                    record: None,
                    post: post.clone(),
                    size,
                };
                match (by_run.get(&version), notes.get(&version)) {
                    (Some((run, prompt, thread, finished)), note) => Resolved {
                        at: note.map_or_else(|| finished.clone(), |(_, _, at)| at.clone()),
                        thread: Some(thread.clone()),
                        prompt: Some(prompt.clone()),
                        run: Some(run.clone()),
                        author: AI,
                        ..blank
                    },
                    (None, Some((thread, _, at))) => Resolved { at: at.clone(), thread: Some(thread.clone()), ..blank },
                    (None, None) => blank,
                }
            }
        };
        out.push(resolved);
    }
    Ok(out)
}

/// A run that changed the live file after its newest version and left it out of any version: what the file holds now
/// partly comes from it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoppedRun {
    pub id: String,
    pub prompt_id: String,
    pub prompt_title: String,
    pub thread_slug: String,
    pub status: String,
    pub finished_at: String,
}

pub fn stopped_run(conn: &Connection, project: &str, base: &str) -> Result<Option<StoppedRun>> {
    let newest = resolve(conn, project, base)?.into_iter().map(|v| v.at).max();
    let runs = query_all(
        conn,
        "SELECT r.id, r.prompt_id, r.thread_slug, r.status, r.finished_at FROM claude_runs r
         JOIN claude_run_files f ON f.run_id = r.id
         WHERE r.project_slug = ?1 AND f.path = ?2 ORDER BY r.finished_at DESC, r.rowid DESC",
        [project, base],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?)),
    )?;
    let Some((id, prompt_id, thread_slug, status, finished_at)) = runs.into_iter().find(|run| newest.as_ref().is_none_or(|at| run.4 > *at)) else {
        return Ok(None);
    };
    let prompt_title = prompts::display_title_of(conn, project, &prompt_id)?.unwrap_or_else(|| "a deleted prompt".to_string());
    Ok(Some(StoppedRun { id, prompt_id, prompt_title, thread_slug, status, finished_at }))
}

// ---- changing a record by hand ----------------------------------------------------------------------

/// Sets who made a version and/or its AI-use label, and/or its change note. A version without a record gets one made
/// from what is known of it, so that marking it never loses the thread or the run it was found with.
pub fn update_record(conn: &Connection, project: &str, base: &str, version: &str, update: &VersionUpdate) -> Result<()> {
    let known = resolve(conn, project, base)?;
    let current = known.iter().find(|v| v.version == version).ok_or_else(|| Error::not_found("Version not found"))?;

    let author = match update.by.as_deref().map(str::trim).filter(|b| !b.is_empty()) {
        None => None,
        Some(text) => {
            let author = author_of(text);
            if author.is_empty() {
                return Err(Error::invalid("A version is made by a person (human) or by AI (ai)"));
            }
            if current.run.is_some() && author != AI {
                return Err(Error::invalid("Claude Code made this version, which Ledger saw happen. That can't be changed here."));
            }
            Some(author)
        }
    };
    let mmm = match update.mmm.as_deref().map(str::trim) {
        None => None,
        Some("") => Some(None),
        Some(text) => Some(Some(level(text).ok_or_else(|| Error::invalid("That is not one of the AI-use labels"))?)),
    };

    if current.record.is_none() {
        conn.execute(
            "INSERT INTO version_records
                (id, project_slug, deliverable_path, version, thread_slug, prompt_id, run_id, author, mmm, sha256, note, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, ?9, '', ?10)",
            params![ids::new_id(), project, base, version, current.thread, current.prompt, current.run, current.author, current.sha256, current.at],
        )?;
    }
    if let Some(author) = author {
        conn.execute(
            "UPDATE version_records SET author = ?4 WHERE project_slug = ?1 AND deliverable_path = ?2 AND version = ?3",
            params![project, base, version, author],
        )?;
    }
    if let Some(mmm) = mmm {
        conn.execute(
            "UPDATE version_records SET mmm = ?4 WHERE project_slug = ?1 AND deliverable_path = ?2 AND version = ?3",
            params![project, base, version, mmm],
        )?;
    }
    if let Some(note) = &update.note {
        conn.execute(
            "UPDATE version_records SET note = ?4 WHERE project_slug = ?1 AND deliverable_path = ?2 AND version = ?3",
            params![project, base, version, note.trim()],
        )?;
    }
    Ok(())
}

// ---- the side window --------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptRef {
    pub id: String,
    pub title: String,
    pub thread_slug: String,
    /// Whether the prompt is still there.
    pub exists: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSummary {
    pub id: String,
    pub prompt_id: String,
    pub prompt_title: String,
    pub thread_slug: String,
    pub status: String,
    pub model: String,
    pub permission_mode: String,
    pub started_at: String,
    pub finished_at: String,
    pub seconds: i64,
    pub turns: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteRef {
    pub thread_slug: String,
    pub id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionEntry {
    pub version: String,
    /// The snapshot, relative to the deliverables folder.
    pub path: String,
    pub at: String,
    /// "ai", "human" or "unknown".
    pub by: String,
    pub prompt: Option<PromptRef>,
    pub run: Option<RunSummary>,
    pub mmm: Option<String>,
    pub note: String,
    pub size: i64,
    /// Against the version before it.
    pub size_change: Option<i64>,
    pub sha256: Option<String>,
    /// The record's file in the vault (`versions/<id>.md`), when there is one.
    pub record: Option<String>,
    pub post: Option<NoteRef>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunEntry {
    #[serde(flatten)]
    pub run: RunSummary,
    /// The version this run published, if any.
    pub version: Option<String>,
    pub at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Entry {
    Prompt { id: String, title: String, at: String },
    Run(RunEntry),
    Version(VersionEntry),
    Feedback { id: String, status: String, version: Option<String>, excerpt: String, at: String },
}

impl Entry {
    fn at(&self) -> &str {
        match self {
            Entry::Prompt { at, .. } | Entry::Feedback { at, .. } => at,
            Entry::Run(run) => &run.at,
            Entry::Version(version) => &version.at,
        }
    }

    /// What goes first among things at the same moment: the ask, then the run, then what came out.
    fn rank(&self) -> u8 {
        match self {
            Entry::Prompt { .. } => 0,
            Entry::Run(_) => 1,
            Entry::Version(_) => 2,
            Entry::Feedback { .. } => 3,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupThread {
    pub slug: String,
    pub title: String,
    /// Whether the thread is still there.
    pub exists: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    /// `None` for what is filed under no thread.
    pub thread: Option<GroupThread>,
    pub first_at: String,
    pub last_at: String,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LabelRef {
    pub level: String,
    pub version: String,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub versions: i64,
    pub ai: i64,
    pub human: i64,
    pub unrecorded: i64,
    /// The label of the newest version that has one.
    pub label: Option<LabelRef>,
}

/// The live file when it holds something no version does.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Draft {
    /// The newest version, if any.
    pub since: Option<String>,
    pub modified: String,
    pub stopped_run: Option<StoppedRun>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Provenance {
    pub groups: Vec<Group>,
    pub summary: Summary,
    pub draft: Option<Draft>,
    /// Where a publish goes when the person does not say.
    pub default_thread: Option<String>,
    /// A run that changed the live file and is in no version: the publish dialog starts with "made with AI" ticked.
    pub stopped_run: Option<StoppedRun>,
    pub history_has_ai: bool,
    /// The label of the newest labelled version: where the next one starts from.
    pub suggested_label: Option<String>,
}

fn excerpt(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let mut out: String = line.chars().take(140).collect();
    if line.chars().count() > 140 {
        out = format!("{}…", out.trim_end());
    }
    out
}

/// Everything the side window of a deliverable shows.
pub fn for_deliverable(conn: &Connection, project: &str, base: &str) -> Result<Provenance> {
    let resolved = resolve(conn, project, base)?;

    let thread_titles: HashMap<String, String> =
        query_all(conn, "SELECT slug, title FROM threads WHERE project_slug = ?1", [project], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .into_iter()
            .collect();

    // Prompts: the ones linked to the file or to one of its versions, and the ones the records name.
    let mut prompt_rows: BTreeMap<String, (String, String, String)> = BTreeMap::new(); // id -> thread, title, created_at
    for (id, thread, title, body, created_at, path) in query_all(
        conn,
        "SELECT p.id, p.thread_slug, p.title, p.body, p.created_at, d.path FROM prompt_deliverables d
         JOIN prompts p ON p.id = d.prompt_id WHERE p.project_slug = ?1 ORDER BY p.created_at, p.rowid",
        [project],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?)),
    )? {
        if split_version(&path).0 == base {
            prompt_rows.entry(id).or_insert((thread, prompts::display_title(&title, &body), created_at));
        }
    }
    for id in resolved.iter().filter_map(|v| v.prompt.as_ref()) {
        if prompt_rows.contains_key(id) {
            continue;
        }
        if let Some(row) = query_opt(
            conn,
            "SELECT thread_slug, title, body, created_at FROM prompts WHERE project_slug = ?1 AND id = ?2",
            [project, id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?)),
        )? {
            prompt_rows.insert(id.clone(), (row.0, prompts::display_title(&row.1, &row.2), row.3));
        }
    }
    let prompt_ref = |id: &str| -> PromptRef {
        match prompt_rows.get(id) {
            Some((thread, title, _)) => PromptRef { id: id.to_string(), title: title.clone(), thread_slug: thread.clone(), exists: true },
            None => PromptRef { id: id.to_string(), title: "A deleted prompt".to_string(), thread_slug: String::new(), exists: false },
        }
    };

    // Runs: the ones that touched this file or one of its versions, and the ones the versions name.
    let named_runs: HashSet<&str> = resolved.iter().filter_map(|v| v.run.as_deref()).collect();
    let mut run_files: HashMap<String, Vec<String>> = HashMap::new();
    for (run, path) in query_all(
        conn,
        "SELECT f.run_id, f.path FROM claude_run_files f JOIN claude_runs r ON r.id = f.run_id WHERE r.project_slug = ?1",
        [project],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    )? {
        run_files.entry(run).or_default().push(path);
    }
    let mut runs: BTreeMap<String, RunSummary> = BTreeMap::new();
    for run in query_all(
        conn,
        "SELECT id, prompt_id, thread_slug, status, model, permission_mode, started_at, finished_at, seconds, turns
         FROM claude_runs WHERE project_slug = ?1 ORDER BY started_at, rowid",
        [project],
        |r| {
            Ok(RunSummary {
                id: r.get(0)?,
                prompt_id: r.get(1)?,
                prompt_title: String::new(),
                thread_slug: r.get(2)?,
                status: r.get(3)?,
                model: r.get(4)?,
                permission_mode: r.get(5)?,
                started_at: r.get(6)?,
                finished_at: r.get(7)?,
                seconds: r.get(8)?,
                turns: r.get(9)?,
            })
        },
    )? {
        let touched = run_files.get(&run.id).is_some_and(|files| files.iter().any(|p| split_version(p).0 == base));
        if touched || named_runs.contains(run.id.as_str()) {
            runs.insert(run.id.clone(), run);
        }
    }
    for run in runs.values_mut() {
        run.prompt_title = prompts::display_title_of(conn, project, &run.prompt_id)?.unwrap_or_else(|| "A deleted prompt".to_string());
    }

    let mut entries: Vec<(Option<String>, Entry)> = vec![];
    for (id, (thread, title, at)) in &prompt_rows {
        entries.push((Some(thread.clone()), Entry::Prompt { id: id.clone(), title: title.clone(), at: at.clone() }));
    }
    let produced: HashMap<&str, &str> = resolved.iter().filter_map(|v| v.run.as_deref().map(|r| (r, v.version.as_str()))).collect();
    for run in runs.values() {
        entries.push((
            Some(run.thread_slug.clone()),
            Entry::Run(RunEntry { at: run.started_at.clone(), version: produced.get(run.id.as_str()).map(|v| v.to_string()), run: run.clone() }),
        ));
    }
    let mut previous_size: Option<i64> = None;
    for v in &resolved {
        entries.push((
            v.thread.clone(),
            Entry::Version(VersionEntry {
                version: v.version.clone(),
                path: v.path.clone(),
                at: v.at.clone(),
                by: if v.author.is_empty() { "unknown".to_string() } else { v.author.to_string() },
                prompt: v.prompt.as_deref().map(prompt_ref),
                run: v.run.as_ref().and_then(|id| runs.get(id)).cloned(),
                mmm: v.mmm.clone(),
                note: v.note.clone(),
                size: v.size,
                size_change: previous_size.map(|before| v.size - before),
                sha256: v.sha256.clone(),
                record: v.record.clone(),
                post: v.post.as_ref().map(|(thread, id)| NoteRef { thread_slug: thread.clone(), id: id.clone() }),
            }),
        ));
        previous_size = Some(v.size);
    }
    for (id, thread, path, version, status, body, created_at) in query_all(
        conn,
        "SELECT id, thread_slug, deliverable_path, deliverable_version, status, body, created_at FROM feedback
         WHERE project_slug = ?1 ORDER BY created_at, rowid",
        [project],
        |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?, r.get::<_, String>(6)?))
        },
    )? {
        if split_version(&path).0 == base {
            entries.push((Some(thread), Entry::Feedback { id, status, version, excerpt: excerpt(&body), at: created_at }));
        }
    }

    let mut grouped: BTreeMap<Option<String>, Vec<Entry>> = BTreeMap::new();
    for (thread, entry) in entries {
        grouped.entry(thread).or_default().push(entry);
    }
    let mut groups: Vec<Group> = grouped
        .into_iter()
        .map(|(thread, mut entries)| {
            entries.sort_by(|a, b| (a.at(), a.rank()).cmp(&(b.at(), b.rank())));
            Group {
                first_at: entries.first().map(|e| e.at().to_string()).unwrap_or_default(),
                last_at: entries.last().map(|e| e.at().to_string()).unwrap_or_default(),
                thread: thread.map(|slug| match thread_titles.get(&slug) {
                    Some(title) => GroupThread { title: title.clone(), exists: true, slug },
                    None => GroupThread { title: "A deleted thread".to_string(), exists: false, slug },
                }),
                entries,
            }
        })
        .collect();
    groups.sort_by(|a, b| match (&a.thread, &b.thread) {
        (Some(x), Some(y)) => (&a.first_at, &x.slug).cmp(&(&b.first_at, &y.slug)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });

    let mut summary = Summary { versions: resolved.len() as i64, ..Summary::default() };
    for v in &resolved {
        match v.author {
            AI => summary.ai += 1,
            HUMAN => summary.human += 1,
            _ => summary.unrecorded += 1,
        }
        if let Some(level) = &v.mmm {
            summary.label = Some(LabelRef { level: level.clone(), version: v.version.clone() });
        }
    }

    let stopped = stopped_run(conn, project, base)?;
    let draft = draft_of(conn, project, base, &resolved)?.map(|(since, modified)| Draft { since, modified, stopped_run: stopped.clone() });
    Ok(Provenance {
        groups,
        draft,
        default_thread: default_thread(conn, project, base)?,
        stopped_run: stopped,
        history_has_ai: summary.ai > 0,
        suggested_label: summary.label.as_ref().map(|l| l.level.clone()),
        summary,
    })
}

/// Whether the live file holds something no version does: the newest version's number and when the file changed.
fn draft_of(conn: &Connection, project: &str, base: &str, resolved: &[Resolved]) -> Result<Option<(Option<String>, String)>> {
    let Some((live_size, live_modified, live_body)) = query_opt(
        conn,
        "SELECT size, modified, body FROM deliverables WHERE project_slug = ?1 AND path = ?2",
        [project, base],
        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?)),
    )?
    else {
        return Ok(None);
    };
    let Some(newest) = resolved.last() else {
        return Ok(Some((None, live_modified)));
    };
    let (size, modified, body) = query_opt(
        conn,
        "SELECT size, modified, body FROM deliverables WHERE project_slug = ?1 AND path = ?2",
        [project, newest.path.as_str()],
        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?)),
    )?
    .unwrap_or_default();
    let changed = match (&live_body, &body) {
        (Some(live), Some(snapshot)) => live != snapshot,
        _ => live_size != size || live_modified > modified,
    };
    Ok(changed.then(|| (Some(newest.version.clone()), live_modified)))
}
