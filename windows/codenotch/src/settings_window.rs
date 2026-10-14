//! The settings window. Created when it is opened and destroyed when it is closed, so no second
//! WebView sits hidden for the life of the app. Frameless over Windows 11's Mica, the closest
//! Windows material to the Mac's window vibrancy, and told what the page cannot read for itself:
//! whether Mica is there to draw on, and the accent colour.

use tauri::window::{Effect, EffectsBuilder};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

#[derive(Clone, PartialEq, Eq, serde::Serialize)]
struct SettingsNavigation {
    sequence: u64,
    section: String,
    provider: Option<String>,
    window_index: Option<usize>,
}
static PENDING_NAVIGATION: std::sync::Mutex<Option<SettingsNavigation>> = std::sync::Mutex::new(None);
static WINDOW_READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static NAVIGATION_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn acknowledge_navigation(pending: &mut Option<SettingsNavigation>, delivered: &SettingsNavigation, ready: bool) {
    if ready && pending.as_ref() == Some(delivered) { *pending = None; }
}

const LABEL: &str = "settings";
/// Mica arrived with Windows 11's first build.
const FIRST_MICA_BUILD: u32 = 22000;

/// Always built on a later turn of the event loop. A window built inside a synchronous command
/// deadlocks WebView2 and comes up blank, and asking `run_on_main_thread` from the main thread —
/// where those commands run — builds it on the spot, so the request is posted from another thread.
pub fn open(app: &AppHandle) {
    open_request(app, None);
}

pub fn open_section(app: &AppHandle, section: &str, provider: Option<&str>, window_index: Option<usize>) {
    if !matches!(section, "history" | "usage" | "monitor") { return; }
    let provider = provider.filter(|id| matches!(*id, "claude" | "codex" | "kiro")).map(str::to_string);
    let sequence = NAVIGATION_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::AcqRel) + 1;
    let route = SettingsNavigation { sequence, section: section.into(), provider, window_index: window_index.filter(|n| *n < 4096) };
    if let Ok(mut pending) = PENDING_NAVIGATION.lock() { *pending = Some(route.clone()); }
    open_request(app, Some(route));
}

fn open_request(app: &AppHandle, route: Option<SettingsNavigation>) {
    let handle = app.clone();
    std::thread::spawn(move || {
        let app = handle.clone();
        let _ = handle.run_on_main_thread(move || open_now(&app, route));
    });
}

fn open_now(app: &AppHandle, route: Option<SettingsNavigation>) {
    if route.as_ref().is_some_and(|route| route.sequence != NAVIGATION_SEQUENCE.load(std::sync::atomic::Ordering::Acquire)) { return; }
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        if let Some(route) = route {
            if w.emit("settings_navigate", &route).is_ok() {
                if let Ok(mut pending) = PENDING_NAVIGATION.lock() {
                    acknowledge_navigation(&mut pending, &route, WINDOW_READY.load(std::sync::atomic::Ordering::Acquire));
                }
            }
        }
        return;
    }
    WINDOW_READY.store(false, std::sync::atomic::Ordering::Release);
    if route.is_none() { if let Ok(mut pending) = PENDING_NAVIGATION.lock() { *pending = None; } }
    // History tables need room; the original compact layout remains the minimum size.
    let url = match route {
        Some(route) => {
            let mut url = format!("settings.html?section={}", route.section);
            if let Some(provider) = route.provider { url.push_str(&format!("&provider={provider}")); }
            if let Some(index) = route.window_index { url.push_str(&format!("&window_index={index}")); }
            url
        }
        None => "settings.html".into(),
    };
    let mut builder = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App(url.into()))
        .title("Codenotch Settings")
        .inner_size(900.0, 680.0)
        .min_inner_size(680.0, 520.0)
        .resizable(true)
        .maximizable(true)
        .decorations(false)
        .shadow(true)
        .center()
        // Shown by `settings_ready` once the page has drawn its first state. Shown at once, WebView2
        // paints white before the page does, and every switch slides from off to its real value.
        .visible(false);
    // Both before the build: a window that opens on the system appearance and is corrected after
    // shows the wrong one for a frame, which on a dark Windows under a light choice is a black flash
    let theme = crate::theme_choice(app);
    builder = builder
        .theme(theme)
        .initialization_script(crate::theme_script(crate::resolved_theme(app)));
    // Without Mica the window stays opaque and the page draws solid surfaces instead
    if has_mica() {
        builder = builder.transparent(true).effects(EffectsBuilder::new().effect(mica_for(theme)).build());
    }
    match builder.build() {
        // A page that never reports ready must not leave the window open but invisible
        Ok(w) => {
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(3));
                if !w.is_visible().unwrap_or(true) {
                    reveal(&w);
                }
            });
        }
        Err(e) => crate::applog(&format!("settings window: {e}")),
    }
}

#[tauri::command]
pub fn settings_ready(app: AppHandle) {
    if let Some(w) = app.get_webview_window(LABEL) {
        WINDOW_READY.store(true, std::sync::atomic::Ordering::Release);
        reveal(&w);
        if let Ok(mut pending) = PENDING_NAVIGATION.lock() {
            if let Some(route) = pending.take() { let _ = w.emit("settings_navigate", route); }
        }
    }
}

#[cfg(test)]
mod navigation_tests {
    use super::*;
    #[test]
    fn ready_navigation_is_consumed_without_losing_a_newer_request() {
        let first = SettingsNavigation { sequence: 1, section: "history".into(), provider: Some("codex".into()), window_index: None };
        let next = SettingsNavigation { sequence: 2, section: "usage".into(), provider: Some("kiro".into()), window_index: Some(0) };
        let mut pending = Some(first.clone());
        acknowledge_navigation(&mut pending, &first, false);
        assert!(pending.is_some());
        acknowledge_navigation(&mut pending, &first, true);
        assert!(pending.is_none());
        pending = Some(next.clone());
        acknowledge_navigation(&mut pending, &first, true);
        assert!(pending.as_ref() == Some(&next));
        acknowledge_navigation(&mut pending, &next, true);
        assert!(pending.is_none());
    }
}

/// Raised as well as shown: a window created while the app is not in front can come up behind.
fn reveal(w: &tauri::WebviewWindow) {
    let _ = w.show();
    let _ = w.set_focus();
}

#[derive(serde::Serialize)]
pub struct SystemLook {
    mica: bool,
    /// The accent palette as #rrggbb: light 3, light 2, light 1, accent, dark 1, dark 2, dark 3.
    accent: Vec<String>,
}

#[tauri::command]
pub fn get_system_look() -> SystemLook {
    SystemLook {
        mica: has_mica(),
        accent: reg_binary(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent", "AccentPalette")
            .map(|bytes| palette(&bytes))
            .unwrap_or_default(),
    }
}

#[tauri::command]
pub fn quit_app(app: AppHandle) {
    app.exit(0);
}

/// The credit line's link, as on the Mac.
#[tauri::command]
pub fn open_author_page() {
    let mut cmd = std::process::Command::new("cmd");
    cmd.args(["/C", "start", "", "https://x.com/hivinz_"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let _ = cmd.spawn();
}

/// Keeps Mica on the same side as the page above it.
///
/// Plain `Effect::Mica` follows the Windows setting, which is right for "System" and wrong for the
/// other two: a light page over dark Mica reads as a bug. Nothing to do without Mica, where the page
/// draws its own opaque surfaces.
pub fn follow_theme(app: &AppHandle, theme: Option<tauri::Theme>) {
    let Some(w) = app.get_webview_window(LABEL) else { return };
    if !has_mica() {
        return;
    }
    let _ = w.set_effects(EffectsBuilder::new().effect(mica_for(theme)).build());
}

/// Plain `Mica` follows the Windows setting, which is right for "System" and wrong for the other
/// two: a light page over dark Mica reads as a bug.
fn mica_for(theme: Option<tauri::Theme>) -> Effect {
    match theme {
        Some(tauri::Theme::Light) => Effect::MicaLight,
        Some(tauri::Theme::Dark) => Effect::MicaDark,
        _ => Effect::Mica,
    }
}

fn has_mica() -> bool {
    reg_string(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", "CurrentBuildNumber")
        .and_then(|build| build.trim().parse::<u32>().ok())
        .is_some_and(|build| build >= FIRST_MICA_BUILD)
}

fn palette(bytes: &[u8]) -> Vec<String> {
    let (colours, _) = bytes.as_chunks::<4>();
    colours.iter().take(7).map(|c| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])).collect()
}

#[cfg(windows)]
fn reg_binary(key: &str, value: &str) -> Option<Vec<u8>> {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_BINARY};
    let mut data = vec![0u8; 64];
    let mut size = data.len() as u32;
    reg_get(HKEY_CURRENT_USER, key, value, RRF_RT_REG_BINARY, data.as_mut_ptr().cast(), &mut size).then(|| {
        data.truncate(size as usize);
        data
    })
}

#[cfg(windows)]
fn reg_string(key: &str, value: &str) -> Option<String> {
    use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
    let mut data = vec![0u16; 64];
    let mut size = (data.len() * 2) as u32;
    reg_get(HKEY_LOCAL_MACHINE, key, value, RRF_RT_REG_SZ, data.as_mut_ptr().cast(), &mut size).then(|| {
        let chars = (size as usize / 2).min(data.len());
        String::from_utf16_lossy(&data[..chars]).trim_end_matches('\0').to_string()
    })
}

#[cfg(windows)]
fn reg_get(
    root: windows::Win32::System::Registry::HKEY,
    key: &str,
    value: &str,
    kind: windows::Win32::System::Registry::REG_ROUTINE_FLAGS,
    data: *mut core::ffi::c_void,
    size: &mut u32,
) -> bool {
    use windows::core::HSTRING;
    use windows::Win32::System::Registry::RegGetValueW;
    let (key, value) = (HSTRING::from(key), HSTRING::from(value));
    unsafe { RegGetValueW(root, &key, &value, kind, None, Some(data), Some(size)) }.is_ok()
}

#[cfg(not(windows))]
fn reg_binary(_key: &str, _value: &str) -> Option<Vec<u8>> {
    None
}

#[cfg(not(windows))]
fn reg_string(_key: &str, _value: &str) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::palette;

    #[test]
    fn the_palette_reads_seven_colours_and_drops_the_alpha_byte() {
        let mut bytes = Vec::new();
        for i in 0..8u8 {
            bytes.extend_from_slice(&[0x10 + i, 0x20 + i, 0x30 + i, 0xff]);
        }
        let colours = palette(&bytes);
        assert_eq!(colours.len(), 7);
        assert_eq!(colours[0], "#102030");
        assert_eq!(colours[6], "#162636");
    }
}
