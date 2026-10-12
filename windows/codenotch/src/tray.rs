use crate::i18n::tr;
use crate::traymenu;
use tauri::menu::{Menu, MenuBuilder, MenuItemBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Wry};

pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    let menu = build_menu(app)?;
    let mut builder = TrayIconBuilder::with_id("main");
    // Windows does not tint tray icons and the monochrome outline vanishes on a dark taskbar, so
    // the app's own mark is the icon. trayicon::app_mark explains why at length.
    if let Some(icon) = crate::trayicon::app_mark() {
        builder = builder.icon(icon);
    }
    builder
        .tooltip(tooltip(app))
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, ev| handle(app, ev.id().as_ref()))
        .build(app)?;
    Ok(())
}

/// The readings themselves, as the Mac's menu bar shows them: a line per provider with its headline
/// figure, and under it one greyed line per limit window. The macOS menu is rebuilt as it opens;
/// Tauri has no such hook, so `refresh_menu` is called whenever a reading changes and once a minute
/// besides, which keeps "Resets in 12 min" honest.
/// Every line the menu would show, in order, with the id each carries. Kept apart from building the
/// menu so a refresh can tell whether anything visible changed before it swaps the menu out.
fn menu_lines(app: &AppHandle, lang: &str) -> Vec<(String, String, bool)> {
    let now = crate::now_ms();
    let calendar = crate::work_calendar::get_calendar_settings().ok();
    let (limit, model) = {
        let st = app.state::<crate::AppState>();
        let cfg = st.cfg.lock().unwrap();
        (cfg.antigravity_limit.clone(), cfg.antigravity_model.clone())
    };
    let mut lines = Vec::new();
    for id in crate::provider_ids_in_order(app) {
        let snap = crate::snapshot_of(app, id);
        if snap.status == "absent" {
            continue;
        }
        let head = traymenu::header(
            &crate::provider_display_name(app, id),
            crate::ring_window(id, &snap.windows, &limit, &model).filter(|w| w.count.is_none()).map(|w| w.used.clamp(0.0, 1.0)),
            traymenu::stale_since(&snap, now),
            now,
            lang,
        );
        lines.push((format!("header:{id}"), head, false));
        let insights = crate::provider_insights_from_snapshot(app, id, &snap, calendar.as_ref(), now);
        for (n, line) in traymenu::provider_lines(&snap, now, lang).iter().enumerate() {
            // Windows does not indent submenu-less items, so the indent is in the text.
            let insight = if matches!(id, "claude" | "codex" | "kiro") {
                snap.windows.get(n).and_then(|source| traymenu::matching_insight(&insights, source, n))
            } else { None };
            let supported = insight.is_some();
            lines.push((if supported { format!("insight:{id}:{n}") } else { format!("line:{id}:{n}") }, format!("    {line}"), supported));
            if let Some(window) = insight {
                for (detail_index, detail) in traymenu::insight_lines(window, lang).into_iter().enumerate() {
                    lines.push((format!("detail:{id}:{n}:{detail_index}"), format!("        {detail}"), false));
                }
            }
        }
        lines.push((format!("refresh:{id}"), format!("{}: {}", tr(lang, "refresh_all"), crate::provider_display_name(app, id)), true));
    }
    lines
}

pub fn build_menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let lang = language(app);
    build_menu_from(app, &lang, &menu_lines(app, &lang))
}

fn build_menu_from(app: &AppHandle, lang: &str, lines: &[(String, String, bool)]) -> tauri::Result<Menu<Wry>> {
    let lang = lang.to_string();
    let mut items: Vec<tauri::menu::MenuItem<Wry>> = Vec::new();
    for (id, text, enabled) in lines {
        items.push(MenuItemBuilder::with_id(id.clone(), text.clone()).enabled(*enabled).build(app)?);
    }
    if items.is_empty() {
        items.push(
            MenuItemBuilder::with_id("waiting", tr(&lang, "waiting"))
                .enabled(false)
                .build(app)?,
        );
    }
    let refresh = MenuItemBuilder::with_id("refresh", tr(&lang, "refresh_all")).build(app)?;
    let settings = MenuItemBuilder::with_id("settings", tr(&lang, "settings")).build(app)?;
    let history = MenuItemBuilder::with_id("history", if lang == "pt-BR" { "Histórico" } else { "History" }).build(app)?;
    let quit = MenuItemBuilder::with_id("quit", tr(&lang, "quit_app")).build(app)?;
    let mut menu = MenuBuilder::new(app);
    for item in &items {
        menu = menu.item(item);
    }
    menu.separator()
        .item(&refresh)
        .item(&settings)
        .item(&history)
        .separator()
        .item(&quit)
        .build()
}

/// The language the menu speaks, already resolved: `traymenu` picks its wording by code and has no
/// "auto" of its own.
pub(crate) fn language(app: &AppHandle) -> String {
    let st = app.state::<crate::AppState>();
    let raw = st.cfg.lock().unwrap().lang.clone();
    if raw == "auto" {
        crate::i18n::resolve_auto().to_string()
    } else {
        raw
    }
}

/// The hover text: the same figures the menu opens with, for when the menu is not open.
fn tooltip(app: &AppHandle) -> String {
    let now = crate::now_ms();
    let calendar = crate::work_calendar::get_calendar_settings().ok();
    let lang = language(app);
    let (limit, model) = {
        let st = app.state::<crate::AppState>();
        let cfg = st.cfg.lock().unwrap();
        (cfg.antigravity_limit.clone(), cfg.antigravity_model.clone())
    };
    let mut parts: Vec<String> = Vec::new();
    for id in crate::provider_ids_in_order(app) {
        let snap = crate::snapshot_of(app, id);
        if snap.status == "absent" {
            continue;
        }
        let headline = crate::ring_window(id, &snap.windows, &limit, &model).filter(|w| w.count.is_none());
        let value = headline.map(|w| w.used.clamp(0.0, 1.0))
            .map(|f| format!("{}%", traymenu::pct(f)))
            .unwrap_or_else(|| "—".into());
        let dto = crate::provider_insights_from_snapshot(app, id, &snap, calendar.as_ref(), now);
        let detail = dto.windows.iter().find(|w| w.quota_role == Some(crate::usage_insights::QuotaRole::General))
            .and_then(|w| traymenu::insight_tooltip(w, &lang))
            .map(|text| format!(" · {text}")).unwrap_or_default();
        parts.push(format!("{} {value}{detail}", crate::provider_display_name(app, id)));
    }
    if parts.is_empty() {
        concat!("Codenotch v", env!("CARGO_PKG_VERSION")).to_string()
    } else {
        traymenu::bounded_tooltip(&format!("Codenotch — {}", parts.join(" · ")))
    }
}

/// Rebuilds the tray menu, ALWAYS on the main thread.
///
/// A menu is a Windows UI object. Building one or swapping it in from another thread leaves the
/// tray holding a menu that never opens again — and since changing the language is what triggers a
/// rebuild, the user is then locked out of the only place they could change it back. The tray's own
/// click handlers already run on the main thread, but the readings poller and the settings window
/// do not, so the hop is done here once rather than being remembered at every call site.
/// What the menu last showed, so an unchanged refresh leaves it alone.
static SHOWN: std::sync::Mutex<Option<(String, Vec<(String, String, bool)>)>> = std::sync::Mutex::new(None);

/// Swaps the menu only when a line of it would read differently. `set_menu` replaces the menu the
/// user may have open this moment — the refresh runs on the main thread, which the open popup's
/// message loop still serves — so the minute tick used to close it under the pointer even when
/// "Resets in 12 min" still said 12 min.
pub fn refresh_menu(app: &AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(tray) = handle.tray_by_id("main") {
            let lang = language(&handle);
            let lines = menu_lines(&handle, &lang);
            let key = (lang.clone(), lines.clone());
            if SHOWN.lock().unwrap().as_ref() == Some(&key) {
                // A tooltip can change without a line changing, and setting it closes nothing.
                let _ = tray.set_tooltip(Some(&tooltip(&handle)));
                return;
            }
            match build_menu_from(&handle, &lang, &lines) {
                Ok(menu) => {
                    let _ = tray.set_menu(Some(menu));
                    let _ = tray.set_tooltip(Some(&tooltip(&handle)));
                    *SHOWN.lock().unwrap() = Some(key);
                }
                Err(e) => crate::applog(&format!("tray menu: {e}")),
            }
        }
    });
}

/// Provider rows carry `refresh:<id>`; everything else the menu offers is one of the four fixed
/// items. Settings, language, hooks and the rest arrive as commands from the settings window.
fn handle(app: &AppHandle, id: &str) {
    if let Some((provider, index)) = traymenu::insight_target(id) {
        crate::settings_window::open_section(app, "usage", Some(provider), Some(index));
        return;
    }
    if let Some(provider) = id.strip_prefix("refresh:") {
        crate::refresh_provider(app, provider);
        return;
    }
    match id {
        "refresh" => crate::refresh_all(app),
        "settings" => crate::settings_window::open(app),
        "history" => crate::settings_window::open_section(app, "history", None, None),
        "quit" => app.exit(0),
        _ => {}
    }
}
