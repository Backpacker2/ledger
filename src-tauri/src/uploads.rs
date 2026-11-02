//! Files uploaded into a thread. The file itself goes in the project's `attachments/` folder (whatever the
//! upload was: a file picked in the thread, or a screenshot); the thread gets a manifest — when it was uploaded,
//! where the file is, and a description.

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, Row, params};

use crate::db::{query_all, query_opt};
use crate::error::{Error, Result};
use crate::models::UploadView;
use crate::{ids, markdown, threads};

const COLUMNS: &str = "id, thread_slug, file, name, size, uploaded_at, description";
const IMAGE_EXTENSIONS: [&str; 7] = ["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp"];

pub fn project_dir(root: &Path, project: &str) -> PathBuf {
    root.join("projects").join(project)
}

pub fn is_image(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, ext)| IMAGE_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
}

fn view(root: &Path, project: &str) -> impl Fn(&Row<'_>) -> rusqlite::Result<UploadView> {
    let dir = project_dir(root, project);
    move |r| {
        let (file, name, description): (String, String, String) = (r.get(2)?, r.get(3)?, r.get(6)?);
        Ok(UploadView {
            id: r.get(0)?,
            thread_slug: r.get(1)?,
            image: is_image(&name),
            exists: dir.join(&file).is_file(),
            file,
            name,
            size: r.get(4)?,
            uploaded_at: r.get(5)?,
            description_html: markdown::render(&description),
            description,
        })
    }
}

/// Oldest first.
pub fn list(conn: &Connection, root: &Path, project: &str, thread: &str) -> Result<Vec<UploadView>> {
    query_all(
        conn,
        &format!("SELECT {COLUMNS} FROM uploads WHERE project_slug = ?1 AND thread_slug = ?2 ORDER BY uploaded_at, rowid"),
        [project, thread],
        view(root, project),
    )
}

fn get(conn: &Connection, root: &Path, project: &str, thread: &str, id: &str) -> Result<UploadView> {
    query_opt(
        conn,
        &format!("SELECT {COLUMNS} FROM uploads WHERE project_slug = ?1 AND thread_slug = ?2 AND id = ?3"),
        [project, thread, id],
        view(root, project),
    )?
    .ok_or_else(|| Error::not_found("Upload not found"))
}

pub fn files_of_thread(conn: &Connection, project: &str, thread: &str) -> Result<Vec<String>> {
    query_all(conn, "SELECT file FROM uploads WHERE project_slug = ?1 AND thread_slug = ?2", [project, thread], |r| r.get(0))
}

/// Copies files from anywhere on this machine into the project's attachments and records each in the thread.
pub fn add_files(conn: &Connection, root: &Path, project: &str, thread: &str, sources: &[PathBuf], description: &str) -> Result<Vec<UploadView>> {
    threads::require(conn, project, thread)?;
    let mut added = vec![];
    for source in sources {
        if !source.is_file() {
            return Err(Error::invalid(format!("{} isn't a file", source.display())));
        }
        let name = source.file_name().and_then(|n| n.to_str()).unwrap_or("file");
        let target = place(root, project, name)?;
        fs::copy(source, &target.0).map_err(|e| Error::io(&target.0, e))?;
        let size = fs::metadata(&target.0).map_or(0, |m| m.len());
        added.push(insert(conn, root, project, thread, &target.1, size, description)?);
    }
    Ok(added)
}

/// Saves bytes (a screenshot, say) as a new attachment and records it in the thread.
pub fn add_bytes(conn: &Connection, root: &Path, project: &str, thread: &str, name: &str, bytes: &[u8], description: &str) -> Result<UploadView> {
    threads::require(conn, project, thread)?;
    let target = place(root, project, name)?;
    fs::write(&target.0, bytes).map_err(|e| Error::io(&target.0, e))?;
    insert(conn, root, project, thread, &target.1, bytes.len() as u64, description)
}

pub fn set_description(conn: &Connection, root: &Path, project: &str, thread: &str, id: &str, description: &str) -> Result<UploadView> {
    let changed = conn.execute(
        "UPDATE uploads SET description = ?4 WHERE project_slug = ?1 AND thread_slug = ?2 AND id = ?3",
        params![project, thread, id, description.trim()],
    )?;
    if changed == 0 {
        return Err(Error::not_found("Upload not found"));
    }
    get(conn, root, project, thread, id)
}

/// Removes the manifest, and the file too unless another upload still points at it.
pub fn delete(conn: &Connection, root: &Path, project: &str, thread: &str, id: &str) -> Result<()> {
    let file: Option<String> = query_opt(
        conn,
        "SELECT file FROM uploads WHERE project_slug = ?1 AND thread_slug = ?2 AND id = ?3",
        [project, thread, id],
        |r| r.get(0),
    )?;
    conn.execute("DELETE FROM uploads WHERE project_slug = ?1 AND thread_slug = ?2 AND id = ?3", [project, thread, id])?;
    if let Some(file) = file {
        remove_unreferenced(conn, root, project, &[file]);
    }
    Ok(())
}

/// Deletes attachment files that no upload manifest refers to any more. Only ever touches `attachments/`.
pub fn remove_unreferenced(conn: &Connection, root: &Path, project: &str, files: &[String]) {
    for file in files {
        if !file.starts_with("attachments/") || file.contains("..") {
            continue;
        }
        let still_used = conn
            .query_row("SELECT COUNT(*) FROM uploads WHERE project_slug = ?1 AND file = ?2", [project, file], |r| r.get::<_, i64>(0))
            .unwrap_or(1);
        if still_used == 0 {
            let _ = fs::remove_file(project_dir(root, project).join(file));
        }
    }
}

fn insert(conn: &Connection, root: &Path, project: &str, thread: &str, file: &str, size: u64, description: &str) -> Result<UploadView> {
    let ids::Stamp { id, at } = ids::new_stamp();
    let name = file.rsplit('/').next().unwrap_or(file);
    conn.execute(
        "INSERT INTO uploads (id, project_slug, thread_slug, file, name, size, uploaded_at, description)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![id, project, thread, file, name, size as i64, at, description.trim()],
    )?;
    get(conn, root, project, thread, &id)
}

/// A free spot in the project's attachments: the full path to write to, and the path as recorded
/// ("attachments/<name>"). A name that is taken gets a number.
fn place(root: &Path, project: &str, name: &str) -> Result<(PathBuf, String)> {
    let dir = project_dir(root, project).join("attachments");
    fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;

    let clean = sanitize(name);
    let (stem, ext) = match clean.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem.to_string(), format!(".{ext}")),
        _ => (clean.clone(), String::new()),
    };
    let mut candidate = clean;
    let mut n = 2;
    while dir.join(&candidate).exists() {
        candidate = format!("{stem}-{n}{ext}");
        n += 1;
    }
    Ok((dir.join(&candidate), format!("attachments/{candidate}")))
}

/// A file name that is safe on every platform.
fn sanitize(name: &str) -> String {
    let cleaned: String = name.chars().map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { '_' } else { c }).collect();
    let cleaned = cleaned.trim().trim_matches('.').chars().take(120).collect::<String>();
    if cleaned.is_empty() { "file".to_string() } else { cleaned }
}
