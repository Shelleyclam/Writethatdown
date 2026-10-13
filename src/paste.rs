//! Paste text into whatever app has focus.
//!
//! Windows has no "type this string into the focused app" API that works
//! everywhere, so we do what a person would: put the text on the clipboard and
//! press Ctrl+V. The user's clipboard is saved first and restored afterwards,
//! so dictating never loses what they had copied.

use anyhow::Result;

/// Paste `text` into the focused app, then restore the previous clipboard.
pub fn paste(text: &str) -> Result<()> {
    imp::paste(text)
}

/// File name of the app that currently has focus (e.g. "notepad.exe"), if known.
pub fn foreground_app() -> Option<String> {
    imp::foreground_app()
}

#[cfg(windows)]
mod imp {
    use anyhow::{anyhow, bail, Context, Result};
    use std::time::Duration;
    use windows::core::{w, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, GlobalFree, HANDLE, HGLOBAL};
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData, OpenClipboard,
        RegisterClipboardFormatW, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
        VIRTUAL_KEY, VK_CONTROL, VK_V,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

    const CF_UNICODETEXT: u32 = 13;

    /// The target app reads the clipboard when it handles Ctrl+V, which happens
    /// some time after we send the keys. Restoring too early would paste the
    /// old clipboard instead, so wait this long first.
    const RESTORE_DELAY: Duration = Duration::from_millis(300);

    pub fn paste(text: &str) -> Result<()> {
        let saved = {
            let _cb = Clipboard::open()?;
            let saved = save_formats();
            set_text(text)?;
            saved
        };

        press_ctrl_v()?;
        std::thread::sleep(RESTORE_DELAY);

        let _cb = Clipboard::open()?;
        restore_formats(&saved)
    }

    /// Holds the clipboard open; closes it when dropped.
    struct Clipboard;

    impl Clipboard {
        fn open() -> Result<Self> {
            // Another app may hold the clipboard for a moment; retry briefly.
            for _ in 0..20 {
                if unsafe { OpenClipboard(None) }.is_ok() {
                    return Ok(Clipboard);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            bail!("Could not open the clipboard (another app is using it)")
        }
    }

    impl Drop for Clipboard {
        fn drop(&mut self) {
            let _ = unsafe { CloseClipboard() };
        }
    }

    /// Formats whose data is a GDI handle or app-owned, not a memory block we
    /// can copy. Windows re-creates the common ones (bitmaps from CF_DIB) itself.
    fn is_copyable(format: u32) -> bool {
        const CF_BITMAP: u32 = 2;
        const CF_METAFILEPICT: u32 = 3;
        const CF_PALETTE: u32 = 9;
        const CF_ENHMETAFILE: u32 = 14;
        !matches!(format, CF_BITMAP | CF_METAFILEPICT | CF_PALETTE | CF_ENHMETAFILE)
            && !(0x80..=0x8F).contains(&format) // display / owner-display formats
            && !(0x200..=0x3FF).contains(&format) // private and GDI object ranges
    }

    /// Copy every memory-backed format currently on the clipboard.
    /// Must be called with the clipboard open.
    fn save_formats() -> Vec<(u32, Vec<u8>)> {
        let mut saved = Vec::new();
        let mut format = 0;
        loop {
            format = unsafe { EnumClipboardFormats(format) };
            if format == 0 {
                break;
            }
            if !is_copyable(format) {
                continue;
            }
            let Ok(handle) = (unsafe { GetClipboardData(format) }) else { continue };
            let mem = HGLOBAL(handle.0);
            unsafe {
                let ptr = GlobalLock(mem) as *const u8;
                if ptr.is_null() {
                    continue;
                }
                let bytes = std::slice::from_raw_parts(ptr, GlobalSize(mem)).to_vec();
                let _ = GlobalUnlock(mem);
                saved.push((format, bytes));
            }
        }
        saved
    }

    /// Put the original formats back. Must be called with the clipboard open.
    fn restore_formats(saved: &[(u32, Vec<u8>)]) -> Result<()> {
        unsafe { EmptyClipboard() }.context("Could not clear the clipboard")?;
        for (format, bytes) in saved {
            // A format that won't restore shouldn't stop the rest.
            let _ = set_bytes(*format, bytes);
        }
        exclude_from_history();
        Ok(())
    }

    /// Replace the clipboard contents with `text`. Must be called with the clipboard open.
    fn set_text(text: &str) -> Result<()> {
        unsafe { EmptyClipboard() }.context("Could not clear the clipboard")?;
        let utf16: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes: Vec<u8> = utf16.iter().flat_map(|c| c.to_le_bytes()).collect();
        set_bytes(CF_UNICODETEXT, &bytes)?;
        exclude_from_history();
        Ok(())
    }

    fn set_bytes(format: u32, bytes: &[u8]) -> Result<()> {
        unsafe {
            let mem = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1))?;
            let ptr = GlobalLock(mem) as *mut u8;
            if ptr.is_null() {
                let _ = GlobalFree(Some(mem));
                bail!("Could not lock clipboard memory");
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
            let _ = GlobalUnlock(mem);
            // On success the clipboard owns the memory; on failure we still do.
            if let Err(e) = SetClipboardData(format, Some(HANDLE(mem.0))) {
                let _ = GlobalFree(Some(mem));
                return Err(anyhow!("Could not set clipboard data: {e}"));
            }
        }
        Ok(())
    }

    /// Keep dictated text (and our restore of the old clipboard) out of
    /// Windows clipboard history (Win+V) and cloud clipboard sync.
    fn exclude_from_history() {
        unsafe {
            let format = RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing"));
            if format != 0 {
                let _ = set_bytes(format, &[0]);
            }
        }
    }

    fn press_ctrl_v() -> Result<()> {
        fn key(vk: VIRTUAL_KEY, flags: KEYBD_EVENT_FLAGS) -> INPUT {
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 },
                },
            }
        }
        let none = KEYBD_EVENT_FLAGS(0);
        let inputs = [
            key(VK_CONTROL, none),
            key(VK_V, none),
            key(VK_V, KEYEVENTF_KEYUP),
            key(VK_CONTROL, KEYEVENTF_KEYUP),
        ];
        let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
        if sent as usize != inputs.len() {
            // Windows blocks input into apps running as administrator
            // unless we run as administrator too.
            bail!("Windows blocked the paste keystroke (is the target app running as administrator?)");
        }
        Ok(())
    }

    pub fn foreground_app() -> Option<String> {
        unsafe {
            let hwnd = GetForegroundWindow();
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == 0 {
                return None;
            }
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
            let _ = CloseHandle(process);
            ok.ok()?;
            let path = String::from_utf16_lossy(&buf[..len as usize]);
            path.rsplit('\\').next().map(str::to_string)
        }
    }
}

/// Non-Windows machines (development only) just print the text.
#[cfg(not(windows))]
mod imp {
    use anyhow::Result;

    pub fn paste(_text: &str) -> Result<()> {
        println!("  (pasting is only supported on Windows)");
        Ok(())
    }

    pub fn foreground_app() -> Option<String> {
        None
    }
}
