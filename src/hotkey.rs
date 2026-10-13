//! Push-to-talk key detection.
//!
//! Normal "global shortcut" APIs only report a key press. Push-to-talk also
//! needs the release, so on Windows we install a low-level keyboard hook,
//! which sees every key-down and key-up system-wide.
//!
//! - Right Ctrl: dictate and paste. Passed on to apps as usual (a lone Ctrl
//!   press does nothing in them).
//! - Right Alt: dictate a note. Swallowed, because releasing a lone Alt key
//!   opens the menu bar in many apps. While the note key is enabled, Right Alt
//!   therefore doesn't reach other apps (so it can't be used as AltGr).

use anyhow::Result;
use std::sync::mpsc::{channel, Receiver};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// Right Ctrl: dictate into the focused app.
    Dictate,
    /// Right Alt: dictate a note.
    Note,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEvent {
    Down(Key),
    Up(Key),
}

/// Start listening for the push-to-talk keys. With `note_key` false, Right
/// Alt is left alone entirely.
pub fn listen(note_key: bool) -> Result<Receiver<KeyEvent>> {
    let (tx, rx) = channel();
    imp::spawn(tx, note_key)?;
    Ok(rx)
}

pub fn describe() -> &'static str {
    imp::DESCRIPTION
}

#[cfg(windows)]
mod imp {
    use super::{Key, KeyEvent};
    use anyhow::{anyhow, Result};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::Sender;
    use std::sync::{Mutex, OnceLock};
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetMessageW, SetWindowsHookExW, TranslateMessage,
        HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP,
        WM_SYSKEYDOWN, WM_SYSKEYUP,
    };

    pub const DESCRIPTION: &str = "Hold Right Ctrl to talk, release to transcribe.";

    const VK_RCONTROL: u32 = 0xA3;
    const VK_RMENU: u32 = 0xA5;

    static SENDER: OnceLock<Mutex<Sender<KeyEvent>>> = OnceLock::new();
    static NOTE_KEY: AtomicBool = AtomicBool::new(false);
    static DICTATE_DOWN: AtomicBool = AtomicBool::new(false);
    static NOTE_DOWN: AtomicBool = AtomicBool::new(false);

    unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code == HC_ACTION as i32 {
            // SAFETY: for WH_KEYBOARD_LL, lparam points to a KBDLLHOOKSTRUCT.
            let kb = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
            // Ignore keys sent by programs (including our own Ctrl+V).
            let injected = (kb.flags.0 & LLKHF_INJECTED.0) != 0;
            let target = match kb.vkCode {
                VK_RCONTROL => Some((Key::Dictate, &DICTATE_DOWN)),
                VK_RMENU if NOTE_KEY.load(Ordering::SeqCst) => Some((Key::Note, &NOTE_DOWN)),
                _ => None,
            };
            if let (Some((key, is_down)), false) = (target, injected) {
                // Holding a key auto-repeats key-down; only report the first one.
                let event = match wparam.0 as u32 {
                    WM_KEYDOWN | WM_SYSKEYDOWN if !is_down.swap(true, Ordering::SeqCst) => {
                        Some(KeyEvent::Down(key))
                    }
                    WM_KEYUP | WM_SYSKEYUP if is_down.swap(false, Ordering::SeqCst) => Some(KeyEvent::Up(key)),
                    _ => None,
                };
                if let (Some(ev), Some(tx)) = (event, SENDER.get()) {
                    let _ = tx.lock().unwrap().send(ev);
                }
                if key == Key::Note {
                    // Swallow Right Alt, so apps never see a lone Alt press.
                    return LRESULT(1);
                }
            }
        }
        // Pass everything else on, so Right Ctrl keeps working normally elsewhere.
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }

    pub fn spawn(tx: Sender<KeyEvent>, note_key: bool) -> Result<()> {
        SENDER
            .set(Mutex::new(tx))
            .map_err(|_| anyhow!("Hotkey listener already started"))?;
        NOTE_KEY.store(note_key, Ordering::SeqCst);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();

        // The hook only fires while its thread pumps window messages.
        std::thread::spawn(move || unsafe {
            let module = GetModuleHandleW(PCWSTR::null()).ok().map(Into::into);
            match SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), module, 0) {
                Ok(_hook) => {
                    let _ = ready_tx.send(Ok(()));
                    let mut msg = MSG::default();
                    while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e.to_string()));
                }
            }
        });

        ready_rx
            .recv()
            .map_err(|_| anyhow!("Hotkey thread exited"))?
            .map_err(|e| anyhow!("Could not install keyboard hook: {e}"))
    }
}

/// Fallback for non-Windows machines (used for development only):
/// press Enter to start recording and Enter again to stop.
#[cfg(not(windows))]
mod imp {
    use super::{Key, KeyEvent};
    use anyhow::Result;
    use std::io::BufRead;
    use std::sync::mpsc::Sender;

    pub const DESCRIPTION: &str = "Press Enter to start talking, Enter again to stop.";

    pub fn spawn(tx: Sender<KeyEvent>, _note_key: bool) -> Result<()> {
        std::thread::spawn(move || {
            let mut down = false;
            for _ in std::io::stdin().lock().lines() {
                down = !down;
                let ev = if down { KeyEvent::Down(Key::Dictate) } else { KeyEvent::Up(Key::Dictate) };
                if tx.send(ev).is_err() {
                    break;
                }
            }
        });
        Ok(())
    }
}
