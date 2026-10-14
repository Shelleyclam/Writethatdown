//! The small floating pill that shows "Listening" / "Transcribing".
//!
//! It must never take keyboard focus: the paste goes to whichever window has
//! focus, and that has to stay the app the user is dictating into. So the
//! window is non-focusable, click-through, and shown with Win32's
//! SW_SHOWNOACTIVATE instead of the usual show().

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Manager, PhysicalPosition, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

const LABEL: &str = "pill";
/// Logical size of the pill window; the pill itself is drawn centred inside it.
const WIDTH: f64 = 320.0;
const HEIGHT: f64 = 56.0;
/// Gap between the pill and the bottom of the work area (above the taskbar).
const BOTTOM_MARGIN: f64 = 24.0;

/// Bumped on every show/hide, so a delayed hide doesn't hide a newer pill.
static GENERATION: AtomicU64 = AtomicU64::new(0);

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let window = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::default())
        .title("Writethatdown pill")
        .inner_size(WIDTH, HEIGHT)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focusable(false)
        .focused(false)
        .visible(false)
        .build()?;
    window.set_ignore_cursor_events(true)?;
    Ok(())
}

pub fn show(app: &AppHandle) {
    GENERATION.fetch_add(1, Ordering::SeqCst);
    let Some(window) = app.get_webview_window(LABEL) else { return };
    place(&window);
    #[cfg(windows)]
    if let Ok(hwnd) = window.hwnd() {
        use windows::Win32::UI::WindowsAndMessaging::{ShowWindowAsync, SW_SHOWNOACTIVATE};
        let _ = unsafe { ShowWindowAsync(hwnd, SW_SHOWNOACTIVATE) };
    }
}

pub fn hide(app: &AppHandle) {
    GENERATION.fetch_add(1, Ordering::SeqCst);
    if let Some(window) = app.get_webview_window(LABEL) {
        #[cfg(windows)]
        if let Ok(hwnd) = window.hwnd() {
            use windows::Win32::UI::WindowsAndMessaging::{ShowWindowAsync, SW_HIDE};
            let _ = unsafe { ShowWindowAsync(hwnd, SW_HIDE) };
        }
    }
}

/// Hide after a delay (to leave a message readable), unless the pill was
/// shown again in the meantime.
pub fn hide_after(app: &AppHandle, delay: Duration) {
    let generation = GENERATION.load(Ordering::SeqCst);
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        if GENERATION.load(Ordering::SeqCst) == generation {
            hide(&app);
        }
    });
}

/// Bottom centre of the monitor the user is working on, above the taskbar.
fn place(window: &WebviewWindow) {
    let monitor = window
        .cursor_position()
        .ok()
        .and_then(|p| window.monitor_from_point(p.x, p.y).ok().flatten())
        .or_else(|| window.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else { return };
    let area = monitor.work_area();
    let scale = monitor.scale_factor();
    let width = (WIDTH * scale) as i32;
    let height = (HEIGHT * scale) as i32;
    let x = area.position.x + (area.size.width as i32 - width) / 2;
    let y = area.position.y + area.size.height as i32 - height - (BOTTOM_MARGIN * scale) as i32;
    let _ = window.set_position(PhysicalPosition::new(x, y));
}
