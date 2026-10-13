//! whisperflow desktop app: tray icon, floating "listening" pill, and the
//! history window, around the dictation engine from the core crate.

// No console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod pill;

use serde::Serialize;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use whisperflow::engine::{self, Event};
use whisperflow::storage::{Entry, Storage};
use whisperflow::{cleanup, hotkey};

/// Passed by the "start at login" entry, so the app starts quietly in the tray.
const AUTOSTART_ARG: &str = "--minimized";
const HISTORY_LIMIT: usize = 500;

/// What the history window shows in its status line.
#[derive(Clone, Serialize)]
struct Status {
    /// "loading", "ready" or "error".
    state: &'static str,
    detail: String,
}

struct AppState {
    db_path: PathBuf,
    status: Mutex<Status>,
}

#[tauri::command]
fn get_history(state: tauri::State<AppState>) -> Result<Vec<Entry>, String> {
    let storage = Storage::open(&state.db_path).map_err(|e| format!("{e:#}"))?;
    storage.prune().map_err(|e| format!("{e:#}"))?;
    storage.recent(HISTORY_LIMIT).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn delete_entry(state: tauri::State<AppState>, id: i64) -> Result<(), String> {
    let storage = Storage::open(&state.db_path).map_err(|e| format!("{e:#}"))?;
    storage.delete(id).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn get_status(state: tauri::State<AppState>) -> Status {
    state.status.lock().unwrap().clone()
}

fn main() {
    tauri::Builder::default()
        // A second copy would install a second keyboard hook and paste twice.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_history(app)))
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec![AUTOSTART_ARG])))
        .invoke_handler(tauri::generate_handler![get_history, delete_entry, get_status])
        .setup(|app| {
            let db_path = app.path().app_data_dir()?.join("whisperflow.db");
            app.manage(AppState {
                db_path: db_path.clone(),
                status: Mutex::new(Status { state: "loading", detail: "Loading models…".into() }),
            });

            pill::create(app.handle())?;
            let started_at_login = std::env::args().any(|a| a == AUTOSTART_ARG);
            WebviewWindowBuilder::new(app, "history", WebviewUrl::default())
                .title("whisperflow")
                .inner_size(560.0, 680.0)
                .min_inner_size(400.0, 360.0)
                .visible(!started_at_login)
                .build()?;
            build_tray(app.handle())?;
            start_engine(app.handle().clone(), db_path);
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the history window only hides it; the app lives in the tray.
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "history" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("failed to start whisperflow")
        .run(|_app, event| {
            // Keep running with no windows open; only "Quit" (which passes
            // an exit code) ends the app.
            if let RunEvent::ExitRequested { api, code: None, .. } = event {
                api.prevent_exit();
            }
        });
}

fn show_history(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("history") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open history", true, None::<&str>)?;
    let autostart_on = app.autolaunch().is_enabled().unwrap_or(false);
    let autostart = CheckMenuItem::with_id(app, "autostart", "Start at login", true, autostart_on, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit whisperflow", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[&open, &PredefinedMenuItem::separator(app)?, &autostart, &PredefinedMenuItem::separator(app)?, &quit],
    )?;

    TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().cloned().expect("app icon"))
        .tooltip("whisperflow: loading")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "open" => show_history(app),
            "autostart" => {
                let manager = app.autolaunch();
                let result = if manager.is_enabled().unwrap_or(false) { manager.disable() } else { manager.enable() };
                if let Err(e) = result {
                    eprintln!("Could not change start at login: {e}");
                }
                // Show the real state, whether or not the change worked.
                let _ = autostart.set_checked(manager.is_enabled().unwrap_or(false));
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                show_history(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

fn set_status(app: &AppHandle, state: &'static str, detail: String) {
    let status = Status { state, detail };
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(format!("whisperflow: {}", status.detail)));
    }
    *app.state::<AppState>().status.lock().unwrap() = status.clone();
    let _ = app.emit("status", status);
}

/// Run the dictation loop on its own thread and mirror its events in the UI.
fn start_engine(app: AppHandle, db_path: PathBuf) {
    std::thread::spawn(move || {
        let threads = std::thread::available_parallelism().map(|n| n.get().min(8)).unwrap_or(4) as i32;
        let config = engine::Config {
            model: None,
            threads,
            llm: Some(cleanup::DEFAULT_MODEL.into()),
            paste: true,
            save_wav: false,
            history_db: Some(db_path),
        };
        let handle = app.clone();
        let result = engine::run(config, move |event| on_engine_event(&handle, event));
        if let Err(e) = result {
            set_status(&app, "error", format!("{e:#}"));
            pill::hide(&app);
            show_history(&app);
        }
    });
}

fn on_engine_event(app: &AppHandle, event: Event) {
    // The pill and history windows render from the same events. Send first,
    // so the pill already shows the new state when it appears.
    let _ = app.emit("engine", event.clone());
    match &event {
        Event::Loading => set_status(app, "loading", "Loading models…".into()),
        Event::Ready { model, llm, .. } => {
            let cleanup = llm.as_deref().unwrap_or("no cleanup");
            set_status(app, "ready", format!("Ready. {}. {model}, {cleanup}", hotkey_hint()));
        }
        Event::Listening => pill::show(app),
        Event::Done(d) => {
            let _ = app.emit("history-changed", ());
            if d.pasted {
                pill::hide(app);
            } else {
                pill::hide_after(app, std::time::Duration::from_millis(2500));
            }
        }
        Event::Ignored { .. } => pill::hide(app),
        Event::Transcribing | Event::CleaningUp | Event::Warning { .. } => {}
    }
}

fn hotkey_hint() -> &'static str {
    // "Hold Right Ctrl to talk, release to transcribe." -> "Hold Right Ctrl to talk"
    hotkey::describe().split(',').next().unwrap_or("Hold Right Ctrl to talk")
}
