//! The vault: a folder of markdown files with YAML frontmatter, laid out the way an Obsidian vault would be, so
//! it can be read, edited and searched with any other tool.
//!
//! ```text
//! threads/<slug>/thread.md          title, createdAt, linkedThreadSlugs
//! threads/<slug>/posts/<id>.md      one post: frontmatter (parent, flags, timestamps) and the markdown body
//! documents/<id>.md                 the live draft of a document
//! documents/<id>@v<M.m>.md          a published version, frozen
//! document-types/<slug>.md          name, trigger ("use when…"), questions
//! user-stories/<id>.md              the story, its acceptance criteria and linked threads
//! planboard.md                      the columns
//! ```
//!
//! Files are the source of truth. [`Vault::load`] reads them into the in-memory database; after every change
//! [`Vault::persist`] writes the difference back. Persisting is a diff of the *whole* database's canonical file
//! rendering against what was last written, which makes it self-healing (a failed write is retried by the next
//! one) and keeps "what is on disk" a pure function of "what is in memory".

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use rusqlite::{Connection, params};
use serde_yaml_ng::{Mapping, Value};

use crate::db::{self, query_all};
use crate::error::{Error, Result};
use crate::frontmatter::{self, MetaExt};
use crate::{documents, ids, planboard, settings, stories};

const MANAGED_DIRS: [&str; 4] = ["threads", "documents", "document-types", "user-stories"];
const PLANBOARD_FILE: &str = "planboard.md";

// ---- layout -----------------------------------------------------------------------------------------

fn thread_file(slug: &str) -> String {
    format!("threads/{slug}/thread.md")
}

fn post_file(slug: &str, id: &str) -> String {
    format!("threads/{slug}/posts/{id}.md")
}

fn type_file(slug: &str) -> String {
    format!("document-types/{slug}.md")
}

fn document_file(id: &str) -> String {
    format!("documents/{id}.md")
}

fn snapshot_file(id: &str, version: &str) -> String {
    format!("documents/{id}@v{version}.md")
}

fn story_file(id: &str) -> String {
    format!("user-stories/{id}.md")
}

// ---- the vault on disk ------------------------------------------------------------------------------

/// A file's size and modification time: enough to notice that something else touched it.
type Fingerprint = (u64, u128);

pub struct Vault {
    root: PathBuf,
    /// The hash of the canonical text of every file this vault owns, as last written (or read).
    written: HashMap<String, u64>,
    /// What the managed folders looked like when we last wrote or read them.
    disk: BTreeMap<String, Fingerprint>,
}

impl Vault {
    /// Creates the folder if needed, along with a `.gitignore` that keeps local-only files out of GitHub.
    pub fn open(root: PathBuf) -> Result<Vault> {
        fs::create_dir_all(&root).map_err(|e| Error::io(&root, e))?;
        settings::ensure_gitignore(&root)?;
        Ok(Vault { root, written: HashMap::new(), disk: BTreeMap::new() })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Reads every file into a fresh in-memory database. Anything the files leave out or get wrong is repaired
    /// on the way in (a document whose type is missing gets a placeholder type, a link to a thread that does
    /// not exist is dropped) and written back out by the persist at the end.
    pub fn load(&mut self) -> Result<Connection> {
        let conn = db::open_in_memory()?;
        let listing = managed_listing(&self.root);
        import(&conn, &self.root, &listing)?;

        // A file that is already on disk is taken to match its canonical rendering, so that opening a vault
        // someone formatted by hand does not rewrite all of it.
        self.written.clear();
        for (path, content) in export(&conn)? {
            if listing.contains_key(&path) {
                self.written.insert(path, hash(&content));
            }
        }
        self.disk = listing;

        self.persist(&conn)?;
        Ok(conn)
    }

    /// Writes whatever differs between the database and the files, and deletes files whose thing is gone.
    /// Returns how many files were touched.
    pub fn persist(&mut self, conn: &Connection) -> Result<usize> {
        let canonical = export(conn)?;
        let mut touched = 0;

        for (path, content) in &canonical {
            let digest = hash(content);
            if self.written.get(path) == Some(&digest) {
                continue;
            }
            let file = self.root.join(path);
            write_atomic(&file, content).map_err(|e| Error::io(&file, e))?;
            self.written.insert(path.clone(), digest);
            if let Some(fp) = fingerprint(&file) {
                self.disk.insert(path.clone(), fp);
            }
            touched += 1;
        }

        let gone: Vec<String> = self.written.keys().filter(|p| !canonical.contains_key(*p)).cloned().collect();
        for path in gone {
            let file = self.root.join(&path);
            match fs::remove_file(&file) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(Error::io(&file, e)),
            }
            self.written.remove(&path);
            self.disk.remove(&path);
            prune_empty_dirs(&self.root, &file);
            touched += 1;
        }
        Ok(touched)
    }

    /// Whether files were added, removed or edited by something other than this vault since it last looked.
    pub fn changed_on_disk(&self) -> bool {
        managed_listing(&self.root) != self.disk
    }
}

fn hash(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

fn fingerprint(file: &Path) -> Option<Fingerprint> {
    let meta = fs::metadata(file).ok()?;
    let nanos = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_nanos();
    Some((meta.len(), nanos))
}

/// A file's modification time as an ISO timestamp, for files that don't say when they were created.
fn stamp_of(fp: Option<&Fingerprint>) -> String {
    fp.map_or_else(ids::now, |(_, nanos)| ids::format_millis((nanos / 1_000_000) as i64))
}

/// Every markdown file in the folders the app manages, with its fingerprint. Dotfiles (including the temporary
/// files writes go through) are not part of the vault.
fn managed_listing(root: &Path) -> BTreeMap<String, Fingerprint> {
    fn walk(root: &Path, rel: &str, out: &mut BTreeMap<String, Fingerprint>) {
        let Ok(entries) = fs::read_dir(root.join(rel)) else { return };
        for entry in entries.flatten() {
            let Some(name) = entry.file_name().to_str().map(str::to_string) else { continue };
            if name.starts_with('.') {
                continue;
            }
            let child = format!("{rel}/{name}");
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                walk(root, &child, out);
            } else if name.ends_with(".md")
                && let Some(fp) = fingerprint(&entry.path())
            {
                out.insert(child, fp);
            }
        }
    }

    let mut out = BTreeMap::new();
    for dir in MANAGED_DIRS {
        walk(root, dir, &mut out);
    }
    if let Some(fp) = fingerprint(&root.join(PLANBOARD_FILE)) {
        out.insert(PLANBOARD_FILE.to_string(), fp);
    }
    out
}

/// Replaces the file in one step, so a crash mid-write never leaves half a note behind.
fn write_atomic(file: &Path, content: &str) -> std::io::Result<()> {
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent)?;
    }
    let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let temp = file.with_file_name(format!(".{name}.tmp"));
    fs::write(&temp, content)?;
    fs::rename(&temp, file)
}

/// After a delete, removes the folders it left empty, stopping at the vault root.
fn prune_empty_dirs(root: &Path, file: &Path) {
    let mut dir = file.parent();
    while let Some(current) = dir {
        if current == root || !current.starts_with(root) || fs::remove_dir(current).is_err() {
            break;
        }
        dir = current.parent();
    }
}

// ---- export: database → files -----------------------------------------------------------------------

/// The canonical text of every file, keyed by path relative to the vault root (always with `/`).
pub fn export(conn: &Connection) -> Result<BTreeMap<String, String>> {
    let mut files = BTreeMap::new();
    export_threads(conn, &mut files)?;
    export_types(conn, &mut files)?;
    export_documents(conn, &mut files)?;
    export_stories(conn, &mut files)?;
    export_planboard(conn, &mut files)?;
    Ok(files)
}

fn export_threads(conn: &Connection, files: &mut BTreeMap<String, String>) -> Result<()> {
    let mut links: HashMap<String, Vec<String>> = HashMap::new();
    for (a, b) in query_all(conn, "SELECT a, b FROM thread_links ORDER BY a, b", [], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        links.entry(a.clone()).or_default().push(b.clone());
        links.entry(b).or_default().push(a);
    }

    for (slug, title, created_at) in query_all(conn, "SELECT slug, title, created_at FROM threads", [], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
    })? {
        let mut linked = links.remove(&slug).unwrap_or_default();
        linked.sort();

        let mut meta = Mapping::new();
        frontmatter::set(&mut meta, "title", title);
        frontmatter::set(&mut meta, "createdAt", created_at);
        frontmatter::set_list(&mut meta, "linkedThreadSlugs", &linked);
        files.insert(thread_file(&slug), frontmatter::render(&meta, ""));
    }

    type PostRow = (String, String, Option<String>, bool, bool, Option<String>, Option<String>, String, String, String);
    for (id, slug, parent, guiding, published, doc_id, doc_version, created_at, updated_at, body) in query_all::<PostRow, _>(
        conn,
        "SELECT id, thread_slug, parent_id, guiding_question, version_published, document_id, document_version,
                created_at, updated_at, body FROM posts",
        [],
        |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?))
        },
    )? {
        let mut meta = Mapping::new();
        frontmatter::set(&mut meta, "id", id.clone());
        frontmatter::set_opt(&mut meta, "parentId", parent.as_deref());
        frontmatter::set_flag(&mut meta, "guidingQuestion", guiding);
        frontmatter::set_flag(&mut meta, "versionPublished", published);
        frontmatter::set_opt(&mut meta, "documentId", doc_id.as_deref());
        frontmatter::set_opt(&mut meta, "documentVersion", doc_version.as_deref());
        frontmatter::set(&mut meta, "createdAt", created_at);
        frontmatter::set(&mut meta, "updatedAt", updated_at);
        files.insert(post_file(&slug, &id), frontmatter::render(&meta, &body));
    }
    Ok(())
}

fn export_types(conn: &Connection, files: &mut BTreeMap<String, String>) -> Result<()> {
    let mut questions: HashMap<String, Vec<String>> = HashMap::new();
    for (slug, question) in query_all(
        conn,
        "SELECT type_slug, question FROM document_type_questions ORDER BY type_slug, position",
        [],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    )? {
        questions.entry(slug).or_default().push(question);
    }

    for (slug, name, trigger, created_at, updated_at) in query_all(
        conn,
        "SELECT slug, name, \"trigger\", created_at, updated_at FROM document_types",
        [],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?)),
    )? {
        let mut meta = Mapping::new();
        frontmatter::set(&mut meta, "name", name);
        frontmatter::set_opt(&mut meta, "trigger", Some(&trigger));
        frontmatter::set_list(&mut meta, "questions", &questions.remove(&slug).unwrap_or_default());
        frontmatter::set(&mut meta, "createdAt", created_at);
        frontmatter::set(&mut meta, "updatedAt", updated_at);
        files.insert(type_file(&slug), frontmatter::render(&meta, ""));
    }
    Ok(())
}

fn export_documents(conn: &Connection, files: &mut BTreeMap<String, String>) -> Result<()> {
    type DocRow = (String, String, String, Option<String>, Option<String>, String, String, String);
    for (id, type_slug, title, thread, version, created_at, updated_at, body) in query_all::<DocRow, _>(
        conn,
        "SELECT id, document_type_slug, title, thread_slug, version, created_at, updated_at, body FROM documents",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?)),
    )? {
        let mut meta = Mapping::new();
        frontmatter::set(&mut meta, "id", id.clone());
        frontmatter::set(&mut meta, "documentTypeSlug", type_slug);
        frontmatter::set(&mut meta, "title", title);
        frontmatter::set_opt(&mut meta, "threadSlug", thread.as_deref());
        frontmatter::set_opt(&mut meta, "version", version.as_deref());
        frontmatter::set(&mut meta, "createdAt", created_at);
        frontmatter::set(&mut meta, "updatedAt", updated_at);
        files.insert(document_file(&id), frontmatter::render(&meta, &body));
    }

    for (id, version, type_slug, title, body, published_at) in query_all(
        conn,
        "SELECT document_id, version, document_type_slug, title, body, published_at FROM document_versions",
        [],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?)),
    )? {
        let mut meta = Mapping::new();
        frontmatter::set(&mut meta, "id", id.clone());
        frontmatter::set(&mut meta, "documentTypeSlug", type_slug);
        frontmatter::set(&mut meta, "title", title);
        frontmatter::set(&mut meta, "version", version.clone());
        frontmatter::set(&mut meta, "publishedAt", published_at);
        files.insert(snapshot_file(&id, &version), frontmatter::render(&meta, &body));
    }
    Ok(())
}

fn export_stories(conn: &Connection, files: &mut BTreeMap<String, String>) -> Result<()> {
    for story in stories::list(conn)? {
        let mut meta = Mapping::new();
        frontmatter::set(&mut meta, "id", story.id.clone());
        frontmatter::set(&mut meta, "summary", story.summary);
        frontmatter::set_opt(&mut meta, "role", Some(&story.role));
        frontmatter::set_opt(&mut meta, "want", Some(&story.want));
        frontmatter::set_opt(&mut meta, "reason", Some(&story.reason));
        frontmatter::set(&mut meta, "columnId", story.column_id);
        let slugs: Vec<String> = story.threads.iter().map(|t| t.slug.clone()).collect();
        frontmatter::set_list(&mut meta, "threadSlugs", &slugs);
        if !story.criteria.is_empty() {
            let criteria = story
                .criteria
                .iter()
                .map(|c| {
                    let mut item = Mapping::new();
                    frontmatter::set(&mut item, "text", c.text.clone());
                    frontmatter::set(&mut item, "done", c.done);
                    Value::Mapping(item)
                })
                .collect();
            frontmatter::set(&mut meta, "criteria", Value::Sequence(criteria));
        }
        frontmatter::set(&mut meta, "createdAt", story.created_at);
        frontmatter::set(&mut meta, "updatedAt", story.updated_at);
        files.insert(story_file(&story.id), frontmatter::render(&meta, ""));
    }
    Ok(())
}

fn export_planboard(conn: &Connection, files: &mut BTreeMap<String, String>) -> Result<()> {
    let columns = planboard::columns(conn)?
        .into_iter()
        .map(|c| {
            let mut item = Mapping::new();
            frontmatter::set(&mut item, "id", c.id);
            frontmatter::set(&mut item, "name", c.name);
            Value::Mapping(item)
        })
        .collect();

    let mut meta = Mapping::new();
    frontmatter::set(&mut meta, "columns", Value::Sequence(columns));
    frontmatter::set(&mut meta, "backlogColumnId", planboard::backlog_column_id(conn)?);
    files.insert(PLANBOARD_FILE.to_string(), frontmatter::render(&meta, ""));
    Ok(())
}

// ---- import: files → database -----------------------------------------------------------------------

struct Source<'a> {
    root: &'a Path,
    listing: &'a BTreeMap<String, Fingerprint>,
}

impl Source<'_> {
    /// A file's text; a file that is unreadable is treated as absent, one that is not UTF-8 is read lossily.
    fn read(&self, rel: &str) -> Option<String> {
        fs::read(self.root.join(rel)).ok().map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
    }

    fn stamp(&self, rel: &str) -> String {
        stamp_of(self.listing.get(rel))
    }

    /// The names (without `.md`) of the markdown files directly inside a folder.
    fn stems(&self, rel_dir: &str) -> Vec<String> {
        let prefix = format!("{rel_dir}/");
        self.listing
            .keys()
            .filter_map(|p| p.strip_prefix(&prefix))
            .filter(|rest| !rest.contains('/'))
            .filter_map(|name| name.strip_suffix(".md"))
            .map(str::to_string)
            .collect()
    }

    /// Frontmatter and body of a file; a file without frontmatter is all body.
    fn parsed(&self, rel: &str) -> Option<(Mapping, String)> {
        let text = self.read(rel)?;
        Some(match frontmatter::parse(&text) {
            Some(parsed) => (parsed.meta, parsed.body),
            None => (Mapping::new(), text.trim().to_string()),
        })
    }
}

fn import(conn: &Connection, root: &Path, listing: &BTreeMap<String, Fingerprint>) -> Result<()> {
    let source = Source { root, listing };
    let tx = conn.unchecked_transaction()?;

    import_planboard(&tx, &source)?;
    import_types(&tx, &source)?;
    let thread_slugs = import_threads(&tx, &source)?;
    import_documents(&tx, &source, &thread_slugs)?;
    import_stories(&tx, &source, &thread_slugs)?;

    tx.commit()?;
    Ok(())
}

fn import_planboard(conn: &Connection, source: &Source) -> Result<()> {
    let Some((meta, _)) = source.parsed(PLANBOARD_FILE) else { return Ok(()) };

    let mut columns: Vec<(String, String)> = vec![];
    for item in meta.maps("columns") {
        let Some(name) = item.text("name") else { continue };
        let id = item.text("id").unwrap_or_else(|| ids::slugify(&name));
        if !columns.iter().any(|(existing, _)| *existing == id) {
            columns.push((id, name));
        }
    }
    if columns.is_empty() {
        return Ok(()); // keep the default columns
    }

    conn.execute("DELETE FROM planboard_columns", [])?;
    for (position, (id, name)) in columns.iter().enumerate() {
        conn.execute("INSERT INTO planboard_columns (id, name, position) VALUES (?1, ?2, ?3)", params![id, name, position as i64])?;
    }
    if let Some(backlog) = meta.text("backlogColumnId").filter(|b| columns.iter().any(|(id, _)| id == b)) {
        conn.execute("UPDATE settings SET value = ?1 WHERE key = 'backlog_column_id'", [backlog])?;
    }
    Ok(())
}

fn import_types(conn: &Connection, source: &Source) -> Result<()> {
    for slug in source.stems("document-types") {
        let rel = type_file(&slug);
        let Some((meta, _)) = source.parsed(&rel) else { continue };
        let fallback = source.stamp(&rel);
        let created_at = meta.text("createdAt").unwrap_or_else(|| fallback.clone());
        let updated_at = meta.text("updatedAt").unwrap_or_else(|| created_at.clone());
        conn.execute(
            "INSERT OR IGNORE INTO document_types (slug, name, \"trigger\", created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![slug, meta.text("name").unwrap_or_else(|| slug.clone()), meta.text("trigger").unwrap_or_default(), created_at, updated_at],
        )?;
        for (position, question) in meta.list("questions").iter().enumerate() {
            conn.execute(
                "INSERT OR IGNORE INTO document_type_questions (type_slug, position, question) VALUES (?1, ?2, ?3)",
                params![slug, position as i64, question],
            )?;
        }
    }
    Ok(())
}

fn import_threads(conn: &Connection, source: &Source) -> Result<HashSet<String>> {
    // A thread is a folder under threads/. Its thread.md names it; a folder holding posts but no thread.md is
    // still a thread, titled after its folder.
    let mut slugs: Vec<String> = source
        .listing
        .keys()
        .filter_map(|p| p.strip_prefix("threads/"))
        .filter_map(|rest| rest.split('/').next())
        .map(str::to_string)
        .collect();
    slugs.sort();
    slugs.dedup();

    let mut known = HashSet::new();
    let mut links: Vec<(String, Vec<String>)> = vec![];
    for slug in &slugs {
        let rel = thread_file(slug);
        let (meta, _) = source.parsed(&rel).unwrap_or_default();
        conn.execute(
            "INSERT OR IGNORE INTO threads (slug, title, created_at) VALUES (?1, ?2, ?3)",
            params![slug, meta.text("title").unwrap_or_else(|| slug.clone()), meta.text("createdAt").unwrap_or_else(|| source.stamp(&rel))],
        )?;
        known.insert(slug.clone());
        links.push((slug.clone(), meta.list("linkedThreadSlugs")));
    }

    // A link counts if either side lists it; one that points nowhere is dropped.
    for (slug, others) in links {
        for other in others.into_iter().filter(|o| *o != slug && known.contains(o)) {
            let (a, b) = if slug < other { (&slug, &other) } else { (&other, &slug) };
            conn.execute("INSERT OR IGNORE INTO thread_links (a, b) VALUES (?1, ?2)", [a, b])?;
        }
    }

    type Post = (String, String, Option<String>, bool, bool, Option<String>, Option<String>, String, String, String);
    let mut posts: Vec<Post> = vec![];
    for slug in &slugs {
        for stem in source.stems(&format!("threads/{slug}/posts")) {
            let rel = post_file(slug, &stem);
            let Some((meta, body)) = source.parsed(&rel) else { continue };
            if body.trim().is_empty() {
                continue;
            }
            let created_at = meta.text("createdAt").unwrap_or_else(|| source.stamp(&rel));
            let updated_at = meta.text("updatedAt").unwrap_or_else(|| created_at.clone());
            posts.push((
                meta.text("id").unwrap_or(stem),
                slug.clone(),
                meta.text("parentId"),
                meta.flag("guidingQuestion"),
                meta.flag("versionPublished"),
                meta.text("documentId"),
                meta.text("documentVersion"),
                created_at,
                updated_at,
                body,
            ));
        }
    }
    // Insertion order is the tie-break for posts made in the same instant, so insert them in time order.
    posts.sort_by(|a, b| (&a.7, &a.0).cmp(&(&b.7, &b.0)));
    for (id, slug, parent, guiding, published, doc_id, doc_version, created_at, updated_at, body) in posts {
        conn.execute(
            "INSERT OR IGNORE INTO posts (id, thread_slug, parent_id, guiding_question, version_published, document_id,
                                          document_version, created_at, updated_at, body)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![id, slug, parent, guiding, published, doc_id, doc_version, created_at, updated_at, body],
        )?;
    }
    Ok(known)
}

fn import_documents(conn: &Connection, source: &Source, threads: &HashSet<String>) -> Result<()> {
    let mut ensured: HashSet<String> = db::query_all(conn, "SELECT slug FROM document_types", [], |r| r.get(0))?.into_iter().collect();
    let mut ensure_type = |slug: &str| -> Result<()> {
        if ensured.insert(slug.to_string()) {
            let now = ids::now();
            conn.execute(
                "INSERT OR IGNORE INTO document_types (slug, name, \"trigger\", created_at, updated_at) VALUES (?1, ?1, '', ?2, ?2)",
                params![slug, now],
            )?;
        }
        Ok(())
    };

    for stem in source.stems("documents") {
        let rel = document_file(&stem);

        // "<id>@v<M.m>" is a published version, anything else a live document.
        if let Some((id, version)) = stem.split_once("@v") {
            let Some((major, minor)) = documents::parse_version(version) else { continue };
            let Some((meta, body)) = source.parsed(&rel) else { continue };
            conn.execute(
                "INSERT OR IGNORE INTO document_versions (document_id, version, major, minor, document_type_slug, title, body, published_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    id,
                    version,
                    major,
                    minor,
                    meta.text("documentTypeSlug").unwrap_or_else(|| "document".into()),
                    meta.text("title").unwrap_or_else(|| id.to_string()),
                    body,
                    meta.text("publishedAt").or_else(|| meta.text("updatedAt")).unwrap_or_else(|| source.stamp(&rel)),
                ],
            )?;
            continue;
        }

        let Some((meta, body)) = source.parsed(&rel) else { continue };
        let type_slug = meta.text("documentTypeSlug").unwrap_or_else(|| "document".into());
        ensure_type(&type_slug)?;
        let created_at = meta.text("createdAt").unwrap_or_else(|| source.stamp(&rel));
        let updated_at = meta.text("updatedAt").unwrap_or_else(|| created_at.clone());
        conn.execute(
            "INSERT OR IGNORE INTO documents (id, document_type_slug, title, thread_slug, version, created_at, updated_at, body)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                meta.text("id").unwrap_or_else(|| stem.clone()),
                type_slug,
                meta.text("title").unwrap_or_else(|| stem.clone()),
                meta.text("threadSlug").filter(|t| threads.contains(t)),
                meta.text("version"),
                created_at,
                updated_at,
                body,
            ],
        )?;
    }
    Ok(())
}

fn import_stories(conn: &Connection, source: &Source, threads: &HashSet<String>) -> Result<()> {
    let columns: Vec<String> = db::query_all(conn, "SELECT id FROM planboard_columns ORDER BY position", [], |r| r.get(0))?;
    let backlog = planboard::backlog_column_id(conn)?;

    for id in source.stems("user-stories") {
        let rel = story_file(&id);
        let Some((meta, _)) = source.parsed(&rel) else { continue };
        let created_at = meta.text("createdAt").unwrap_or_else(|| source.stamp(&rel));
        let updated_at = meta.text("updatedAt").unwrap_or_else(|| created_at.clone());
        let column = meta.text("columnId").filter(|c| columns.contains(c)).unwrap_or_else(|| backlog.clone());

        let inserted = conn.execute(
            "INSERT OR IGNORE INTO user_stories (id, summary, role, want, reason, column_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id,
                meta.text("summary").unwrap_or_else(|| id.clone()),
                meta.text("role").unwrap_or_default(),
                meta.text("want").unwrap_or_default(),
                meta.text("reason").unwrap_or_default(),
                column,
                created_at,
                updated_at,
            ],
        )?;
        if inserted == 0 {
            continue; // a story with that id (ignoring case) is already there
        }

        let criteria = meta.maps("criteria").into_iter().filter_map(|c| Some((c.text("text")?, c.flag("done"))));
        for (position, (text, done)) in criteria.enumerate() {
            conn.execute(
                "INSERT INTO acceptance_criteria (story_id, position, text, done) VALUES (?1, ?2, ?3, ?4)",
                params![id, position as i64, text, done],
            )?;
        }
        for slug in meta.list("threadSlugs").into_iter().filter(|s| threads.contains(s)) {
            conn.execute("INSERT OR IGNORE INTO story_threads (story_id, thread_slug) VALUES (?1, ?2)", [id.as_str(), slug.as_str()])?;
        }
    }
    Ok(())
}
