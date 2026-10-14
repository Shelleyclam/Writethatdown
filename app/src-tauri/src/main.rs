//! Writethatdown desktop app (crate `whisperflow-app`): tray icon, floating "listening" pill, and the
//! main window (history, notes, ask), around the dictation engine from the
//! core crate.

// No console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod pill;

use serde::Serialize;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_autostart::ManagerExt;
use whisperflow::engine::{self, Event, Mode};
use whisperflow::{cleanup, storage};

/// Passed by the "start at login" entry, so the app starts quietly in the tray.
const AUTOSTART_ARG: &str = "--minimized";
/// Name of the "start at login" registry entry. Fixed (it predates the
/// Writethatdown name) so existing entries keep working.
const AUTOSTART_NAME: &str = "whisperflow";
const APP_NAME: &str = "Writethatdown";
const MAIN_WINDOW: &str = "main";

/// What the main window shows in its status line.
#[derive(Clone, Serialize)]
struct Status {
    /// "loading", "ready" or "error".
    state: &'static str,
    detail: String,
}

pub struct AppState {
    db_path: PathBuf,
    status: Mutex<Status>,
}

#[tauri::command]
fn get_status(state: tauri::State<AppState>) -> Status {
    state.status.lock().unwrap().clone()
}

fn main() {
    tauri::Builder::default()
        // A second copy would install a second keyboard hook and paste twice.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)))
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .app_name(AUTOSTART_NAME)
                .args([AUTOSTART_ARG])
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            get_status,
            commands::get_history,
            commands::delete_entry,
            commands::promote_to_note,
            commands::get_notes,
            commands::get_categories,
            commands::update_note,
            commands::set_note_category,
            commands::delete_note,
            commands::rename_category,
            commands::merge_categories,
            commands::ask,
        ])
        .setup(|app| {
            // Same database the command-line tool uses.
            let db_path = storage::default_path();
            app.manage(AppState {
                db_path: db_path.clone(),
                status: Mutex::new(Status { state: "loading", detail: "Loading models…".into() }),
            });

            pill::create(app.handle())?;
            let started_at_login = std::env::args().any(|a| a == AUTOSTART_ARG);
            WebviewWindowBuilder::new(app, MAIN_WINDOW, WebviewUrl::default())
                .title(APP_NAME)
                .inner_size(780.0, 720.0)
                .min_inner_size(520.0, 420.0)
                .visible(!started_at_login)
                .build()?;
            build_tray(app.handle())?;
            start_engine(app.handle().clone(), db_path);
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the main window only hides it; the app lives in the tray.
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == MAIN_WINDOW {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("failed to start Writethatdown")
        .run(|_app, event| {
            // Keep running with no windows open; only "Quit" (which passes
            // an exit code) ends the app.
            if let RunEvent::ExitRequested { api, code: None, .. } = event {
                api.prevent_exit();
            }
        });
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(MAIN_WINDOW) {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", &format!("Open {APP_NAME}"), true, None::<&str>)?;
    let autostart_on = app.autolaunch().is_enabled().unwrap_or(false);
    let autostart = CheckMenuItem::with_id(app, "autostart", "Start at login", true, autostart_on, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", &format!("Quit {APP_NAME}"), true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[&open, &PredefinedMenuItem::separator(app)?, &autostart, &PredefinedMenuItem::separator(app)?, &quit],
    )?;

    TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().cloned().expect("app icon"))
        .tooltip(format!("{APP_NAME}: loading"))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "open" => show_main(app),
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
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

fn set_status(app: &AppHandle, state: &'static str, detail: String) {
    let status = Status { state, detail };
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(format!("{APP_NAME}: {}", status.detail)));
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
            db: Some(db_path),
            notes: true,
        };
        let handle = app.clone();
        let result = engine::run(config, move |event| on_engine_event(&handle, event));
        if let Err(e) = result {
            set_status(&app, "error", format!("{e:#}"));
            pill::hide(&app);
            show_main(&app);
        }
    });
}

fn on_engine_event(app: &AppHandle, event: Event) {
    // The pill and main windows render from the same events. Send first,
    // so the pill already shows the new state when it appears.
    let _ = app.emit("engine", event.clone());
    match &event {
        Event::Loading => set_status(app, "loading", "Loading models…".into()),
        Event::Ready { model, llm, notes, .. } => {
            let keys = if *notes { "Right Ctrl to dictate, Right Alt for a note" } else { "Right Ctrl to dictate" };
            let cleanup = llm.as_deref().unwrap_or("no cleanup");
            set_status(app, "ready", format!("Ready. Hold {keys}. {model}, {cleanup}"));
        }
        Event::Listening { .. } => pill::show(app),
        Event::Done(d) => match d.mode {
            Mode::Paste => {
                let _ = app.emit("history-changed", ());
                if d.pasted {
                    pill::hide(app);
                } else {
                    pill::hide_after(app, Duration::from_millis(2500));
                }
            }
            Mode::Note => {
                let _ = app.emit("notes-changed", ());
                // Leave "Saved to <category>" up long enough to read.
                pill::hide_after(app, Duration::from_millis(1800));
            }
        },
        Event::NotesChanged => {
            let _ = app.emit("notes-changed", ());
        }
        Event::Ignored { .. } => pill::hide(app),
        Event::Transcribing { .. } | Event::CleaningUp { .. } | Event::Filing | Event::Warning { .. } => {}
    }
}
