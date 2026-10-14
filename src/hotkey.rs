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
//! - Esc: cancels a recording. Only reported, and only swallowed, while
//!   recording (see [`set_recording`]); otherwise apps get it as usual.
//!
//! Each release says how the key was pressed ([`Press`]), so the engine can
//! tell a quick tap (hands-free mode) from a hold (push-to-talk) and from a
//! shortcut like Right Ctrl+C (not dictation at all).

use anyhow::Result;
use std::sync::mpsc::{channel, Receiver};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// Right Ctrl: dictate into the focused app.
    Dictate,
    /// Right Alt: dictate a note.
    Note,
}

/// How a key was pressed, reported when it is released.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
    /// Released quickly with no other key pressed in between.
    Tap,
    /// Held longer than a tap, alone.
    Hold,
    /// Another key was pressed while it was down (a shortcut like Ctrl+C).
    Combo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEvent {
    Down(Key),
    Up(Key, Press),
    /// Esc while recording.
    Cancel,
}

/// A press shorter than this, with no other key, counts as a tap.
pub const TAP_MS: u32 = 300;

/// Start listening for the push-to-talk keys. With `note_key` false, Right
/// Alt is left alone entirely.
pub fn listen(note_key: bool) -> Result<Receiver<KeyEvent>> {
    let (tx, rx) = channel();
    imp::spawn(tx, note_key)?;
    Ok(rx)
}

/// Tell the hook whether a recording is in progress, so Esc is only taken
/// away from other apps while it can cancel something.
pub fn set_recording(recording: bool) {
    imp::set_recording(recording)
}

pub fn describe() -> &'static str {
    imp::DESCRIPTION
}

#[cfg(windows)]
mod imp {
    use super::{Key, KeyEvent, Press, TAP_MS};
    use anyhow::{anyhow, Result};
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
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

    pub const DESCRIPTION: &str = "Hold Right Ctrl to talk, release to transcribe. Tap it for hands-free.";

    const VK_ESCAPE: u32 = 0x1B;
    const VK_RCONTROL: u32 = 0xA3;
    const VK_RMENU: u32 = 0xA5;
    /// AltGr layouts send a fake Left Ctrl with this scan code before Right
    /// Alt; it isn't a real second key.
    const ALTGR_FAKE_CTRL_SCAN: u32 = 0x21D;

    /// Per-key press tracking.
    struct KeyState {
        down: AtomicBool,
        down_at: AtomicU32,
        combo: AtomicBool,
    }

    impl KeyState {
        const fn new() -> Self {
            Self { down: AtomicBool::new(false), down_at: AtomicU32::new(0), combo: AtomicBool::new(false) }
        }
    }

    static SENDER: OnceLock<Mutex<Sender<KeyEvent>>> = OnceLock::new();
    static NOTE_KEY: AtomicBool = AtomicBool::new(false);
    static RECORDING: AtomicBool = AtomicBool::new(false);
    static ESC_SWALLOWED: AtomicBool = AtomicBool::new(false);
    static DICTATE: KeyState = KeyState::new();
    static NOTE: KeyState = KeyState::new();

    fn send(event: KeyEvent) {
        if let Some(tx) = SENDER.get() {
            let _ = tx.lock().unwrap().send(event);
        }
    }

    fn state(key: Key) -> &'static KeyState {
        match key {
            Key::Dictate => &DICTATE,
            Key::Note => &NOTE,
        }
    }

    /// Any other key going down turns held push-to-talk keys into shortcuts.
    fn mark_combo(except: Option<Key>) {
        for key in [Key::Dictate, Key::Note] {
            let s = state(key);
            if Some(key) != except && s.down.load(Ordering::SeqCst) {
                s.combo.store(true, Ordering::SeqCst);
            }
        }
    }

    fn on_target(key: Key, is_down: bool, time: u32) {
        let s = state(key);
        if is_down {
            // Holding a key auto-repeats key-down; only report the first one.
            if !s.down.swap(true, Ordering::SeqCst) {
                s.down_at.store(time, Ordering::SeqCst);
                s.combo.store(false, Ordering::SeqCst);
                mark_combo(Some(key));
                send(KeyEvent::Down(key));
            }
        } else if s.down.swap(false, Ordering::SeqCst) {
            let held_ms = time.wrapping_sub(s.down_at.load(Ordering::SeqCst));
            let press = if s.combo.load(Ordering::SeqCst) {
                Press::Combo
            } else if held_ms < TAP_MS {
                Press::Tap
            } else {
                Press::Hold
            };
            send(KeyEvent::Up(key, press));
        }
    }

    unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code == HC_ACTION as i32 {
            // SAFETY: for WH_KEYBOARD_LL, lparam points to a KBDLLHOOKSTRUCT.
            let kb = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
            // Ignore keys sent by programs (including our own Ctrl+V).
            let injected = (kb.flags.0 & LLKHF_INJECTED.0) != 0;
            let msg = wparam.0 as u32;
            let is_down = matches!(msg, WM_KEYDOWN | WM_SYSKEYDOWN);
            let is_up = matches!(msg, WM_KEYUP | WM_SYSKEYUP);
            if !injected && (is_down || is_up) {
                match kb.vkCode {
                    VK_RCONTROL => on_target(Key::Dictate, is_down, kb.time),
                    VK_RMENU if NOTE_KEY.load(Ordering::SeqCst) => {
                        on_target(Key::Note, is_down, kb.time);
                        // Swallow Right Alt, so apps never see a lone Alt press.
                        return LRESULT(1);
                    }
                    VK_ESCAPE => {
                        if is_down && RECORDING.load(Ordering::SeqCst) {
                            ESC_SWALLOWED.store(true, Ordering::SeqCst);
                            send(KeyEvent::Cancel);
                            return LRESULT(1);
                        }
                        if is_up && ESC_SWALLOWED.swap(false, Ordering::SeqCst) {
                            return LRESULT(1);
                        }
                    }
                    _ if is_down && kb.scanCode != ALTGR_FAKE_CTRL_SCAN => mark_combo(None),
                    _ => {}
                }
            }
        }
        // Pass everything else on, so Right Ctrl keeps working normally elsewhere.
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }

    pub fn set_recording(recording: bool) {
        RECORDING.store(recording, Ordering::SeqCst);
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
    use super::{Key, KeyEvent, Press};
    use anyhow::Result;
    use std::io::BufRead;
    use std::sync::mpsc::Sender;

    pub const DESCRIPTION: &str = "Press Enter to start talking, Enter again to stop.";

    pub fn set_recording(_recording: bool) {}

    pub fn spawn(tx: Sender<KeyEvent>, _note_key: bool) -> Result<()> {
        std::thread::spawn(move || {
            let mut down = false;
            for _ in std::io::stdin().lock().lines() {
                down = !down;
                let ev = if down { KeyEvent::Down(Key::Dictate) } else { KeyEvent::Up(Key::Dictate, Press::Hold) };
                if tx.send(ev).is_err() {
                    break;
                }
            }
        });
        Ok(())
    }
}
