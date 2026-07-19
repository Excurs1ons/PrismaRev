//! Native fatal-error crash dialog.
//!
//! When the renderer hits an unrecoverable error (device lost, validation
//! fatal, swapchain cannot be recreated, ...) we want to surface it to the
//! user once and stop, instead of spamming the log every frame. This module
//! shows a **native modal dialog** with two actions:
//!
//! - **Copy & Exit** (also copies the full error text to the clipboard so the
//!   user can paste it into a bug report)
//! - **Exit**
//!
//! Each platform uses its own native dialog API so we don't pull in a heavy
//! cross-platform dialog crate:
//!
//! | Platform | Dialog | Clipboard |
//! |----------|--------|-----------|
//! | Windows  | `MessageBoxW` (`MB_YESNO`) | `OpenClipboard` / `SetClipboardData` |
//! | macOS    | `osascript` (`display dialog`) | `pbcopy` |
//! | Linux    | `zenity --question` (fallback: text on stderr) | `xclip`/`xsel` |
//! | Android  | `AlertDialog` via JNI on the UI thread | (no copy; text is in logcat) |
//!
//! The dialog blocks the calling thread (the winit event-loop / main thread)
//! until the user confirms, which naturally "suspends" the render loop. After
//! confirmation the caller tears down the event loop.
//!
//! ## Android
//!
//! There is no clipboard-with-button path that works from native code without
//! a full JNI round-trip; the error text is logged to logcat (tag `PrismaRev`)
//! and an `AlertDialog` is shown on the UI thread via JNI against the
//! `Activity` exposed by [`android_activity::AndroidApp`]. The dialog has a
//! single OK button (exit). The error text is also written to logcat so the
//! user / developer can grab it with `adb logcat`.

/// The user's choice in the crash dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrashChoice {
    /// "Copy & Exit": copy the error text to the clipboard, then exit.
    CopyAndExit,
    /// "Exit": just exit.
    Exit,
}

// ---------------------------------------------------------------------------
// Android app registration (only compiled on Android)
// ---------------------------------------------------------------------------

#[cfg(target_os = "android")]
struct AndroidAppHandles {
    vm_ptr: *mut std::ffi::c_void,
    activity_ptr: *mut std::ffi::c_void,
}

#[cfg(target_os = "android")]
// SAFETY: the handles are raw pointers into the JVM/Activity, which live for
// the entire process. They are only dereferenced from the main thread (the
// winit event loop thread), which is also the thread that registered them.
unsafe impl Send for AndroidAppHandles {}
#[cfg(target_os = "android")]
unsafe impl Sync for AndroidAppHandles {}

#[cfg(target_os = "android")]
static ANDROID_APP: std::sync::OnceLock<AndroidAppHandles> = std::sync::OnceLock::new();

/// Register the `AndroidApp` so the crash dialog can reach the JVM/Activity
/// for showing an `AlertDialog`. Called once from `android_main` before the
/// event loop starts. Safe to call multiple times (only the first wins).
///
/// No-op on non-Android platforms (the function simply isn't compiled there).
#[cfg(target_os = "android")]
pub fn register_android_app(app: &android_activity::AndroidApp) {
    let handles = AndroidAppHandles {
        vm_ptr: app.vm_as_ptr(),
        activity_ptr: app.activity_as_ptr(),
    };
    let _ = ANDROID_APP.set(handles);
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Show the native crash dialog with `title` and `message`.
///
/// Blocks the calling thread until the user confirms. Returns the chosen
/// action. If showing the dialog fails on a platform that uses a subprocess
/// (macOS/Linux) or JNI (Android), the error text is logged and `Exit` is
/// returned so the caller still terminates.
pub fn show_crash_dialog(title: &str, message: &str) -> CrashChoice {
    // Always log the full error first so it's in the log even if the dialog
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
// Platform dispatch
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

/// Copy `text` to the system clipboard. Platform-specific; on Android this is
/// a no-op (the dialog text is available via logcat instead).
fn copy_to_clipboard(text: &str) -> std::io::Result<()> {
    platform::copy_to_clipboard(text)
}
