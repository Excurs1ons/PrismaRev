//! Windows 崩溃 对话框 `MessageBoxW` with Yes/No buttons + Win32 clipboard.
//!
//! - "Yes" -> `CopyAndExit` (clipboard filled with the 错误 text)
//! - "No"  -> `Exit`
//!
//! `MessageBoxW` is modal and blocks the calling 线程 which is exactly the
//! "suspend main 线程 behavior we want. `MB_TOPMOST | MB_SETFOREGROUND`
//! keep the 对话框 可见 even when the (now-crashed) 渲染 窗口 is
//! unresponsive.

use std::ffi::OsStr;
use std::iter;
use std::os::windows::ffi::OsStrExt;

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows_sys::Win32::System::Ole::CF_UNICODETEXT;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, IDYES, MB_DEFBUTTON2, MB_ICONERROR, MB_SETFOREGROUND, MB_TOPMOST, MB_YESNO,
};

use super::CrashChoice;

/// 编码 a &str as a NUL-terminated UTF-16 向量 suitable for Win32 APIs.
fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(iter::once(0)).collect()
}

pub fn show(title: &str, message: &str) -> CrashChoice {
    let title_w = to_wide(title);

    // 构建 a combined body: the 消息 + a hint about the buttons.
    let body = format!(
        "{message}\n\n\
         \u{2014}\u{2014}\u{2014}\n\
         Yes = copy this text to the clipboard, then exit\n\
         No  = exit without copying"
    );
    let body_w = to_wide(&body);

    let flags = MB_YESNO | MB_ICONERROR | MB_TOPMOST | MB_SETFOREGROUND | MB_DEFBUTTON2;
    // HWND 所有者 = null (no parent; the 渲染 窗口 may be in a bad 状态
    let ret = unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            body_w.as_ptr(),
            title_w.as_ptr(),
            flags,
        )
    };

    match ret {
        IDYES => CrashChoice::CopyAndExit,
        // IDNO or any error/cancel path falls through to plain Exit.
        _ => CrashChoice::Exit,
    }
}

pub fn copy_to_clipboard(text: &str) -> std::io::Result<()> {
    // Win32 clipboard wants a NUL-terminated UTF-16 缓冲区 in a 全局 alloc.
    // We go through the Rust allocator + raw HGLOBAL via `GlobalAlloc` is the
    // textbook approach, but `SetClipboardData` can also receive a HANDLE we
    // allocate ourselves. Use the 标准 `GlobalAlloc` 流程
    use windows_sys::Win32::System::Memory::{
        GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE,
    };

    let mut wide: Vec<u16> = OsStr::new(text).encode_wide().collect();
    wide.push(0); // NUL terminator
    let byte_len = wide.len() * 2;

    unsafe {
        let hmem = GlobalAlloc(GMEM_MOVEABLE, byte_len);
        if hmem.is_null() {
            return Err(std::io::Error::other("GlobalAlloc failed"));
        }
        let ptr = GlobalLock(hmem) as *mut u16;
        if ptr.is_null() {
            return Err(std::io::Error::other("GlobalLock failed"));
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
        GlobalUnlock(hmem);

        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return Err(std::io::Error::other("OpenClipboard failed"));
        }
        EmptyClipboard();
        let handle: HANDLE = SetClipboardData(CF_UNICODETEXT as u32, hmem as HANDLE);
        let close_ok = CloseClipboard() != 0;
        if handle.is_null() {
            return Err(std::io::Error::other("SetClipboardData failed"));
        }
        if !close_ok {
            return Err(std::io::Error::other("CloseClipboard failed"));
        }
    }
    Ok(())
}
