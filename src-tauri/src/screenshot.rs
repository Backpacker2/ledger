//! Screenshots. A global hotkey (or the button in Settings) takes a screenshot, and the PNG is kept here, in
//! memory, while the main window comes to the front and asks which thread it belongs to. Attaching it saves the
//! file in that project's attachments and records it in the thread like any other upload (see `uploads`).
//!
//! Three ways a picture gets here:
//!
//! - **The user's own tool** (`command`): a command from Settings, such as `flameshot gui --raw`. Ledger runs it
//!   and takes the PNG it prints, or the file it wrote to `{file}`. Same on every platform; the way to go.
//! - **Opened with Ledger** (`open_files`): Flameshot's "Open with", or a file manager, launches Ledger with a
//!   picture. A second launch is folded into the running app by the single-instance plugin.
//! - **The built-in capture** (`builtin`, below), for whoever hasn't set up a tool. See the note there: it is meant
//!   to be removable.

use std::io::Cursor;
use std::path::Path;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use image::{ImageFormat, ImageReader, RgbaImage};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Shortcut, ShortcutState};

use crate::error::{Error, Result};
use crate::settings::{self, ScreenshotTool};
use crate::state::App;

pub const DEFAULT_HOTKEY: &str = "CommandOrControl+Shift+S";
/// The overlay window of the built-in capture; never opened on Linux.
const OVERLAY_LABEL: &str = "screenshot-overlay";

/// Combinations the system (or every program) already means something by; a global shortcut on one of these
/// would break it everywhere.
const RESERVED: [&str; 16] = [
    "Alt+F4", "Alt+Tab", "Control+Alt+Delete", "Control+C", "Control+V", "Control+X", "Control+Z", "Command+C",
    "Command+V", "Command+X", "Command+Z", "Command+Q", "Command+Tab", "Command+Space", "Super+L", "Super+D",
];

/// What the overlay sends back: the dragged rectangle, in capture pixels.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Region {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// The payload of the `screenshot-ready` event: a screenshot is waiting to be attached to a thread.
#[derive(Debug, Serialize)]
pub struct Taken {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenshotSettings {
    pub hotkey: String,
    pub default_hotkey: String,
    /// Why the saved hotkey isn't working, if it isn't.
    pub problem: Option<String>,
    /// Something about this platform worth knowing before the first screenshot.
    pub note: Option<String>,
    /// Whether the user's own tool takes the screenshots, and its command (kept while the built-in one is used).
    pub use_command: bool,
    pub command: String,
    /// Ready-made commands for this platform, with whether the program was found on this machine.
    pub presets: Vec<Preset>,
    /// What the built-in capture does here, for the choice in Settings.
    pub builtin: &'static str,
    /// `linux`, `macos` or `windows`: which instructions Settings shows.
    pub platform: &'static str,
}

/// A command for a well-known screenshot tool.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preset {
    pub name: &'static str,
    pub command: String,
    /// The program is installed where the command expects it.
    pub found: bool,
}

pub struct Screenshots {
    /// The hotkey currently registered with the system.
    registered: Mutex<Option<Shortcut>>,
    problem: Mutex<Option<String>>,
    /// A capture is being made or the overlay is up; further hotkey presses are ignored until it closes.
    busy: AtomicBool,
    /// The capture the overlay is showing.
    capture: Mutex<Option<RgbaImage>>,
    /// The cropped screenshot (as PNG) waiting to be attached to a thread.
    pending: Mutex<Option<Vec<u8>>>,
    #[cfg(target_os = "linux")]
    portal_task: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}

impl Screenshots {
    pub fn new() -> Screenshots {
        Screenshots {
            registered: Mutex::new(None),
            problem: Mutex::new(None),
            busy: AtomicBool::new(false),
            capture: Mutex::new(None),
            pending: Mutex::new(None),
            #[cfg(target_os = "linux")]
            portal_task: Mutex::new(None),
        }
    }

    /// The hotkey saved in the vault's settings (or the default) and whether it is working.
    pub fn settings(&self, root: &Path) -> ScreenshotSettings {
        let tool = settings::read_settings(root).screenshot_tool;
        ScreenshotSettings {
            hotkey: saved_hotkey(root),
            default_hotkey: DEFAULT_HOTKEY.to_string(),
            problem: lock(&self.problem).clone(),
            note: platform_note(tool.use_command),
            use_command: tool.use_command,
            command: tool.command,
            presets: presets(),
            builtin: builtin::DESCRIPTION,
            platform: PLATFORM,
        }
    }
}

impl Default for Screenshots {
    fn default() -> Self {
        Screenshots::new()
    }
}

fn saved_hotkey(root: &Path) -> String {
    settings::read_settings(root).screenshot_hotkey.unwrap_or_else(|| DEFAULT_HOTKEY.to_string())
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn state(app: &AppHandle) -> tauri::State<'_, App> {
    app.state::<App>()
}

// ---- the hotkey -------------------------------------------------------------------------------------

/// A shortcut the system will accept and that is safe to take over globally.
pub fn parse_hotkey(text: &str) -> Result<Shortcut> {
    let shortcut = Shortcut::from_str(text.trim()).map_err(|_| Error::invalid("That isn't a shortcut Ledger understands."))?;

    let function_key = [Code::PrintScreen, Code::F1, Code::F2, Code::F3, Code::F4, Code::F5, Code::F6, Code::F7, Code::F8, Code::F9, Code::F10, Code::F11, Code::F12]
        .contains(&shortcut.key);
    if shortcut.mods.is_empty() && !function_key {
        return Err(Error::invalid("Include a modifier key (Ctrl, Alt, Shift or ⌘): a global shortcut without one would take that key away from every other program."));
    }
    if RESERVED.iter().filter_map(|r| Shortcut::from_str(r).ok()).any(|r| r.id() == shortcut.id()) {
        return Err(Error::invalid("The system or most programs already use that combination. Choose another."));
    }
    Ok(shortcut)
}

/// Makes `text` the screenshot hotkey, replacing the previous one without a gap. If the new one can't be
/// registered the old one stays in place and the reason comes back as the error.
pub fn apply_hotkey(app: &AppHandle, text: &str) -> Result<()> {
    let shots = &state(app).screenshots;
    #[cfg(target_os = "linux")]
    if portal::is_wayland() {
        return portal::bind(app, text).inspect(|()| *lock(&shots.problem) = None);
    }

    let shortcut = parse_hotkey(text)?;
    let mut registered = lock(&shots.registered);
    if *registered != Some(shortcut) {
        register(app, shortcut, text)?;
        if let Some(old) = registered.replace(shortcut) {
            let _ = app.global_shortcut().unregister(old);
        }
    }
    *lock(&shots.problem) = None;
    Ok(())
}

fn register(app: &AppHandle, shortcut: Shortcut, label: &str) -> Result<()> {
    app.global_shortcut()
        .on_shortcut(shortcut, |app, _, event| {
            if event.state == ShortcutState::Pressed {
                trigger(app);
            }
        })
        .map_err(|e| Error::invalid(format!("{label} couldn't be registered, probably because another program already uses it. Choose a different shortcut. ({e})")))
}

/// At startup, and whenever the vault changes: registers the vault's saved hotkey (or the default), remembering
/// why if that fails.
pub fn register_saved(app: &AppHandle) {
    let app_state = state(app);
    let hotkey = saved_hotkey(app_state.lock().vault.root());
    if let Err(e) = apply_hotkey(app, &hotkey) {
        *lock(&app_state.screenshots.problem) = Some(e.to_string());
    }
}

/// While the settings screen records a new shortcut, the current one must not fire: pressing it would take
/// a screenshot instead of being recorded.
pub fn pause_hotkey(app: &AppHandle, paused: bool) {
    let registered = *lock(&state(app).screenshots.registered);
    if let Some(shortcut) = registered {
        if paused {
            let _ = app.global_shortcut().unregister(shortcut);
        } else {
            let _ = register(app, shortcut, "The shortcut");
        }
    }
}

// ---- taking one -------------------------------------------------------------------------------------

/// Starts a screenshot; does nothing if one is already under way.
pub fn trigger(app: &AppHandle) {
    if state(app).screenshots.busy.swap(true, Ordering::AcqRel) {
        return;
    }
    let app = app.clone();
    // The tool, window creation and screen capture must not run on the thread the hotkey or a command arrived on.
    std::thread::spawn(move || {
        if let Err(e) = begin(&app) {
            release(&app);
            report(&app, &e);
        }
    });
}

fn begin(app: &AppHandle) -> Result<()> {
    let tool = saved_tool(app);
    if tool.use_command && !tool.command.trim().is_empty() {
        let outcome = command::run(&tool.command);
        release(app);
        return match outcome? {
            command::Outcome::Picture(png) => hand_over(app, png)
                .map_err(|_| Error::screenshot("The screenshot tool didn't hand back a PNG picture. Check its command under Settings → Screenshots.")),
            command::Outcome::Cancelled => Ok(()),
        };
    }
    builtin::begin(app)
}

fn saved_tool(app: &AppHandle) -> ScreenshotTool {
    settings::read_settings(state(app).lock().vault.root()).screenshot_tool
}

fn release(app: &AppHandle) {
    let shots = &state(app).screenshots;
    *lock(&shots.capture) = None;
    shots.busy.store(false, Ordering::Release);
}

/// Saves the choice between the built-in capture and the user's own tool. A command that can't be split into a
/// program and its arguments is refused.
pub fn save_tool(root: &Path, use_command: bool, command: &str) -> Result<()> {
    let command = command.trim().to_string();
    if use_command {
        command::split(&command)?;
    }
    settings::update_settings(root, |s| s.screenshot_tool = ScreenshotTool { use_command, command })
}

#[cfg(target_os = "linux")]
const PLATFORM: &str = "linux";
#[cfg(target_os = "macos")]
const PLATFORM: &str = "macos";
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
const PLATFORM: &str = "windows";

/// Commands for the tools worth suggesting on this platform.
fn presets() -> Vec<Preset> {
    #[cfg(target_os = "linux")]
    {
        // Under Linux Flameshot's command line hands the capture to the running Flameshot and waits; --raw has
        // the PNG printed back.
        vec![Preset { name: "Flameshot", command: "flameshot gui --raw".into(), found: on_path("flameshot") }]
    }
    #[cfg(target_os = "macos")]
    {
        const FLAMESHOT: &str = "/Applications/flameshot.app/Contents/MacOS/flameshot";
        vec![
            Preset { name: "Flameshot", command: format!("{FLAMESHOT} gui -p {{file}}"), found: Path::new(FLAMESHOT).exists() },
            // Built into macOS: -i is the interactive area picker (Space switches to a window); Esc writes nothing.
            Preset { name: "macOS Screenshot", command: "screencapture -i {file}".into(), found: true },
        ]
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        const FLAMESHOT: &str = r"C:\Program Files\Flameshot\bin\flameshot.exe";
        vec![Preset { name: "Flameshot", command: format!("\"{FLAMESHOT}\" gui -p {{file}}"), found: Path::new(FLAMESHOT).exists() }]
    }
}

#[cfg(target_os = "linux")]
fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

fn platform_note(use_command: bool) -> Option<String> {
    #[cfg(target_os = "macos")]
    if !use_command && !core_graphics::access::ScreenCaptureAccess.preflight() {
        return Some(builtin::MAC_PERMISSION.to_string());
    }
    #[cfg(target_os = "linux")]
    if portal::is_wayland() {
        return Some("Under Wayland the shortcut goes through the desktop's global-shortcut portal. If your desktop doesn't offer that portal, use the button below, or give the tool its own key in your desktop's keyboard settings.".into());
    }
    let _ = use_command;
    None
}

// ---- the user's own tool ----------------------------------------------------------------------------

/// Running a screenshot program of the user's choosing. The command is split into a program and its arguments
/// here, not by a shell, so it means the same on every platform: spaces separate, double or single quotes keep
/// a part together, and a backslash is just a backslash (Windows paths stay as they are).
///
/// `{file}` in the command stands for a fresh temporary `.png` path: the tool writes there and Ledger reads it
/// once the tool has exited. Without `{file}`, Ledger reads the PNG from what the tool prints. A tool that exits
/// without a picture was cancelled; that is not an error.
pub(crate) mod command {
    use std::path::PathBuf;
    use std::process::{Command, Stdio};

    use crate::error::{Error, Result};

    pub const FILE: &str = "{file}";

    #[derive(Debug, PartialEq, Eq)]
    pub enum Outcome {
        Picture(Vec<u8>),
        Cancelled,
    }

    pub fn split(text: &str) -> Result<Vec<String>> {
        let (mut parts, mut part, mut in_part, mut quote) = (Vec::new(), String::new(), false, None);
        for c in text.chars() {
            match quote {
                Some(q) if c == q => quote = None,
                Some(_) => part.push(c),
                None if c == '"' || c == '\'' => (quote, in_part) = (Some(c), true),
                None if c.is_whitespace() => {
                    if in_part {
                        parts.push(std::mem::take(&mut part));
                        in_part = false;
                    }
                }
                None => (part.push(c), in_part = true).1,
            }
        }
        if quote.is_some() {
            return Err(Error::invalid("A quote in the screenshot command isn't closed."));
        }
        if in_part {
            parts.push(part);
        }
        if parts.is_empty() {
            return Err(Error::invalid("Enter the command that takes the screenshot."));
        }
        Ok(parts)
    }

    /// The command's parts with `{file}` filled in, and whether it was there (so the picture is in that file).
    pub fn prepare(text: &str, file: &str) -> Result<(Vec<String>, bool)> {
        let parts = split(text)?;
        let uses_file = parts.iter().any(|p| p.contains(FILE));
        Ok((parts.into_iter().map(|p| p.replace(FILE, file)).collect(), uses_file))
    }

    pub fn run(text: &str) -> Result<Outcome> {
        let file = temp_file();
        let (parts, uses_file) = prepare(text, &file.to_string_lossy())?;
        let mut command = Command::new(&parts[0]);
        command.args(&parts[1..]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        {
            // A console program would otherwise flash a black window.
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let output = command.output().map_err(|e| {
            Error::screenshot(format!("Ledger couldn't start {} ({e}). Check the command under Settings → Screenshots.", parts[0]))
        })?;
        let picture = if uses_file {
            let bytes = std::fs::read(&file).unwrap_or_default();
            let _ = std::fs::remove_file(&file);
            bytes
        } else {
            output.stdout
        };
        interpret(output.status.success(), picture, &String::from_utf8_lossy(&output.stderr))
    }

    /// What the tool's exit means. No picture is a cancel when the tool exited cleanly, said nothing, or said it was
    /// aborted (Flameshot prints "Screenshot aborted." and exits with 1 on Esc); otherwise its words are the error.
    pub fn interpret(succeeded: bool, picture: Vec<u8>, said: &str) -> Result<Outcome> {
        if !picture.is_empty() {
            return Ok(Outcome::Picture(picture));
        }
        let said = said.trim();
        let lower = said.to_lowercase();
        if succeeded || said.is_empty() || lower.contains("abort") || lower.contains("cancel") {
            return Ok(Outcome::Cancelled);
        }
        let tail: Vec<&str> = said.lines().rev().take(3).collect();
        let tail: String = tail.into_iter().rev().collect::<Vec<_>>().join(" ").chars().take(300).collect();
        Err(Error::screenshot(format!("The screenshot tool stopped with an error: {tail}")))
    }

    fn temp_file() -> PathBuf {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        std::env::temp_dir().join(format!("ledger-screenshot-{}-{nanos}.png", std::process::id()))
    }
}

// ---- pictures opened with Ledger --------------------------------------------------------------------

/// Pictures handed to Ledger by another program (Flameshot's "Open with", a file manager) as command-line
/// arguments, at startup or forwarded from a second launch: the first PNG among them becomes the screenshot
/// waiting to be attached. Anything else in the arguments is ignored.
pub fn open_files(app: &AppHandle, args: &[String], cwd: &Path) {
    let Some(path) = args.iter().map(|a| cwd.join(a)).find(|p| is_png_name(p) && p.is_file()) else {
        return;
    };
    let result = std::fs::read(&path).map_err(|e| Error::io(&path, e)).and_then(|png| hand_over(app, png));
    if let Err(e) = result {
        report(app, &e);
    }
}

pub(crate) fn is_png_name(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
}

// ---- handing over -----------------------------------------------------------------------------------

/// Keeps a finished PNG for attaching and tells the main window, which asks where it belongs.
fn hand_over(app: &AppHandle, png: Vec<u8>) -> Result<()> {
    let (width, height) = dimensions(&png)?;
    *lock(&state(app).screenshots.pending) = Some(png);
    let _ = app.emit("screenshot-ready", &Taken { width, height });
    reveal_main(app);
    Ok(())
}

/// The size of a PNG, read from its header; an error if it isn't one.
pub(crate) fn dimensions(png: &[u8]) -> Result<(u32, u32)> {
    ImageReader::with_format(Cursor::new(png), ImageFormat::Png)
        .into_dimensions()
        .map_err(|_| Error::screenshot("That isn't a PNG picture."))
}

/// The screenshot waiting to be attached, if any: the main window asks at startup, when Ledger was launched
/// with a picture before it was listening for `screenshot-ready`.
pub fn pending(app: &AppHandle) -> Option<Taken> {
    let app_state = state(app);
    let pending = lock(&app_state.screenshots.pending);
    let (width, height) = dimensions(pending.as_deref()?).ok()?;
    Some(Taken { width, height })
}

/// The screenshot waiting to be attached, as PNG bytes (for the preview).
pub fn pending_image(app: &AppHandle) -> Result<Vec<u8>> {
    lock(&state(app).screenshots.pending).clone().ok_or_else(|| Error::screenshot("There is no screenshot waiting."))
}

pub fn discard_pending(app: &AppHandle) {
    *lock(&state(app).screenshots.pending) = None;
}

/// The file name a screenshot is saved under.
pub fn file_name() -> String {
    format!("screenshot-{}.png", chrono::Utc::now().format("%Y%m%d-%H%M%S"))
}

pub fn reveal_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        // A focus request from the background (a hotkey, another program's "Open with") is often turned into a
        // "Ledger is ready" notice by the window manager (GNOME does this). Being on top for a moment brings the
        // window up anyway.
        let _ = window.set_always_on_top(true);
        let _ = window.set_focus();
        let _ = window.set_always_on_top(false);
    }
}

fn report(app: &AppHandle, error: &Error) {
    let _ = app.emit("screenshot-error", error.to_string());
    reveal_main(app);
}

// ---- the built-in capture ---------------------------------------------------------------------------

/// Ledger's own capture, used when no tool is set up under Settings → Screenshots.
///
/// - **macOS, Windows**: xcap grabs the monitor under the cursor, a frameless always-on-top window
///   (`/screenshot-overlay` in the frontend) shows it so a region can be dragged out, and the region is cropped
///   here, in capture pixels, so HiDPI screens come out exact.
/// - **Linux**: the desktop's screenshot portal (`portal`), which shows its own area picker. xcap is not built
///   on Linux: it needs PipeWire's development files to compile, for a recording feature Ledger doesn't use.
///
/// REMOVABLE. Your own tool (`command`) does the same job, with annotations, on every platform. Once nobody
/// depends on the built-in capture, it can go completely:
/// - this module, `capture`, `Region`, `crop` and `deliver` (and the crop test in `tests_screenshot.rs`);
/// - the commands `screenshot_image`, `screenshot_overlay_ready`, `screenshot_finish`, `screenshot_cancel`
///   (commands.rs, lib.rs, api.ts) and the `Region` type in types.ts;
/// - the `src/routes/screenshot-overlay` page, its window in `capabilities/default.json` and its check in
///   `routes/+layout.svelte`;
/// - `xcap`, and `core-graphics` (only the Screen Recording check uses it), in Cargo.toml;
/// - `ashpd`'s "screenshot" feature (keep "global_shortcuts": the Wayland hotkey needs it);
/// - the built-in choice in the Settings page, so the tool is all there is.
mod builtin {
    use super::*;

    #[cfg(target_os = "linux")]
    pub const DESCRIPTION: &str = "The desktop takes the screenshot (its screenshot portal) and shows its own area picker. No annotations.";
    #[cfg(target_os = "macos")]
    pub const DESCRIPTION: &str =
        "Ledger captures the screen under the cursor and you drag out an area. No annotations. Needs the Screen Recording permission.";
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub const DESCRIPTION: &str = "Ledger captures the screen under the cursor and you drag out an area. No annotations.";

    #[cfg(target_os = "linux")]
    pub fn begin(app: &AppHandle) -> Result<()> {
        let image = super::portal::capture()?;
        release(app);
        deliver(app.clone(), image, None);
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    pub fn begin(app: &AppHandle) -> Result<()> {
        let (image, monitor) = capture_monitor_under_cursor(app)?;
        *lock(&state(app).screenshots.capture) = Some(image);
        open_overlay(app, &monitor)
    }

    #[cfg(not(target_os = "linux"))]
    fn capture_monitor_under_cursor(app: &AppHandle) -> Result<(RgbaImage, tauri::Monitor)> {
        #[cfg(target_os = "macos")]
        ensure_screen_access()?;

        let cursor = app.cursor_position().map_err(Error::screenshot)?;
        let monitor = match app.monitor_from_point(cursor.x, cursor.y).map_err(Error::screenshot)? {
            Some(monitor) => monitor,
            None => app.primary_monitor().map_err(Error::screenshot)?.ok_or_else(|| Error::screenshot("No screen was found"))?,
        };

        // xcap addresses screens in the system's own units: physical pixels everywhere but macOS, which uses points.
        let unit = if cfg!(target_os = "macos") { monitor.scale_factor() } else { 1.0 };
        let (x, y) = ((f64::from(monitor.position().x) / unit).round() as i32, (f64::from(monitor.position().y) / unit).round() as i32);
        let screens = xcap::Monitor::all().map_err(Error::screenshot)?;
        let screen = match screens.into_iter().find(|s| s.x().ok() == Some(x) && s.y().ok() == Some(y)) {
            Some(screen) => screen,
            None => xcap::Monitor::from_point((cursor.x / unit) as i32, (cursor.y / unit) as i32).map_err(Error::screenshot)?,
        };
        Ok((screen.capture_image().map_err(Error::screenshot)?, monitor))
    }

    #[cfg(target_os = "macos")]
    fn ensure_screen_access() -> Result<()> {
        use core_graphics::access::ScreenCaptureAccess;
        if ScreenCaptureAccess.preflight() {
            return Ok(());
        }
        ScreenCaptureAccess.request(); // asks the first time; afterwards the user has to flip the switch themselves
        Err(Error::screenshot(MAC_PERMISSION))
    }

    #[cfg(target_os = "macos")]
    pub const MAC_PERMISSION: &str = "Ledger needs permission to record your screen. Open System Settings → Privacy & Security → \
        Screen & System Audio Recording, switch Ledger on, then quit and reopen Ledger and try again.";

    #[cfg(not(target_os = "linux"))]
    fn open_overlay(app: &AppHandle, monitor: &tauri::Monitor) -> Result<()> {
        use tauri::{WebviewUrl, WebviewWindowBuilder, WindowEvent};

        if let Some(stale) = app.get_webview_window(OVERLAY_LABEL) {
            let _ = stale.destroy();
        }
        // Hidden until the page has the image decoded (`overlay_ready`), so the screen never flashes blank.
        let window = WebviewWindowBuilder::new(app, OVERLAY_LABEL, WebviewUrl::App("screenshot-overlay".into()))
            .title("Ledger screenshot")
            .decorations(false)
            .transparent(true)
            .shadow(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .resizable(false)
            .visible(false)
            .build()
            .map_err(Error::screenshot)?;
        window.set_position(*monitor.position()).map_err(Error::screenshot)?;
        window.set_size(*monitor.size()).map_err(Error::screenshot)?;

        // However the overlay goes away — selection, Esc, or the window being closed — the next capture may begin.
        let handle = app.clone();
        window.on_window_event(move |event| {
            if matches!(event, WindowEvent::Destroyed) {
                release(&handle);
            }
        });
        Ok(())
    }
}

/// The capture, as PNG bytes for the overlay to display. Compression is traded for speed: this is a preview.
pub fn overlay_image(app: &AppHandle) -> Result<Vec<u8>> {
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    use image::{ExtendedColorType, ImageEncoder};

    let app_state = state(app);
    let capture = lock(&app_state.screenshots.capture);
    let image = capture.as_ref().ok_or_else(|| Error::screenshot("There is no screenshot in progress."))?;
    let mut png = Vec::new();
    PngEncoder::new_with_quality(&mut png, CompressionType::Fast, FilterType::NoFilter)
        .write_image(image.as_raw(), image.width(), image.height(), ExtendedColorType::Rgba8)
        .map_err(Error::screenshot)?;
    Ok(png)
}

pub fn overlay_ready(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(OVERLAY_LABEL) {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

pub fn cancel(app: &AppHandle) {
    close_overlay(app);
}

/// The region was chosen: close the overlay, then crop it in the background and hand it over for attaching.
pub fn finish(app: &AppHandle, region: Region) -> Result<()> {
    let image = lock(&state(app).screenshots.capture)
        .take()
        .ok_or_else(|| Error::screenshot("There is no screenshot in progress."))?;
    close_overlay(app);
    deliver(app.clone(), image, Some(region));
    Ok(())
}

fn close_overlay(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(OVERLAY_LABEL) {
        let _ = window.destroy();
    }
}

/// Crops a capture and hands it over, off the calling thread.
fn deliver(app: AppHandle, image: RgbaImage, region: Option<Region>) {
    std::thread::spawn(move || {
        let result = crop(image, region).and_then(|image| {
            let mut png = Vec::new();
            image.write_to(&mut Cursor::new(&mut png), ImageFormat::Png).map_err(Error::screenshot)?;
            hand_over(&app, png)
        });
        if let Err(e) = result {
            report(&app, &e);
        }
    });
}

/// The region, clamped to the image: whatever the overlay computed, the crop stays inside the capture.
pub(crate) fn crop(image: RgbaImage, region: Option<Region>) -> Result<RgbaImage> {
    let Some(r) = region else { return Ok(image) };
    let (width, height) = image.dimensions();
    if r.width == 0 || r.height == 0 || r.x >= width || r.y >= height {
        return Err(Error::screenshot("The selected area is empty."));
    }
    let (w, h) = (r.width.min(width - r.x), r.height.min(height - r.y));
    Ok(image::imageops::crop_imm(&image, r.x, r.y, w, h).to_image())
}

// ---- Linux: the desktop portals (the hotkey under Wayland, and the built-in capture) ---------------

#[cfg(target_os = "linux")]
mod portal {
    use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
    use ashpd::desktop::screenshot::Screenshot;
    use futures_util::StreamExt;
    use tauri::AppHandle;

    use super::{Error, Result, RgbaImage, lock, state, trigger};

    pub fn is_wayland() -> bool {
        std::env::var("XDG_SESSION_TYPE").is_ok_and(|t| t == "wayland") || std::env::var_os("WAYLAND_DISPLAY").is_some()
    }

    /// Asks the desktop for a screenshot. Interactive: the compositor shows its own area picker, so what comes
    /// back is already the region the user chose.
    pub fn capture() -> Result<RgbaImage> {
        tauri::async_runtime::block_on(async {
            let shot = Screenshot::request()
                .interactive(true)
                .send()
                .await
                .and_then(|request| request.response())
                .map_err(|e| Error::screenshot(format!("The desktop's screenshot portal isn't available: {e}")))?;
            let path = url::Url::parse(shot.uri().as_str())
                .ok()
                .and_then(|uri| uri.to_file_path().ok())
                .ok_or_else(|| Error::screenshot("The portal returned something that isn't a file."))?;
            let image = image::open(&path).map_err(Error::screenshot)?.to_rgba8();
            let _ = std::fs::remove_file(path);
            Ok(image)
        })
    }

    /// Asks the desktop's global-shortcut portal to bind `text`. The desktop decides what the key really is (and
    /// may ask the user), so a refusal comes back later, through `problem`, as an answer rather than a bug.
    pub fn bind(app: &AppHandle, text: &str) -> Result<()> {
        let preferred = xdg_trigger(text);
        let handle = app.clone();
        let task = tauri::async_runtime::spawn(async move {
            let outcome: ashpd::Result<()> = async {
                let portal = GlobalShortcuts::new().await?;
                let session = portal.create_session(Default::default()).await?;
                let shortcut = NewShortcut::new("screenshot", "Take a screenshot").preferred_trigger(preferred.as_deref());
                portal.bind_shortcuts(&session, &[shortcut], None, Default::default()).await?.response()?;

                let mut activations = Box::pin(portal.receive_activated().await?);
                while let Some(activation) = activations.next().await {
                    if activation.shortcut_id() == "screenshot" {
                        trigger(&handle);
                    }
                }
                Ok(())
            }
            .await;
            if let Err(e) = outcome {
                let app_state = state(&handle);
                *lock(&app_state.screenshots.problem) =
                    Some(format!("This desktop doesn't offer global shortcuts to apps ({e}). Use the button in Settings instead."));
            }
        });
        let app_state = state(app);
        if let Some(old) = lock(&app_state.screenshots.portal_task).replace(task) {
            old.abort(); // dropping the old session releases its binding
        }
        Ok(())
    }

    /// "Control+Shift+S" in the portal's notation, "CTRL+SHIFT+s"; `None` for keys it has no plain name for.
    fn xdg_trigger(text: &str) -> Option<String> {
        let mut parts = vec![];
        for part in text.split('+') {
            parts.push(match part.to_lowercase().as_str() {
                "commandorcontrol" | "cmdorctrl" | "control" | "ctrl" => "CTRL".to_string(),
                "alt" | "option" => "ALT".to_string(),
                "shift" => "SHIFT".to_string(),
                "super" | "command" | "cmd" => "LOGO".to_string(),
                key if key.len() == 1 => key.to_string(),
                key if key.starts_with('f') && key[1..].parse::<u8>().is_ok() => key.to_uppercase(),
                _ => return None,
            });
        }
        Some(parts.join("+"))
    }
}
