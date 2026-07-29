//! Linux 崩溃 对话框 `zenity --question` + `xclip`/`xsel`.
//!
//! `zenity --question` shows a modal Yes/No 对话框 exit 代码 0 = Yes, 1 = No.
//! If `zenity` isn't installed we fall 后 to printing the 错误 to stderr
//! (the user still sees it in the terminal). Clipboard 复制 tries `xclip`
//! 第一个 then `xsel`; if neither is available it's a no-op (the text is
//! already on stderr / the 对话框

use std::io::Write;
use std::process::Command;

use super::CrashChoice;

pub fn show(title: &str, message: &str) -> CrashChoice {
    // zenity --question: exit 0 = Yes 复制 & Exit), 1 = No (Exit).
    let body = format!(
        "{message}\n\n\
         Yes = copy this text to the clipboard, then exit\n\
         No  = exit without copying"
    );
    match Command::new("zenity")
        .arg("--question")
        .arg("--title")
        .arg(title)
        .arg("--text")
        .arg(&body)
        .arg("--icon")
        .arg("error")
        .status()
    {
        Ok(status) => {
            if status.success() {
                CrashChoice::CopyAndExit
            } else {
                CrashChoice::Exit
            }
        }
        Err(e) => {
            log::warn!("zenity dialog failed ({e}); printing to stderr");
            eprintln!("\n--- {title} ---\n{message}\n");
            CrashChoice::Exit
        }
    }
}

pub fn copy_to_clipboard(text: &str) -> std::io::Result<()> {
    // Try xclip 第一个 fall 后 to xsel.
    if try_clip("xclip", &["-selection", "clipboard"], text).is_ok() {
        return Ok(());
    }
    if try_clip("xsel", &["--clipboard", "--input"], text).is_ok() {
        return Ok(());
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "neither xclip nor xsel is available",
    ))
}

fn try_clip(bin: &str, args: &[&str], text: &str) -> std::io::Result<()> {
    let mut child = Command::new(bin)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes())?;
    }
    let status = child.wait()?;
    if !status.success() {
        return Err(std::io::Error::other(format!("{bin} exited non-zero")));
    }
    Ok(())
}
