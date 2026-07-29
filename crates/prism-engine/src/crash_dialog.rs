//! Native fatal-error 崩溃 对话框
//!
//! When the 渲染器 hits an unrecoverable 错误 设备 lost, 验证
//! fatal, 交换链 cannot be recreated, ...) we want to 表面 it to the
//! user once and stop, instead of spamming the 对数 every 帧 This 模块
//! shows a **native modal dialog** with two actions:
//!
//! - **Copy & Exit** (also copies the 完整 错误 text to the clipboard so the
//!   user can paste it into a bug report)
//! - **Exit**
//!
//! Each platform uses its own native 对话框 API so we don't pull in a heavy
//! cross-platform 对话框 crate:
//!
//! | Platform | 对话框 | Clipboard |
//! |----------|--------|-----------|
//! | Windows | `MessageBoxW` (`MB_YESNO`) | `OpenClipboard` / `SetClipboardData` |
//! | macOS | `osascript` (`display 对话框 | `pbcopy` |
//! | Linux | `zenity --question` 回退 text on stderr) | `xclip`/`xsel` |
//! | Android | `AlertDialog` via JNI on the UI 线程 | (no 复制 text is in logcat) |
//!
//! The 对话框 blocks the calling 线程 (the winit event-loop / main 线程
//! until the user confirms, which naturally "suspends" the 渲染 循环 After
//! confirmation the 调用者 tears 下 the 事件 循环
//!
//! ## Android
//!
//! There is no clipboard-with-button path that works from native 代码 without
//! a 完整 JNI round-trip; the 错误 text is logged to logcat (tag `PrismaRev`)
//! and an `AlertDialog` is shown on the UI 线程 via JNI against the
//! `Activity` exposed by [`android_activity::AndroidApp`]. The 对话框 has a
//! single OK 按钮 (exit). The 错误 text is also written to logcat so the
//! user / developer can grab it with `adb logcat`.

/// The user's choice in the 崩溃 对话框
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrashChoice {
    /// 复制 & Exit": 复制 the 错误 text to the clipboard, then exit.
    CopyAndExit,
    /// "Exit": just exit.
    Exit,
}

// ---------------------------------------------------------------------------
// Android app registration (only compiled on Android
// ---------------------------------------------------------------------------

#[cfg(target_os = "android")]
struct AndroidAppHandles {
    vm_ptr: *mut std::ffi::c_void,
    activity_ptr: *mut std::ffi::c_void,
}

#[cfg(target_os = "android")]
// 安全性 the handles are raw pointers into the JVM/Activity, which live for
// the entire 进程 They are only dereferenced from the main 线程 (the
// winit 事件 循环 线程 which is also the 线程 that registered them.
unsafe impl Send for AndroidAppHandles {}
#[cfg(target_os = "android")]
unsafe impl Sync for AndroidAppHandles {}

#[cfg(target_os = "android")]
static ANDROID_APP: std::sync::OnceLock<AndroidAppHandles> = std::sync::OnceLock::new();

/// Register the `AndroidApp` so the 崩溃 对话框 can reach the JVM/Activity
/// for showing an `AlertDialog`. Called once from `android_main` before the
/// 事件 循环 starts. Safe to 调用 multiple times (only the 第一个 wins).
///
/// No-op on non-Android platforms (the 函数 simply isn't compiled there).
#[cfg(target_os = "android")]
pub fn register_android_app(app: &android_activity::AndroidApp) {
    let handles = AndroidAppHandles {
        vm_ptr: app.vm_as_ptr(),
        activity_ptr: app.activity_as_ptr(),
    };
    let _ = ANDROID_APP.set(handles);
}

// ---------------------------------------------------------------------------
// 公开 entry point
// ---------------------------------------------------------------------------

/// Show the native 崩溃 对话框 with `title` and 消息
///
/// Blocks the calling 线程 until the user confirms. Returns the chosen
/// action. If showing the 对话框 fails on a platform that uses a subprocess
/// (macOS/Linux) or JNI Android the 错误 text is logged and `Exit` is
/// returned so the 调用者 still terminates.
pub fn show_crash_dialog(title: &str, message: &str) -> CrashChoice {
    // Always 对数 the 完整 错误 第一个 so it's in the 对数 even if the 对话框
    // backend fails (e.g. zenity not installed).
    log::error!("FATAL: {title}\n{message}");

    let choice = show_native(title, message);
    if matches!(choice, CrashChoice::CopyAndExit) {
        if let Err(e) = copy_to_clipboard(message) {
            log::warn!("failed to copy crash text to clipboard: {e}");
        }
    }
    choice
}

// ---------------------------------------------------------------------------
// Platform 分发
// ---------------------------------------------------------------------------

#[cfg(windows)]
#[path = "crash_dialog_windows.rs"]
mod platform;

#[cfg(target_os = "macos")]
#[path = "crash_dialog_macos.rs"]
mod platform;

#[cfg(all(unix, not(target_os = "android"), not(target_os = "macos")))]
#[path = "crash_dialog_linux.rs"]
mod platform;

#[cfg(target_os = "android")]
#[path = "crash_dialog_android.rs"]
mod platform;

#[cfg(not(any(
    windows,
    target_os = "macos",
    target_os = "android",
    all(unix, not(target_os = "android"), not(target_os = "macos"))
)))]
#[path = "crash_dialog_fallback.rs"]
mod platform;

fn show_native(title: &str, message: &str) -> CrashChoice {
    platform::show(title, message)
}

/// 复制 `text` to the 系统 clipboard. Platform-specific; on Android this is
/// a no-op (the 对话框 text is available via logcat instead).
fn copy_to_clipboard(text: &str) -> std::io::Result<()> {
    platform::copy_to_clipboard(text)
}
