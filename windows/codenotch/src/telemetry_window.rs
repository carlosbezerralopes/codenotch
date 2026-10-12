use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

const LABEL: &str = "telemetry";

pub fn open(app: &AppHandle) {
    let handle = app.clone();
    std::thread::spawn(move || {
        let app = handle.clone();
        let _ = handle.run_on_main_thread(move || open_now(&app));
    });
}

fn open_now(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    match WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("telemetry.html".into()))
        .title("Codenotch Telemetry")
        .inner_size(960.0, 680.0)
        .min_inner_size(640.0, 480.0)
        .resizable(true)
        .center()
        .theme(crate::theme_choice(app))
        .initialization_script(crate::theme_script(crate::resolved_theme(app)))
        .visible(false)
        .build()
    {
        Ok(w) => {
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(3));
                if !w.is_visible().unwrap_or(true) {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            });
        }
        Err(e) => crate::applog(&format!("telemetry window: {e}")),
    }
}

#[tauri::command]
pub fn telemetry_ready(app: AppHandle) {
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.show();
        let _ = w.set_focus();
    }
}
