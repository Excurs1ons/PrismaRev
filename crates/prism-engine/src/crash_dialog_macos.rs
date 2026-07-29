//! macOS 崩溃 对话框 `osascript` (AppleScript `display 对话框 + `pbcopy`.
//!
//! Invokes `/usr/bin/osascript` with a `display 对话框 ... buttons 复制 & Exit", "Exit"}`
//! script. `display 对话框 is modal and blocks until the user picks a 按钮
//! `pbcopy` fills the pasteboard with the 错误 text.

use std::io::Write;
use std::process::Command;

use super::CrashChoice;

pub fn show(title: &str, message: &str) -> CrashChoice {
    // Escape double-quotes and backslashes for the AppleScript 字符串 literal.
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let title_e = esc(title);
    // 替换 literal newlines with AppleScript newline concatenation so the
    // 对话框 shows real line breaks instead of a single run-on line.
    let msg_e = esc(message).replace('\n', "\" & return & \"");

    // 复制 & Exit" is the 默认 第一个 按钮 `giving 上 after 0` means
    // "no 超时 here is not used; we let it 块
    let script = format!(
        "display dialog \"{msg_e}\" with title \"{title_e}\" \
         buttons {{\"Copy & Exit\", \"Exit\"}} default button \"Copy & Exit\" \
         with icon stop"
    );

    match run_osascript(&script) {
        Ok(output) => {
            // `osascript` prints 按钮 returned:Copy & Exit` to stdout.
            if output.contains("Copy & Exit") {
                CrashChoice::CopyAndExit
            } else {
                CrashChoice::Exit
            }
        }
        Err(e) => {
            log::warn!("osascript dialog failed ({e}); falling back to plain exit");
            CrashChoice::Exit
        }
    }
}

fn run_osascript(script: &str) -> std::io::Result<String> {
    let out = Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(script)
        .output()?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn copy_to_clipboard(text: &str) -> std::io::Result<()> {
    let mut child = Command::new("/usr/bin/pbcopy")
        .stdin(std::process::Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes())?;
    }
    let status = child.wait()?;
    if !status.success() {
        return Err(std::io::Error::other("pbcopy exited non-zero"));
    }
    Ok(())
}
