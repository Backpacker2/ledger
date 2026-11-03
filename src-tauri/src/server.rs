//! `ledger-server`: Ledger without its window, for the mobile version.
//!
//! It opens a vault exactly as the desktop app does (in-memory database, write-through to the files, the GitHub
//! sync worker) and answers the same commands over HTTP, so the Svelte screens can run in a phone's browser:
//!
//! - `POST /api/<command>` with the command's arguments as a JSON object (camelCase, as `invoke` sends them);
//!   answers `200` with the result as JSON, `400` with the error message as a JSON string, or `404` for a
//!   command the server doesn't offer (the desktop-only ones, see [`DESKTOP_ONLY`]).
//! - `GET /api/file?project=…&path=…`: a file in a project's folder (attachments, images, deliverables).
//! - `POST /api/upload`: multipart form with `project`, `thread`, `description` and one or more `file` parts,
//!   attached to the thread like `upload_files` does on the desktop.
//! - `GET /api/events`: server-sent events for what the desktop app emits (`sync-status`, `vault-changed`,
//!   `log-entry`).
//! - `GET /api/health`: `ok`.
//! - anything else: the built screens from `LEDGER_STATIC`, if set (a single-page app, so unknown paths get
//!   `index.html`).
//!
//! It listens on `127.0.0.1:8787` by default, meant to be put on the tailnet with `tailscale serve`; it has no
//! login of its own. With `LEDGER_ALLOWED_LOGINS` (comma-separated Tailscale logins) it also refuses requests
//! whose `Tailscale-User-Login` header (set by `tailscale serve`) isn't one of them.
//!
//! Environment: `LEDGER_VAULT` (required: the vault folder), `LEDGER_CONFIG_DIR` (per-machine settings and the
//! log; default `~/Library/Application Support/ledger-server`), `LEDGER_SERVER_ADDR`, `LEDGER_STATIC`,
//! `LEDGER_ALLOWED_LOGINS`, and `LEDGER_GITHUB_API` as for the desktop app.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{DefaultBodyLimit, Multipart, Path as UrlPath, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::broadcast;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

use crate::commands;
use crate::error::{Error, Result};
use crate::models::*;
use crate::screenshot::Screenshots;
use crate::state::{App, AppRef, Core};
use crate::syncer::{self, Event};
use crate::{applog, uploads};

/// Largest request body, for uploads from the phone (photos, recordings).
const MAX_UPLOAD_BYTES: usize = 200 * 1024 * 1024;

/// Commands of the desktop app the server leaves out: they need its window, the machine's screen or programs,
/// or a worker the server doesn't run yet. A test checks that every command in `lib.rs` is either served or here.
pub const DESKTOP_ONLY: &[&str] = &[
    // The system's file manager and programs, and the update installer.
    "open_project_file",
    "read_project_file", // binary IPC; `GET /api/file` instead
    "upload_files",      // paths on this machine; `POST /api/upload` instead
    "about_info",
    "update_status",
    "save_update_settings",
    "check_for_updates",
    "install_update",
    "simulate_update",
    // The vault location is fixed by LEDGER_VAULT on the server.
    "set_vault_path",
    // Screenshots: the hotkey, the capture and its overlay window.
    "save_screenshot_hotkey",
    "save_screenshot_tool",
    "pending_screenshot",
    "take_screenshot",
    "pause_screenshot_hotkey",
    "screenshot_image",
    "screenshot_overlay_ready",
    "screenshot_finish",
    "screenshot_cancel",
    "pending_screenshot_image",
    "attach_screenshot",
    "discard_screenshot",
    "log_set_max_bytes",
    // Not yet on the server (a later phase): Claude Code runs and the Trello sync.
    "claude_code_settings",
    "save_claude_code_settings",
    "check_claude_code",
    "claude_code_preview",
    "claude_code_start",
    "claude_code_cancel",
    "claude_code_runs",
    "claude_code_run",
    "trello_get",
    "trello_status",
    "trello_test",
    "trello_save_credentials",
    "trello_disconnect",
    "trello_boards",
    "trello_board_members",
    "trello_save_member",
    "trello_link_project",
    "trello_project_mapping",
    "trello_set_column_list",
    "trello_sync_now",
    "trello_relink_item",
];

/// What a command answered: JSON for the screen, or the message of what went wrong.
type Reply = std::result::Result<Value, String>;

fn json<T: serde::Serialize>(value: T) -> Reply {
    serde_json::to_value(value).map_err(|e| format!("Couldn't encode the answer: {e}"))
}

fn args<T: DeserializeOwned>(command: &str, value: Value) -> std::result::Result<T, String> {
    serde_json::from_value(value).map_err(|e| format!("The arguments for {command} don't fit: {e}"))
}

/// Generates [`dispatch`] and `LISTED` from the list of served commands and their arguments. Argument names are
/// the commands' own; the frontend sends them in camelCase, as Tauri expects. `Option` arguments may be left out.
macro_rules! served {
    (
        fallible { $( $name:ident ( $( $arg:ident : $ty:ty ),* ) ),* $(,)? }
        plain { $( $pname:ident ( $( $parg:ident : $pty:ty ),* ) ),* $(,)? }
    ) => {
        /// Runs a served command; `None` when the server doesn't offer it.
        fn dispatch(app: &App, command: &str, input: Value) -> Option<Reply> {
            match command {
                $(
                    stringify!($name) => Some((|| {
                        #[derive(Deserialize)]
                        #[serde(rename_all = "camelCase")]
                        struct Args { $( $arg: $ty ),* }
                        let Args { $( $arg ),* } = args(command, input)?;
                        json(commands::$name(AppRef(app) $(, $arg)*).map_err(|e| e.to_string())?)
                    })()),
                )*
                $(
                    stringify!($pname) => Some((|| {
                        #[derive(Deserialize)]
                        #[serde(rename_all = "camelCase")]
                        struct Args { $( $parg: $pty ),* }
                        let Args { $( $parg ),* } = args(command, input)?;
                        json(commands::$pname(AppRef(app) $(, $parg)*))
                    })()),
                )*
                _ => extra(app, command, input),
            }
        }

        /// The commands the list below answers ([`EXTRA`] holds the rest).
        const LISTED: &[&str] = &[ $( stringify!($name), )* $( stringify!($pname), )* ];
    };
}

served! {
    fallible {
        search(query: String),
        list_activity(),
        list_projects(),
        create_project(name: String),
        rename_project(slug: String, name: String),
        get_project(slug: String),
        save_readme(slug: String, text: String),
        delete_project(slug: String),
        create_thread(project: String, title: String),
        rename_thread(project: String, slug: String, title: String),
        get_thread(project: String, slug: String),
        delete_thread(project: String, slug: String),
        link_threads(project: String, slug: String, other_slug: String),
        unlink_threads(project: String, slug: String, other_slug: String),
        add_post(project: String, thread: String, body: String, reply_to_id: Option<String>),
        update_post(project: String, thread: String, id: String, body: String),
        delete_post(project: String, thread: String, id: String),
        create_prompt(project: String, thread: String, title: String, body: String, template_id: Option<String>, deliverable: Option<String>),
        update_prompt(project: String, thread: String, id: String, title: Option<String>, body: Option<String>),
        delete_prompt(project: String, thread: String, id: String),
        link_prompt_deliverable(project: String, thread: String, id: String, path: String),
        unlink_prompt_deliverable(project: String, thread: String, id: String, path: String),
        create_prompt_deliverable(project: String, thread: String, id: String, name: String),
        add_feedback(project: String, thread: String, deliverable_path: String, body: String),
        set_feedback_status(project: String, thread: String, id: String, status: String),
        update_feedback(project: String, thread: String, id: String, body: String),
        delete_feedback(project: String, thread: String, id: String),
        update_upload(project: String, thread: String, id: String, description: String),
        delete_upload(project: String, thread: String, id: String),
        list_deliverables(project: String),
        get_deliverable(project: String, path: String),
        create_deliverable(project: String, name: String, body: String),
        save_deliverable(project: String, path: String, body: String),
        publish_deliverable(project: String, path: String, bump: Option<Bump>, version: Option<String>, choices: Option<PublishChoices>),
        get_provenance(project: String, path: String),
        update_version_record(project: String, path: String, version: String, update: VersionUpdate),
        delete_deliverable(project: String, path: String),
        list_templates(scope: String),
        project_templates(project: String),
        effective_templates(project: String),
        create_template(scope: String, name: String, trigger: String, questions: Vec<String>, body: String),
        override_template(project: String, id: String),
        update_template(scope: String, id: String, name: Option<String>, trigger: Option<String>, questions: Option<Vec<String>>, body: Option<String>),
        delete_template(scope: String, id: String),
        list_stories(project: String),
        create_story(project: String, draft: StoryDraft),
        update_story(project: String, id: String, updates: StoryUpdate),
        delete_story(project: String, id: String),
        link_story_thread(project: String, id: String, thread_slug: String),
        unlink_story_thread(project: String, id: String, thread_slug: String),
        get_board(project: String),
        create_task(project: String, title: String, body: String, story_id: Option<String>, thread_slug: Option<String>),
        update_task(project: String, id: String, updates: TaskUpdate),
        delete_task(project: String, id: String),
        add_column(project: String, name: String),
        rename_column(project: String, id: String, name: String),
        delete_column(project: String, id: String),
        save_sync_settings(repo: String, branch: String, token: Option<String>),
        disconnect_sync(),
        sync_now(),
        set_encryption_enabled(enabled: bool),
        generate_encryption_key(replace: bool),
        import_encryption_key(key: String, replace: bool),
        export_recovery_key(),
        reload_vault(),
        check_vault_changes(),
    }
    plain {
        get_settings(),
        sync_status(),
        encryption_status(),
    }
}

/// The served commands that don't take the `App` the same way: no state at all, or async.
fn extra(app: &App, command: &str, input: Value) -> Option<Reply> {
    #[derive(Deserialize)]
    struct Markdown {
        body: String,
    }
    #[derive(Deserialize)]
    struct TestSync {
        repo: String,
        branch: String,
        token: Option<String>,
    }
    #[derive(Deserialize)]
    struct LogWrite {
        level: String,
        source: String,
        message: String,
    }
    #[derive(Deserialize)]
    struct LogRead {
        limit: Option<usize>,
    }
    #[derive(Deserialize)]
    struct Text {
        text: String,
    }
    #[derive(Deserialize)]
    struct Debug {
        source: String,
        message: String,
    }
    let fail = |e: Error| e.to_string();
    if !EXTRA.contains(&command) {
        return None;
    }
    Some((|| match command {
        "render_markdown" => json(commands::render_markdown(args::<Markdown>(command, input)?.body)),
        "test_sync" => {
            let a: TestSync = args(command, input)?;
            let future = commands::test_sync(AppRef(app), a.repo, a.branch, a.token);
            // Called from a blocking thread of the runtime, where waiting on a future is allowed.
            json(tokio::runtime::Handle::current().block_on(future).map_err(fail)?)
        }
        "log_write" => {
            let a: LogWrite = args(command, input)?;
            applog::log_write(a.level, a.source, a.message);
            json(())
        }
        "log_read" => json(applog::log_read(args::<LogRead>(command, input)?.limit).map_err(fail)?),
        "log_clear" => json(applog::log_clear().map_err(fail)?),
        "capture_status" => json(applog::capture_status().map_err(fail)?),
        "capture_start" => json(applog::capture_start().map_err(fail)?),
        "capture_debug" => {
            let a: Debug = args(command, input)?;
            applog::capture_debug(a.source, a.message);
            json(())
        }
        "capture_stop" => json(applog::capture_stop().map_err(fail)?),
        "capture_save" => json(applog::capture_save(args::<Text>(command, input)?.text).map_err(fail)?),
        _ => unreachable!("only the commands in EXTRA get here"),
    })())
}

/// Every command the server answers.
pub fn served() -> impl Iterator<Item = &'static str> {
    LISTED.iter().chain(EXTRA).copied()
}

/// The commands [`extra`] answers.
const EXTRA: &[&str] = &[
    "render_markdown",
    "test_sync",
    "log_write",
    "log_read",
    "log_clear",
    "capture_status",
    "capture_start",
    "capture_debug",
    "capture_stop",
    "capture_save",
];

// ---- the HTTP side -----------------------------------------------------------------------------------

#[derive(Clone)]
struct Shared {
    app: Arc<App>,
    events: broadcast::Sender<(&'static str, Value)>,
    allowed_logins: Option<Arc<Vec<String>>>,
}

fn error(status: StatusCode, message: impl Into<String>) -> Response {
    (status, axum::Json(Value::String(message.into()))).into_response()
}

async fn command(State(shared): State<Shared>, UrlPath(name): UrlPath<String>, body: axum::body::Bytes) -> Response {
    let input = if body.is_empty() {
        Value::Object(Default::default())
    } else {
        match serde_json::from_slice::<Value>(&body) {
            Ok(Value::Null) => Value::Object(Default::default()),
            Ok(value) => value,
            Err(e) => {
                return error(StatusCode::BAD_REQUEST, format!("The request isn't JSON: {e}"));
            }
        }
    };
    let app = shared.app.clone();
    let name_for_task = name.clone();
    // Commands lock the database and some talk to GitHub with a blocking client: off the async threads.
    let reply = tokio::task::spawn_blocking(move || dispatch(&app, &name_for_task, input)).await;
    match reply {
        Ok(Some(Ok(value))) => axum::Json(value).into_response(),
        Ok(Some(Err(message))) => error(StatusCode::BAD_REQUEST, message),
        Ok(None) if DESKTOP_ONLY.contains(&name.as_str()) => error(
            StatusCode::NOT_FOUND,
            format!("{name} is only available in the desktop app"),
        ),
        Ok(None) => error(StatusCode::NOT_FOUND, format!("There is no command {name}")),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("{name} stopped unexpectedly: {e}"),
        ),
    }
}

#[derive(Deserialize)]
struct FileQuery {
    project: String,
    path: String,
}

async fn file(State(shared): State<Shared>, Query(q): Query<FileQuery>) -> Response {
    let path = match shared
        .app
        .read(|_, root| commands::project_path(root, &q.project, &q.path))
    {
        Ok(path) => path,
        Err(e) => return error(StatusCode::BAD_REQUEST, e.to_string()),
    };
    match tokio::fs::read(&path).await {
        Ok(bytes) => {
            let mut headers = HeaderMap::new();
            headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type(&path)));
            headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
            (headers, bytes).into_response()
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            error(StatusCode::NOT_FOUND, "That file isn't there any more")
        }
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, Error::io(&path, e).to_string()),
    }
}

/// The content type for a file, by its extension; the rest is plain bytes.
fn content_type(path: &std::path::Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "heic" => "image/heic",
        "pdf" => "application/pdf",
        "md" | "markdown" => "text/markdown; charset=utf-8",
        "txt" | "csv" | "log" => "text/plain; charset=utf-8",
        "json" => "application/json",
        "html" | "htm" => "text/plain; charset=utf-8", // never rendered as a page from the vault
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        _ => "application/octet-stream",
    }
}

async fn upload(State(shared): State<Shared>, mut form: Multipart) -> Response {
    let (mut project, mut thread, mut description) = (String::new(), String::new(), String::new());
    let mut files: Vec<(String, Vec<u8>)> = vec![];
    loop {
        let field = match form.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(e) => {
                return error(StatusCode::BAD_REQUEST, format!("The upload didn't come through: {e}"));
            }
        };
        let name = field.name().unwrap_or_default().to_string();
        let file_name = field.file_name().map(str::to_string);
        let data = match field.bytes().await {
            Ok(data) => data,
            Err(e) => {
                return error(StatusCode::BAD_REQUEST, format!("The upload didn't come through: {e}"));
            }
        };
        let text = || String::from_utf8_lossy(&data).into_owned();
        match name.as_str() {
            "project" => project = text(),
            "thread" => thread = text(),
            "description" => description = text(),
            "file" => files.push((safe_file_name(file_name.as_deref()), data.to_vec())),
            _ => {}
        }
    }
    if files.is_empty() {
        return error(StatusCode::BAD_REQUEST, "Choose a file to upload");
    }
    let app = shared.app.clone();
    let added = tokio::task::spawn_blocking(move || -> Result<Vec<UploadView>> {
        app.mutate(|c, root| {
            files
                .iter()
                .map(|(name, bytes)| uploads::add_bytes(c, root, &project, &thread, name, bytes, &description))
                .collect()
        })
    })
    .await;
    match added {
        Ok(Ok(added)) => axum::Json(added).into_response(),
        Ok(Err(e)) => error(StatusCode::BAD_REQUEST, e.to_string()),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("The upload stopped unexpectedly: {e}"),
        ),
    }
}

/// The last part of an uploaded file's name, without anything that could point elsewhere.
fn safe_file_name(name: Option<&str>) -> String {
    let base = name
        .unwrap_or_default()
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .trim();
    let cleaned: String = base.chars().filter(|c| !c.is_control()).collect();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "file".into()
    } else {
        cleaned
    }
}

async fn events(
    State(shared): State<Shared>,
) -> Sse<impl tokio_stream::Stream<Item = std::result::Result<SseEvent, Infallible>>> {
    let stream = BroadcastStream::new(shared.events.subscribe()).filter_map(|item| {
        let (name, payload) = item.ok()?;
        Some(Ok(SseEvent::default().event(name).data(payload.to_string())))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// With `LEDGER_ALLOWED_LOGINS` set, only those Tailscale users get in.
async fn tailnet_login(State(shared): State<Shared>, request: Request, next: Next) -> Response {
    if let Some(allowed) = &shared.allowed_logins {
        let login = request
            .headers()
            .get("tailscale-user-login")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        if !allowed.iter().any(|a| a.eq_ignore_ascii_case(login)) {
            return error(StatusCode::FORBIDDEN, "This Ledger isn't shared with you");
        }
    }
    next.run(request).await
}

fn router(shared: Shared, static_dir: Option<PathBuf>) -> Router {
    let api = Router::new()
        .route("/api/health", get(|| async { "ok" }))
        .route("/api/events", get(events))
        .route("/api/file", get(file))
        .route("/api/upload", post(upload))
        .route("/api/{command}", post(command))
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES));
    let api = match static_dir {
        Some(dir) => {
            let index = dir.join("index.html");
            api.fallback_service(
                tower_http::services::ServeDir::new(dir).fallback(tower_http::services::ServeFile::new(index)),
            )
        }
        None => api.fallback(|| async { error(StatusCode::NOT_FOUND, "Nothing here") }),
    };
    api.layer(middleware::from_fn_with_state(shared.clone(), tailnet_login))
        .with_state(shared)
}

/// The Tailscale logins let in, from `LEDGER_ALLOWED_LOGINS`. Empty (or only commas and spaces) means no list:
/// everyone who can reach the server on the tailnet, not no one.
fn allowed_logins(list: &str) -> Option<Vec<String>> {
    let logins: Vec<String> = list.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    (!logins.is_empty()).then_some(logins)
}

fn default_config_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/ledger-server")
    } else {
        home.join(".config/ledger-server")
    }
}

/// Starts the server and runs until it is stopped (Ctrl-C or SIGTERM, as launchd sends).
pub fn main() {
    if let Err(message) = run() {
        eprintln!("ledger-server: {message}");
        std::process::exit(1);
    }
}

fn run() -> std::result::Result<(), String> {
    if std::env::var_os("LEDGER_VAULT").filter(|v| !v.is_empty()).is_none() {
        return Err("Set LEDGER_VAULT to the vault's folder".into());
    }
    let config_dir = std::env::var_os("LEDGER_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(default_config_dir);
    std::fs::create_dir_all(&config_dir).map_err(|e| format!("Can't create {}: {e}", config_dir.display()))?;
    let addr: SocketAddr = std::env::var("LEDGER_SERVER_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8787".into())
        .parse()
        .map_err(|e| format!("LEDGER_SERVER_ADDR isn't an address like 127.0.0.1:8787: {e}"))?;
    let static_dir = std::env::var_os("LEDGER_STATIC").map(PathBuf::from);
    if let Some(dir) = &static_dir
        && !dir.join("index.html").is_file()
    {
        return Err(format!("LEDGER_STATIC has no index.html: {}", dir.display()));
    }
    let allowed_logins = allowed_logins(&std::env::var("LEDGER_ALLOWED_LOGINS").unwrap_or_default()).map(Arc::new);

    let (events, _) = broadcast::channel::<(&'static str, Value)>(256);
    let log_events = events.clone();
    applog::init(
        &config_dir.join("logs"),
        crate::settings::load_config(&config_dir)
            .log_max_bytes
            .unwrap_or(applog::DEFAULT_MAX_BYTES),
        Box::new(move |entry| {
            if let Ok(value) = serde_json::to_value(entry) {
                let _ = log_events.send(("log-entry", value));
            }
        }),
    );
    applog::info(
        "server",
        format!(
            "ledger-server {} started on {} {}",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH
        ),
    );

    let core = Core::open(config_dir, None).map_err(|e| format!("Can't open the vault: {e}"))?;
    let sync = core.sync.clone();
    let core = Arc::new(Mutex::new(core));
    let sync_events = events.clone();
    let syncer = syncer::spawn(
        core.clone(),
        Arc::new(move |event| {
            let _ = match event {
                Event::Status(status) => {
                    sync_events.send(("sync-status", serde_json::to_value(status).unwrap_or_default()))
                }
                Event::VaultChanged => sync_events.send(("vault-changed", Value::Null)),
            };
        }),
    );
    syncer.reconfigure(sync.as_ref());
    let app = Arc::new(App::new(core, syncer, Screenshots::new()));

    let shared = Shared {
        app,
        events,
        allowed_logins,
    };
    let runtime = tokio::runtime::Runtime::new().map_err(|e| format!("Can't start: {e}"))?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| format!("Can't listen on {addr}: {e}"))?;
        applog::info("server", format!("Listening on http://{addr}"));
        println!("ledger-server listening on http://{addr}");
        axum::serve(listener, router(shared, static_dir))
            .with_graceful_shutdown(shutdown())
            .await
            .map_err(|e| format!("The server stopped: {e}"))
    })?;
    applog::info("server", "Stopped");
    Ok(())
}

async fn shutdown() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut signal) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = term => {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every command the desktop app registers is either served or listed as desktop-only, so a command added
    /// upstream can't go missing on the phone unnoticed.
    #[test]
    fn every_command_is_served_or_desktop_only() {
        let lib = include_str!("lib.rs");
        let start = lib.find("generate_handler![").expect("lib.rs registers its commands");
        let end = start + lib[start..].find("])").expect("the list ends");
        let registered: Vec<&str> = lib[start + "generate_handler![".len()..end]
            .lines()
            .map(|line| line.split("//").next().unwrap_or_default().trim().trim_end_matches(','))
            .filter(|line| !line.is_empty())
            .map(|path| path.rsplit("::").next().unwrap_or(path))
            .collect();
        assert!(registered.len() > 50, "found only {} commands", registered.len());
        let served: Vec<&str> = served().collect();
        let missing: Vec<_> = registered
            .iter()
            .filter(|c| !served.contains(c) && !DESKTOP_ONLY.contains(c))
            .collect();
        assert!(missing.is_empty(), "neither served nor desktop-only: {missing:?}");
        let both: Vec<_> = served.iter().filter(|c| DESKTOP_ONLY.contains(c)).collect();
        assert!(both.is_empty(), "both served and desktop-only: {both:?}");
        let unknown: Vec<_> = served
            .iter()
            .chain(DESKTOP_ONLY)
            .filter(|c| !registered.contains(c))
            .collect();
        assert!(unknown.is_empty(), "not registered by the desktop app: {unknown:?}");
    }

    #[test]
    fn an_empty_login_list_lets_the_tailnet_in() {
        assert_eq!(allowed_logins(""), None);
        assert_eq!(allowed_logins(" , "), None);
        assert_eq!(allowed_logins("me@example.com, you@example.com"), Some(vec!["me@example.com".into(), "you@example.com".into()]));
    }

    #[test]
    fn uploaded_names_stay_in_the_folder() {
        assert_eq!(safe_file_name(Some("../../etc/passwd")), "passwd");
        assert_eq!(safe_file_name(Some("C:\\photos\\IMG_1.HEIC")), "IMG_1.HEIC");
        assert_eq!(safe_file_name(Some("..")), "file");
        assert_eq!(safe_file_name(None), "file");
    }
}
