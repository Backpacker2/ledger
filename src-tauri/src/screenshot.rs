//! Screenshots. A global hotkey captures the monitor under the cursor, a frameless always-on-top window shows the
//! capture so a region can be dragged out, and the cropped PNG is kept here, in memory, while the main window comes
//! to the front and asks which thread it belongs to. Attaching it saves the file in that project's attachments and
//! records it in the thread like any other upload (see `uploads`).
//!
//! The overlay (`/screenshot-overlay` in the frontend) works in the capture's own pixels: it asks for the image,
//! scales the drag from window units to image pixels, and sends the region back. Nothing about the image
//! crosses the IPC boundary except that one PNG.
//!
//! On Linux under Wayland there is no way to grab the screen or a global key directly, so both go through the
//! XDG desktop portals (`portal`), and the compositor's own area picker replaces the overlay.

use std::io::Cursor;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ExtendedColorType, ImageEncoder, ImageFormat, RgbaImage};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, Monitor, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Shortcut, ShortcutState};

use crate::error::{Error, Result};
use crate::settings;
use crate::state::App;

pub const DEFAULT_HOTKEY: &str = "CommandOrControl+Shift+S";
const OVERLAY: &str = "screenshot-overlay";

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
    pub fn settings(&self, root: &std::path::Path) -> ScreenshotSettings {
        ScreenshotSettings {
            hotkey: saved_hotkey(root),
            default_hotkey: DEFAULT_HOTKEY.to_string(),
            problem: lock(&self.problem).clone(),
            note: platform_note(),
        }
    }
}

impl Default for Screenshots {
    fn default() -> Self {
        Screenshots::new()
    }
}

fn saved_hotkey(root: &std::path::Path) -> String {
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
    // Window creation and screen capture must not run on the thread the hotkey or a command arrived on.
    std::thread::spawn(move || {
        if let Err(e) = begin(&app) {
            release(&app);
            report(&app, &e);
        }
    });
}

fn begin(app: &AppHandle) -> Result<()> {
    #[cfg(target_os = "linux")]
    if portal::is_wayland() {
        let image = portal::capture()?;
        release(app);
        deliver(app.clone(), image, None);
        return Ok(());
    }

    let (image, monitor) = capture_monitor_under_cursor(app)?;
    *lock(&state(app).screenshots.capture) = Some(image);
    open_overlay(app, &monitor)
}

fn release(app: &AppHandle) {
    let shots = &state(app).screenshots;
    *lock(&shots.capture) = None;
    shots.busy.store(false, Ordering::Release);
}

fn capture_monitor_under_cursor(app: &AppHandle) -> Result<(RgbaImage, Monitor)> {
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
const MAC_PERMISSION: &str = "Ledger needs permission to record your screen. Open System Settings → Privacy & Security → \
    Screen & System Audio Recording, switch Ledger on, then quit and reopen Ledger and try again.";

fn platform_note() -> Option<String> {
    #[cfg(target_os = "macos")]
    if !core_graphics::access::ScreenCaptureAccess.preflight() {
        return Some(MAC_PERMISSION.to_string());
    }
    #[cfg(target_os = "linux")]
    if portal::is_wayland() {
        return Some("Under Wayland the desktop takes the screenshot: it shows its own area picker, and the shortcut goes through the desktop's global-shortcut portal. If your desktop doesn't offer that portal, use the button instead.".into());
    }
    None
}

// ---- the overlay ------------------------------------------------------------------------------------

fn open_overlay(app: &AppHandle, monitor: &Monitor) -> Result<()> {
    if let Some(stale) = app.get_webview_window(OVERLAY) {
        let _ = stale.destroy();
    }
    // Hidden until the page has the image decoded (`overlay_ready`), so the screen never flashes blank.
    let window = WebviewWindowBuilder::new(app, OVERLAY, WebviewUrl::App("screenshot-overlay".into()))
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

/// The capture, as PNG bytes for the overlay to display. Compression is traded for speed: this is a preview.
pub fn overlay_image(app: &AppHandle) -> Result<Vec<u8>> {
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
    if let Some(window) = app.get_webview_window(OVERLAY) {
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
    if let Some(window) = app.get_webview_window(OVERLAY) {
        let _ = window.destroy();
    }
}

// ---- cropping and handing over -------------------------------------------------------------------------

/// Crops the capture and keeps it for attaching, off the calling thread.
fn deliver(app: AppHandle, image: RgbaImage, region: Option<Region>) {
    std::thread::spawn(move || {
        let result = crop(image, region).and_then(|image| {
            let mut png = Vec::new();
            image.write_to(&mut Cursor::new(&mut png), ImageFormat::Png).map_err(Error::screenshot)?;
            Ok((Taken { width: image.width(), height: image.height() }, png))
        });
        match result {
            Ok((taken, png)) => {
                *lock(&state(&app).screenshots.pending) = Some(png);
                let _ = app.emit("screenshot-ready", &taken);
                reveal_main(&app);
            }
            Err(e) => report(&app, &e),
        }
    });
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

fn reveal_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn report(app: &AppHandle, error: &Error) {
    let _ = app.emit("screenshot-error", error.to_string());
    reveal_main(app);
}

// ---- Linux under Wayland ----------------------------------------------------------------------------

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
