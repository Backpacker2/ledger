//! Deliverables are the files prompts produce, kept in a project's `deliverables/` folder. They can be anything;
//! markdown ones can be read and edited here. Publishing freezes a copy as "<name>@v1.0.<ext>" next to the live
//! file, and that snapshot never changes.
//!
//! Paths in this module are relative to the deliverables folder and use `/`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};

use crate::db::{query_all, query_opt};
use crate::error::{Error, Result};
use crate::models::{Bump, DeliverableSummary, DeliverableView, PublishChoices};
use crate::posts::{self, AddOptions};
use crate::provenance::{self, Origin};
use crate::{feedback, frontmatter, ids, markdown, projects, prompts, uploads};

// ---- paths and versions -----------------------------------------------------------------------------

pub fn is_markdown(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown"))
}

/// "markdown", "image" or "other".
pub fn kind_of(path: &str) -> &'static str {
    if is_markdown(path) {
        "markdown"
    } else if uploads::is_image(path) {
        "image"
    } else {
        "other"
    }
}

pub fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

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

/// Splits "reports/plan@v1.2.md" into ("reports/plan.md", Some("1.2")); anything else is its own base.
pub fn split_version(path: &str) -> (String, Option<String>) {
    let (dir, file) = path.rsplit_once('/').map_or(("", path), |(d, f)| (d, f));
    let join = |file: String| if dir.is_empty() { file } else { format!("{dir}/{file}") };
    if let Some((base, version)) = file.rsplit_once("@v")
        && !base.is_empty()
        && parse_version(version).is_some()
    {
        return (join(base.to_string()), Some(version.to_string()));
    }
    let (stem, ext) = match file.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, Some(ext)),
        _ => (file, None),
    };
    if let Some((base, version)) = stem.rsplit_once("@v")
        && !base.is_empty()
        && parse_version(version).is_some()
    {
        let file = ext.map_or_else(|| base.to_string(), |ext| format!("{base}.{ext}"));
        return (join(file), Some(version.to_string()));
    }
    (path.to_string(), None)
}

/// "reports/plan.md" + "1.2" → "reports/plan@v1.2.md".
pub fn snapshot_path(base: &str, version: &str) -> String {
    let (dir, file) = base.rsplit_once('/').map_or(("", base), |(d, f)| (d, f));
    let file = match file.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{stem}@v{version}.{ext}"),
        _ => format!("{file}@v{version}"),
    };
    if dir.is_empty() { file } else { format!("{dir}/{file}") }
}

pub fn deliverables_dir(root: &Path, project: &str) -> PathBuf {
    uploads::project_dir(root, project).join("deliverables")
}

// ---- reading ----------------------------------------------------------------------------------------

pub fn exists(conn: &Connection, project: &str, path: &str) -> Result<bool> {
    Ok(query_opt(conn, "SELECT 1 FROM deliverables WHERE project_slug = ?1 AND path = ?2", [project, path], |_| Ok(()))?.is_some())
}

/// How many deliverables the project has (published versions are not counted separately).
pub fn count_live(conn: &Connection, project: &str) -> Result<i64> {
    let paths: Vec<String> = query_all(conn, "SELECT path FROM deliverables WHERE project_slug = ?1", [project], |r| r.get(0))?;
    Ok(paths.iter().filter(|p| split_version(p).1.is_none()).count() as i64)
}

/// Published versions of the deliverable at `base`, oldest first.
pub fn versions_of(conn: &Connection, project: &str, base: &str) -> Result<Vec<String>> {
    let paths: Vec<String> = query_all(conn, "SELECT path FROM deliverables WHERE project_slug = ?1", [project], |r| r.get(0))?;
    let mut versions: Vec<(u32, u32, String)> = paths
        .iter()
        .filter_map(|p| match split_version(p) {
            (b, Some(v)) if b == base => parse_version(&v).map(|(major, minor)| (major, minor, v)),
            _ => None,
        })
        .collect();
    versions.sort();
    Ok(versions.into_iter().map(|(_, _, v)| v).collect())
}

#[derive(Default)]
struct Group {
    live: Option<(i64, String)>,
    snapshots: Vec<(u32, u32, String, i64, String)>,
}

/// Every deliverable, by name. A deliverable whose live file is gone but which has published versions is still
/// listed (with `exists` false).
pub fn list(conn: &Connection, project: &str) -> Result<Vec<DeliverableSummary>> {
    let rows: Vec<(String, i64, String)> = query_all(
        conn,
        "SELECT path, size, modified FROM deliverables WHERE project_slug = ?1",
        [project],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;

    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    for (path, size, modified) in rows {
        let (base, version) = split_version(&path);
        let group = groups.entry(base).or_default();
        match version.as_deref().and_then(|v| parse_version(v).map(|p| (p, v))) {
            Some(((major, minor), v)) => group.snapshots.push((major, minor, v.to_string(), size, modified)),
            None => group.live = Some((size, modified)),
        }
    }

    // A prompt counts once for a deliverable, whether it is linked to the file or to some of its published versions.
    let mut prompt_counts: HashMap<String, i64> = HashMap::new();
    let mut counted: HashSet<(String, String)> = HashSet::new();
    for (prompt, path) in query_all::<(String, String), _>(
        conn,
        "SELECT d.prompt_id, d.path FROM prompt_deliverables d JOIN prompts p ON p.id = d.prompt_id WHERE p.project_slug = ?1",
        [project],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )? {
        let base = split_version(&path).0;
        if counted.insert((prompt, base.clone())) {
            *prompt_counts.entry(base).or_default() += 1;
        }
    }
    let mut open_feedback: HashMap<String, i64> = HashMap::new();
    for path in query_all::<String, _>(conn, "SELECT deliverable_path FROM feedback WHERE project_slug = ?1 AND status = 'open'", [project], |r| r.get(0))? {
        *open_feedback.entry(split_version(&path).0).or_default() += 1;
    }

    let mut out: Vec<DeliverableSummary> = groups
        .into_iter()
        .map(|(base, mut group)| {
            group.snapshots.sort();
            let (size, modified) = group
                .live
                .clone()
                .or_else(|| group.snapshots.last().map(|s| (s.3, s.4.clone())))
                .unwrap_or_default();
            DeliverableSummary {
                name: file_name(&base).to_string(),
                kind: kind_of(&base).to_string(),
                size,
                modified,
                exists: group.live.is_some(),
                versions: group.snapshots.iter().map(|s| s.2.clone()).collect(),
                prompt_count: prompt_counts.get(&base).copied().unwrap_or(0),
                open_feedback: open_feedback.get(&base).copied().unwrap_or(0),
                path: base,
            }
        })
        .collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.path.cmp(&b.path)));
    Ok(out)
}

/// A deliverable, or — if `path` is a "<name>@v1.0.<ext>" snapshot — that published version.
pub fn get(conn: &Connection, project: &str, path: &str) -> Result<DeliverableView> {
    let (size, modified, body): (i64, String, Option<String>) = query_opt(
        conn,
        "SELECT size, modified, body FROM deliverables WHERE project_slug = ?1 AND path = ?2",
        [project, path],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?
    .ok_or_else(|| Error::not_found("Deliverable not found"))?;

    let (base, version) = split_version(path);
    let html = body.as_ref().map(|text| markdown::render(&frontmatter::parse(text).map_or_else(|| text.clone(), |p| p.body)));
    Ok(DeliverableView {
        path: path.to_string(),
        name: file_name(&base).to_string(),
        kind: kind_of(&base).to_string(),
        editable: version.is_none() && body.is_some(),
        versions: versions_of(conn, project, &base)?,
        exists: true,
        prompts: prompts::producing(conn, project, &base)?,
        feedback: feedback::list_for_deliverable(conn, project, &base)?,
        base_path: base,
        version,
        size,
        modified,
        body,
        html,
    })
}

// ---- writing ----------------------------------------------------------------------------------------

/// Records a file found in the deliverables folder (or written there by this app).
pub fn upsert(conn: &Connection, project: &str, path: &str, size: i64, modified: &str, body: Option<&str>) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO deliverables (project_slug, path, size, modified, body) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![project, path, size, modified, body],
    )?;
    Ok(())
}

/// A new markdown deliverable, not yet produced by any prompt (link it from a prompt, or just write it).
pub fn create(conn: &Connection, project: &str, name: &str, body: &str) -> Result<DeliverableView> {
    projects::require(conn, project)?;
    // "plan.md" means the markdown file "plan", not one called "plan-md".
    let name = name.trim();
    let name = name.strip_suffix(".md").or_else(|| name.strip_suffix(".MD")).unwrap_or(name).trim();
    if name.is_empty() {
        return Err(Error::invalid("Name is required"));
    }
    let stem = ids::slugify_as(name, "deliverable");
    let mut path = format!("{stem}.md");
    let mut n = 2;
    while exists(conn, project, &path)? {
        path = format!("{stem}-{n}.md");
        n += 1;
    }
    let mut text = if body.trim().is_empty() { format!("# {name}\n") } else { body.to_string() };
    if !text.ends_with('\n') {
        text.push('\n');
    }
    upsert(conn, project, &path, text.len() as i64, &ids::now(), Some(&text))?;
    get(conn, project, &path)
}

/// Saves the text of a markdown deliverable. Published versions can't be edited.
pub fn save(conn: &Connection, project: &str, path: &str, body: &str) -> Result<DeliverableView> {
    let view = get(conn, project, path)?;
    if !view.editable {
        return Err(Error::invalid("Only the current version of a markdown deliverable can be edited"));
    }
    conn.execute(
        "UPDATE deliverables SET body = ?3, size = ?4, modified = ?5 WHERE project_slug = ?1 AND path = ?2",
        params![project, path, body, body.len() as i64, ids::now()],
    )?;
    get(conn, project, path)
}

/// Freezes the current file as the next version, as a person typed it, and notes it in the thread the version is filed
/// under (see [`provenance::default_thread`]). The page uses [`publish_with`]; this is the publish with every default.
#[cfg(test)]
pub fn publish(conn: &Connection, root: &Path, project: &str, path: &str, bump: Bump, explicit: Option<&str>) -> Result<DeliverableView> {
    publish_with(conn, root, project, path, bump, explicit, &PublishChoices::default())
}

/// [`publish`], with what the person said about the change: the thread, whether AI made it, its AI-use label.
pub fn publish_with(
    conn: &Connection,
    root: &Path,
    project: &str,
    path: &str,
    bump: Bump,
    explicit: Option<&str>,
    choices: &PublishChoices,
) -> Result<DeliverableView> {
    let (base, version) = split_version(path);
    if version.is_some() {
        return Err(Error::invalid("Publish the current version, not a published one"));
    }
    let origin = provenance::manual_origin(conn, project, &base, choices)?;
    freeze(conn, root, project, &base, bump, explicit, None, &origin)?;
    get(conn, project, &base)
}

/// What a run of a prompt publishes: the next minor version, noted in the prompt's thread. With `held` it is that
/// content (what the file said before the run changed it) rather than the file as it is now. Returns the version,
/// like "0.2"; the snapshot is `snapshot_path(base, version)`.
pub fn publish_run(conn: &Connection, root: &Path, project: &str, base: &str, held: Option<&[u8]>, origin: &Origin) -> Result<String> {
    freeze(conn, root, project, base, Bump::Minor, None, held, origin)
}

/// Writes the snapshot of the live file at `base` (or of `held`, when given) as the next version, records how it came to
/// be, and when the origin has a thread notes it there. Returns the version.
#[allow(clippy::too_many_arguments)]
fn freeze(
    conn: &Connection,
    root: &Path,
    project: &str,
    base: &str,
    bump: Bump,
    explicit: Option<&str>,
    held: Option<&[u8]>,
    origin: &Origin,
) -> Result<String> {
    let live = get(conn, project, base)?;

    let existing = live.versions.clone();
    let version = match explicit.map(str::trim).filter(|v| !v.is_empty()) {
        Some(v) => {
            let (major, minor) = parse_version(v).ok_or_else(|| Error::invalid("Version must be in M.m format, e.g. 1.4"))?;
            format!("{major}.{minor}")
        }
        // Bump from the highest published version, so a custom version earlier on never collides.
        None => bump_version(existing.last().map(String::as_str), bump),
    };
    if existing.contains(&version) {
        return Err(Error::invalid(format!("Version {version} is already published")));
    }

    let snapshot = snapshot_path(base, &version);
    let at = ids::now();
    let tx = conn.unchecked_transaction()?;
    let sha256 = match &live.body {
        Some(body) => {
            let held_text = held.map(|bytes| String::from_utf8_lossy(bytes).into_owned());
            let text = held_text.as_deref().unwrap_or(body);
            upsert(&tx, project, &snapshot, text.len() as i64, &at, Some(text))?;
            provenance::sha256_hex(text.as_bytes())
        }
        None => {
            let to = deliverables_dir(root, project).join(&snapshot);
            match held {
                Some(bytes) => fs::write(&to, bytes).map_err(|e| Error::io(&to, e))?,
                None => {
                    let from = deliverables_dir(root, project).join(base);
                    fs::copy(&from, &to).map_err(|e| Error::io(&to, e))?;
                }
            }
            upsert(&tx, project, &snapshot, held.map_or(live.size, |bytes| bytes.len() as i64), &at, None)?;
            provenance::sha256_hex(&fs::read(&to).map_err(|e| Error::io(&to, e))?)
        }
    };
    if let Some(thread) = &origin.thread {
        posts::add(
            &tx,
            project,
            thread,
            &provenance::announcement(&tx, project, &live.name, &version, origin)?,
            AddOptions {
                version_published: true,
                deliverable_path: Some(base.to_string()),
                deliverable_version: Some(version.clone()),
                prompt_id: origin.prompt.clone(),
                ..Default::default()
            },
        )?;
    }
    provenance::insert_record(&tx, project, base, &version, origin, &sha256, &at)?;
    tx.commit()?;
    Ok(version)
}

/// Deletes the current file; its published versions stay. A markdown file leaves with the next write-through; any
/// other kind is removed from the folder here.
pub fn delete(conn: &Connection, root: &Path, project: &str, path: &str) -> Result<()> {
    let view = get(conn, project, path)?;
    if view.version.is_some() {
        return Err(Error::invalid("Published versions are kept"));
    }
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM deliverables WHERE project_slug = ?1 AND path = ?2", [project, path])?;
    tx.execute(
        "DELETE FROM prompt_deliverables WHERE path = ?2 AND prompt_id IN (SELECT id FROM prompts WHERE project_slug = ?1)",
        [project, path],
    )?;
    tx.commit()?;
    if view.body.is_none() {
        let file = deliverables_dir(root, project).join(path);
        if let Err(e) = fs::remove_file(&file)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            return Err(Error::io(&file, e));
        }
    }
    Ok(())
}
