//! Desktop overlay — "evolve any app".
//!
//! A global shortcut (default ⌘⇧E / Ctrl+Shift+E, override with
//! `EVOLVO_DESKTOP_SHORTCUT`) snapshots whatever application is in front,
//! freezes the monitor it is on, and opens a borderless always-on-top
//! window showing that frozen frame with the Canvas + Feedback panel on
//! top. Submitting goes through the ordinary `submit_feedback` command with
//! a `targetApp` attached, so the feedback → lineage pipeline (I-P1) is the
//! same one in-app feedback uses.
//!
//! Capture happens *before* the overlay is shown, so — unlike the in-app
//! hide → capture → composite dance — nothing of Evolvo can leak into the
//! screenshot, and the user draws on exactly what they saw.
//!
//! The in-app white canvas (Shell's Feedback FAB) is untouched: this is a
//! second, separate surface for *other* applications.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::types::{current_time_unix_ms, TargetApp};

pub const OVERLAY_LABEL: &str = "desktop-overlay";
pub const CAPTURE_READY_EVENT: &str = "evolvo://desktop-capture";
pub const DEFAULT_SHORTCUT: &str = "CommandOrControl+Shift+E";

/// Longest edge (px) of the stored frozen frame. Retina full-screen PNGs
/// otherwise blow past the 8 MB per-attachment cap.
const MAX_CAPTURE_EDGE: u32 = 2560;

/// System processes that own on-screen windows but are never the app the
/// user means (menu bar, Dock, notification centre, …).
const IGNORED_APP_NAMES: &[&str] = &[
    "Window Server",
    "Dock",
    "Control Center",
    "ControlCenter",
    "Notification Center",
    "NotificationCenter",
    "SystemUIServer",
    "Spotlight",
    "loginwindow",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSession {
    /// PNG of the frozen monitor, base64 (no data: prefix).
    pub screenshot_base64: String,
    pub width: u32,
    pub height: u32,
    /// `None` when the frontmost app could not be determined (or was
    /// Evolvo itself); the feedback is then saved without a target.
    pub target_app: Option<TargetApp>,
    pub captured_at_unix_ms: u64,
    /// Set when the overlay was opened by "Revise on Canvas" for an earlier
    /// desktop feedback: the panel hydrates that record's annotations and
    /// the frame is the original frozen screen (`frame.png`).
    #[serde(default)]
    pub revision_of: Option<String>,
    #[serde(default)]
    pub revision_agent: Option<crate::types::AgentKind>,
    /// Hide Evolvo on close so focus returns to the captured app. True for
    /// shortcut captures; false for revisions, which start in Evolvo.
    #[serde(default)]
    pub return_to_target: bool,
    /// Visible windows on the captured monitor, front to back, for the
    /// overlay's window picker (W). Rects are in CSS px / points relative
    /// to the monitor — the overlay window is sized to the monitor, so they
    /// map 1:1 onto the overlay viewport.
    #[serde(default)]
    pub windows: Vec<PickableWindow>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PickableWindow {
    pub id: u32,
    pub app_name: String,
    pub title: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// The window of the app that was frontmost at shortcut time — the
    /// picker pre-highlights it so Enter means "the active window".
    pub focused: bool,
}

/// A single window captured on its own (window mode).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WindowCapture {
    pub screenshot_base64: String,
    pub width: u32,
    pub height: u32,
    pub target_app: Option<TargetApp>,
}

/// Windows smaller than this (after clipping to the monitor) are not
/// offered by the picker — tooltips, badges, menu-bar extras.
const MIN_PICKABLE_EDGE: u32 = 40;

/// Process-wide holder for the capture currently displayed by the overlay.
#[derive(Default)]
pub struct DesktopState {
    session: Mutex<Option<DesktopSession>>,
}

impl DesktopState {
    pub fn current(&self) -> Option<DesktopSession> {
        self.session.lock().ok().and_then(|s| s.clone())
    }

    fn set(&self, value: Option<DesktopSession>) {
        if let Ok(mut s) = self.session.lock() {
            *s = value;
        }
    }
}

pub fn shortcut_from_env() -> String {
    std::env::var("EVOLVO_DESKTOP_SHORTCUT")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_SHORTCUT.to_string())
}

/// Shortcut handler entry point. Capture runs off the event-loop thread
/// (monitor capture + PNG encode take a few hundred ms) and the overlay is
/// shown once the frame is ready.
pub fn trigger(app: &AppHandle) {
    // A second press while the overlay is up closes it (toggle), matching
    // the in-app FAB's open/close semantics.
    if let Some(win) = app.get_webview_window(OVERLAY_LABEL) {
        if win.is_visible().unwrap_or(false) {
            close_overlay(app);
            return;
        }
    }
    let app = app.clone();
    std::thread::spawn(move || match capture_session() {
        Ok((mut session, monitor)) => {
            // Feedback on a version Evolvo launched continues that job.
            if let (Some(target), Some(state)) = (
                session.target_app.as_mut(),
                app.try_state::<crate::state::AppState>(),
            ) {
                crate::apprun::link_launched_build(&state.store(), target);
            }
            if let Some(state) = app.try_state::<DesktopState>() {
                state.set(Some(session.clone()));
            }
            if let Err(err) = show_overlay(&app, &monitor) {
                eprintln!("evolvo: desktop overlay failed to open: {err}");
                return;
            }
            let _ = app.emit_to(
                OVERLAY_LABEL,
                CAPTURE_READY_EVENT,
                &session.captured_at_unix_ms,
            );
        }
        Err(err) => eprintln!("evolvo: desktop capture failed: {err}"),
    });
}

/// Hide the overlay and, when the capture targeted another app, hand focus
/// back to it instead of leaving Evolvo's main window in front.
pub fn close_overlay(app: &AppHandle) {
    let had_external_target = app
        .try_state::<DesktopState>()
        .and_then(|s| s.current())
        .map(|s| s.return_to_target && s.target_app.is_some())
        .unwrap_or(false);
    if let Some(win) = app.get_webview_window(OVERLAY_LABEL) {
        let _ = win.hide();
    }
    if let Some(state) = app.try_state::<DesktopState>() {
        state.set(None);
    }
    #[cfg(target_os = "macos")]
    if had_external_target {
        let _ = app.hide();
    }
    #[cfg(not(target_os = "macos"))]
    let _ = had_external_target;
}

struct MonitorRect {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

fn capture_session() -> Result<(DesktopSession, MonitorRect), String> {
    let own_pid = std::process::id();
    let candidates = eligible_windows(own_pid);
    let front = frontmost_of(&candidates);

    let monitor = front
        .as_ref()
        .and_then(|w| w.current_monitor().ok())
        .or_else(primary_monitor)
        .ok_or_else(|| "no monitor available to capture".to_string())?;
    let rect = MonitorRect {
        x: monitor.x().map_err(|e| e.to_string())?,
        y: monitor.y().map_err(|e| e.to_string())?,
        width: monitor.width().map_err(|e| e.to_string())?,
        height: monitor.height().map_err(|e| e.to_string())?,
    };

    let rgba = monitor.capture_image().map_err(|e| e.to_string())?;
    let rgba = downscale(rgba, MAX_CAPTURE_EDGE);
    let (width, height) = (rgba.width(), rgba.height());
    let mut buf: Vec<u8> = Vec::new();
    image::DynamicImage::ImageRgba8(rgba)
        .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;

    let target_app = front.as_ref().and_then(describe_window);
    let front_pid = front.as_ref().and_then(|w| w.pid().ok());
    let windows = pickable_windows(&candidates, &rect, front_pid);
    Ok((
        DesktopSession {
            screenshot_base64: STANDARD.encode(&buf),
            width,
            height,
            target_app,
            captured_at_unix_ms: current_time_unix_ms(),
            revision_of: None,
            revision_agent: None,
            return_to_target: true,
            windows,
        },
        rect,
    ))
}

/// Window mode: capture one window on its own (other windows covering it,
/// including this overlay, are excluded by the OS), and describe its app.
pub fn capture_window(id: u32) -> Result<WindowCapture, String> {
    let window = xcap::Window::all()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|w| w.id().ok() == Some(id))
        .ok_or_else(|| "that window is no longer open".to_string())?;
    let rgba = window.capture_image().map_err(|e| e.to_string())?;
    let rgba = downscale(rgba, MAX_CAPTURE_EDGE);
    let (width, height) = (rgba.width(), rgba.height());
    let mut buf: Vec<u8> = Vec::new();
    image::DynamicImage::ImageRgba8(rgba)
        .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(WindowCapture {
        screenshot_base64: STANDARD.encode(&buf),
        width,
        height,
        target_app: describe_window(&window),
    })
}

/// Clip a window rect (global points) to the monitor and return it relative
/// to the monitor origin, or `None` when too little of it is on this monitor.
fn rect_on_monitor(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    monitor: &MonitorRect,
) -> Option<(i32, i32, u32, u32)> {
    let left = x.max(monitor.x);
    let top = y.max(monitor.y);
    let right = (x + width as i32).min(monitor.x + monitor.width as i32);
    let bottom = (y + height as i32).min(monitor.y + monitor.height as i32);
    if right - left < MIN_PICKABLE_EDGE as i32 || bottom - top < MIN_PICKABLE_EDGE as i32 {
        return None;
    }
    Some((
        left - monitor.x,
        top - monitor.y,
        (right - left) as u32,
        (bottom - top) as u32,
    ))
}

fn pickable_windows(
    candidates: &[xcap::Window],
    monitor: &MonitorRect,
    front_pid: Option<u32>,
) -> Vec<PickableWindow> {
    let mut focused_assigned = false;
    candidates
        .iter()
        .filter_map(|w| {
            let (x, y, width, height) = rect_on_monitor(
                w.x().ok()?,
                w.y().ok()?,
                w.width().ok()?,
                w.height().ok()?,
                monitor,
            )?;
            // Only the frontmost window of the active app counts as focused.
            let focused = !focused_assigned && w.pid().ok().is_some() && w.pid().ok() == front_pid;
            focused_assigned |= focused;
            Some(PickableWindow {
                id: w.id().ok()?,
                app_name: w.app_name().unwrap_or_default(),
                title: w.title().unwrap_or_default(),
                x,
                y,
                width,
                height,
                focused,
            })
        })
        .collect()
}

/// "Revise on Canvas" for a desktop feedback: reopen the overlay on the
/// primary monitor with the original frozen frame, so the user redraws over
/// the screen they captured rather than over Evolvo's white canvas.
pub fn open_revision(
    app: &AppHandle,
    feedback_id: &str,
    agent: crate::types::AgentKind,
    frame_png: &[u8],
    target_app: Option<TargetApp>,
) -> Result<(), String> {
    let monitor = primary_monitor().ok_or_else(|| "no monitor available".to_string())?;
    let rect = MonitorRect {
        x: monitor.x().map_err(|e| e.to_string())?,
        y: monitor.y().map_err(|e| e.to_string())?,
        width: monitor.width().map_err(|e| e.to_string())?,
        height: monitor.height().map_err(|e| e.to_string())?,
    };
    let (width, height) = image::load_from_memory(frame_png)
        .map(|img| (img.width(), img.height()))
        .unwrap_or((rect.width, rect.height));
    let session = DesktopSession {
        screenshot_base64: STANDARD.encode(frame_png),
        width,
        height,
        target_app,
        captured_at_unix_ms: current_time_unix_ms(),
        revision_of: Some(feedback_id.to_string()),
        revision_agent: Some(agent),
        return_to_target: false,
        windows: Vec::new(),
    };
    if let Some(state) = app.try_state::<DesktopState>() {
        state.set(Some(session.clone()));
    }
    show_overlay(app, &rect)?;
    let _ = app.emit_to(
        OVERLAY_LABEL,
        CAPTURE_READY_EVENT,
        &session.captured_at_unix_ms,
    );
    Ok(())
}

fn downscale(img: image::RgbaImage, max_edge: u32) -> image::RgbaImage {
    let (w, h) = (img.width(), img.height());
    let longest = w.max(h);
    if longest <= max_edge || longest == 0 {
        return img;
    }
    let scale = max_edge as f64 / longest as f64;
    let nw = ((w as f64 * scale).round() as u32).max(1);
    let nh = ((h as f64 * scale).round() as u32).max(1);
    image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Triangle)
}

fn primary_monitor() -> Option<xcap::Monitor> {
    let monitors = xcap::Monitor::all().ok()?;
    let primary = monitors
        .iter()
        .position(|m| m.is_primary().unwrap_or(false))
        .unwrap_or(0);
    monitors.into_iter().nth(primary)
}

/// The focused, visible window of the frontmost application, excluding
/// Evolvo itself and OS chrome. `xcap::Window::all()` is ordered front to
/// back, so the first eligible focused window is the one the user sees.
/// On-screen windows of other apps, front to back (the order
/// `xcap::Window::all()` returns), excluding Evolvo itself and OS chrome.
fn eligible_windows(own_pid: u32) -> Vec<xcap::Window> {
    let Ok(windows) = xcap::Window::all() else {
        return Vec::new();
    };
    windows
        .into_iter()
        .filter(|w| {
            let pid_ok = w.pid().map(|p| p != own_pid).unwrap_or(false);
            let visible = !w.is_minimized().unwrap_or(false)
                && w.width().unwrap_or(0) > 1
                && w.height().unwrap_or(0) > 1;
            let name = w.app_name().unwrap_or_default();
            pid_ok && visible && !IGNORED_APP_NAMES.contains(&name.as_str())
        })
        .collect()
}

/// The focused window of the frontmost application, else the topmost
/// eligible window.
fn frontmost_of(candidates: &[xcap::Window]) -> Option<xcap::Window> {
    candidates
        .iter()
        .find(|w| w.is_focused().unwrap_or(false))
        .or_else(|| candidates.first())
        .cloned()
}

fn describe_window(w: &xcap::Window) -> Option<TargetApp> {
    let name = w.app_name().ok().filter(|n| !n.trim().is_empty())?;
    let pid = w.pid().ok();
    let executable_path = pid.and_then(executable_for_pid);
    let bundle_id = executable_path
        .as_deref()
        .and_then(|p| bundle_id_for_executable(Path::new(p)));
    let repo_path = executable_path
        .as_deref()
        .and_then(|p| guess_repo_from_executable(Path::new(p), home_dir().as_deref()))
        .map(|p| p.display().to_string());
    let (repo_source, repo_status) = if repo_path.is_some() {
        ("executable".to_string(), "resolved".to_string())
    } else {
        (String::new(), String::new())
    };
    Some(TargetApp {
        name,
        window_title: w.title().unwrap_or_default(),
        pid,
        bundle_id,
        executable_path,
        repo_path,
        repo_source,
        repo_status,
        ..Default::default()
    })
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(target_os = "linux")]
fn executable_for_pid(pid: u32) -> Option<String> {
    std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .map(|p| p.display().to_string())
}

#[cfg(not(target_os = "linux"))]
fn executable_for_pid(pid: u32) -> Option<String> {
    // `ps -o comm=` prints the full executable path on macOS.
    let out = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!path.is_empty()).then_some(path)
}

/// For an executable inside `Foo.app/Contents/MacOS/foo`, read
/// `CFBundleIdentifier` from the bundle's Info.plist.
fn bundle_id_for_executable(exe: &Path) -> Option<String> {
    let bundle = enclosing_app_bundle(exe)?;
    let plist = bundle.join("Contents").join("Info.plist");
    if !plist.is_file() {
        return None;
    }
    let out = Command::new("plutil")
        .args(["-extract", "CFBundleIdentifier", "raw", "-o", "-"])
        .arg(&plist)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!id.is_empty()).then_some(id)
}

fn enclosing_app_bundle(exe: &Path) -> Option<PathBuf> {
    exe.ancestors()
        .find(|p| p.extension().map(|e| e == "app").unwrap_or(false))
        .map(Path::to_path_buf)
}

/// Walk up from a running executable to the nearest git checkout. This is
/// what makes "an open-source app I built locally" resolve automatically:
/// `~/code/foo/target/release/foo` → `~/code/foo`. Stops before `home` so a
/// dotfiles repo at `$HOME` never masquerades as an app's source.
pub fn guess_repo_from_executable(exe: &Path, home: Option<&Path>) -> Option<PathBuf> {
    for dir in exe.ancestors().skip(1) {
        if home.map(|h| dir == h).unwrap_or(false) || dir.parent().is_none() {
            return None;
        }
        if dir.join(".git").exists() {
            return Some(dir.to_path_buf());
        }
    }
    None
}

fn show_overlay(app: &AppHandle, rect: &MonitorRect) -> Result<(), String> {
    let win = match app.get_webview_window(OVERLAY_LABEL) {
        Some(win) => win,
        None => WebviewWindowBuilder::new(app, OVERLAY_LABEL, WebviewUrl::default())
            .title("Evolvo — Desktop feedback")
            // The UI picks its root component from this flag (see
            // `ui/src/main.rs`): the overlay mounts canvas + panel only.
            .initialization_script("window.__EVOLVO_MODE__ = 'desktop';")
            .decorations(false)
            .resizable(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .visible_on_all_workspaces(true)
            .visible(false)
            .build()
            .map_err(|e| e.to_string())?,
    };
    let pos = tauri::LogicalPosition::new(rect.x as f64, rect.y as f64);
    let size = tauri::LogicalSize::new(rect.width as f64, rect.height as f64);
    win.set_position(pos).map_err(|e| e.to_string())?;
    win.set_size(size).map_err(|e| e.to_string())?;
    win.show().map_err(|e| e.to_string())?;
    let _ = win.set_always_on_top(true);
    let _ = win.set_focus();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_guess_walks_up_from_build_output() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("code").join("foo");
        let exe_dir = repo.join("target").join("release");
        std::fs::create_dir_all(&exe_dir).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let exe = exe_dir.join("foo");
        std::fs::write(&exe, b"").unwrap();
        assert_eq!(guess_repo_from_executable(&exe, None), Some(repo));
    }

    #[test]
    fn repo_guess_stops_at_home() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(home.join(".git")).unwrap();
        let exe_dir = home.join("Applications").join("Foo.app").join("Contents");
        std::fs::create_dir_all(&exe_dir).unwrap();
        let exe = exe_dir.join("foo");
        assert_eq!(guess_repo_from_executable(&exe, Some(&home)), None);
    }

    #[test]
    fn repo_guess_none_for_installed_app() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("Applications/Foo.app/Contents/MacOS/foo");
        assert_eq!(guess_repo_from_executable(&exe, None), None);
    }

    #[test]
    fn finds_enclosing_app_bundle() {
        let exe = Path::new("/Applications/Foo.app/Contents/MacOS/foo");
        assert_eq!(
            enclosing_app_bundle(exe),
            Some(PathBuf::from("/Applications/Foo.app"))
        );
        assert_eq!(enclosing_app_bundle(Path::new("/usr/bin/foo")), None);
    }

    #[test]
    fn rect_on_monitor_clips_and_relativises() {
        let mon = MonitorRect {
            x: 1512,
            y: 0,
            width: 1920,
            height: 1080,
        };
        // Fully inside a secondary monitor → relative to its origin.
        assert_eq!(
            rect_on_monitor(1612, 100, 800, 600, &mon),
            Some((100, 100, 800, 600))
        );
        // Straddling the left edge → clipped to the visible part.
        assert_eq!(
            rect_on_monitor(1400, 50, 400, 300, &mon),
            Some((0, 50, 288, 300))
        );
        // On another monitor, or a sliver → not pickable.
        assert_eq!(rect_on_monitor(0, 0, 800, 600, &mon), None);
        assert_eq!(rect_on_monitor(1500, 0, 30, 600, &mon), None);
    }

    #[test]
    fn downscale_caps_longest_edge() {
        let img = image::RgbaImage::new(5120, 2880);
        let out = downscale(img, 2560);
        assert_eq!((out.width(), out.height()), (2560, 1440));
        let small = image::RgbaImage::new(800, 600);
        assert_eq!(downscale(small, 2560).dimensions(), (800, 600));
    }

    #[test]
    fn session_round_trips_camel_case() {
        let s = DesktopSession {
            screenshot_base64: "AA==".into(),
            width: 10,
            height: 5,
            target_app: Some(TargetApp {
                name: "Foo".into(),
                ..Default::default()
            }),
            captured_at_unix_ms: 1,
            revision_of: None,
            revision_agent: None,
            return_to_target: true,
            windows: Vec::new(),
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("screenshotBase64"));
        assert!(json.contains("targetApp"));
        let back: DesktopSession = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }
}
