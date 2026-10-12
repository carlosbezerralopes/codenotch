use tauri::{AppHandle, Manager};

/// Legacy entrypoint: history now lives in Settings, including first-use setup.
pub fn open(app: &AppHandle) {
    crate::settings_window::open_section(app, "history", None, None);
}

#[tauri::command]
pub fn telemetry_ready(app: AppHandle) {
    if let Some(window) = app.get_webview_window("settings") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}
