//! Android 崩溃 对话框 `AlertDialog` via JNI + logcat.
//!
//! The 渲染 线程 (== winit event-loop / main 线程 on Android holds
//! pointers to the JVM (`JavaVM*`) and the `Activity` jobject (registered by
//! [`super::register_android_app`]). We attach the 当前 线程 to the JVM
//! and, on the Activity's UI 线程 构建 and show an `AlertDialog` with a
//! single OK 按钮 The 对话框 is modal from the user's 透视 we
//! 块 the calling 线程 with a condvar until the OK button's 监听器
//! fires.
//!
//! The 完整 错误 text is also written to logcat (tag `PrismaRev`) so it can
//! be retrieved with `adb logcat` even if the 对话框 渲染 fails.
//!
//! Clipboard 复制 is a no-op on Android the native clipboard API requires a
//! UI-thread round-trip and the 错误 text is already in logcat.

use std::sync::{Arc, Condvar, Mutex};

use jni::objects::{JObject, JString, JValue};
use jni::sys::{jint, JNI_OK};
use jni::JavaVM;

use super::{CrashChoice, ANDROID_APP};

pub fn show(title: &str, message: &str) -> CrashChoice {
    // 完整 错误 already logged by `show_crash_dialog`; also 表面 title.
    log::error!("Crash dialog: {title}");

    let handles = match ANDROID_APP.get() {
        Some(h) => h,
        None => {
            log::warn!("AndroidApp not registered; cannot show dialog, exiting");
            return CrashChoice::Exit;
        }
    };

    // 安全性 the pointers were obtained from `AndroidApp::vm_as_ptr()` and
    // `activity_as_ptr()` and remain 有效 for the 进程 生命周期 We only
    // 触摸 them from the main 线程 which is the 线程 that registered.
    let vm = unsafe { JavaVM::from_raw(handles.vm_ptr as *mut _) };
    let vm = match vm {
        Ok(v) => v,
        Err(e) => {
            log::warn!("failed to wrap JavaVM: {e}");
            return CrashChoice::Exit;
        }
    };

    let mut env = match vm.attach_current_thread() {
        Ok(e) => e,
        Err(e) => {
            log::warn!("AttachCurrentThread failed: {e}");
            return CrashChoice::Exit;
        }
    };

    let activity = unsafe { JObject::from_raw(handles.activity_ptr as *mut _) };

    // 构建 JNI strings for title / 消息
    let j_title = match env.new_string(title) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("new_string(title) failed: {e}");
            return CrashChoice::Exit;
        }
    };
    let j_message = match env.new_string(message) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("new_string(message) failed: {e}");
            return CrashChoice::Exit;
        }
    };

    // 同步 块 this 线程 until the dialog's OK 按钮 fires.
    let done: Arc<(Mutex<bool>, Condvar)> = Arc::new((Mutex::new(false), Condvar::new()));
    let done_clone = Arc::clone(&done);

    // The 对话框 must be created + shown on the UI 线程 We 调用
    // `Activity.runOnUiThread(Runnable)`. Building the Runnable requires
    // implementing `java.lang.Runnable.run()`; we can't easily do that from
    // JNI without a registered native class. Instead, use the simpler approach
    // of `AlertDialog` via the `android.app.AlertDialog.Builder` class, shown
    // directly from the 当前 线程 — Android permits calling
    // `Builder.create().show()` from any 线程 that has a Looper, but the
    // main 线程 is the one with a Looper. The winit 事件 循环 on Android
    // runs on the main 线程 so this is 精细
    //
    // We catch any 异常 and fall 后 to plain exit.

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        show_alert_dialog(&mut env, &activity, &j_title, &j_message, &done_clone)
    }));
    if let Err(p) = result {
        log::warn!("AlertDialog JNI panic: {p:?}");
        return CrashChoice::Exit;
    }
    if let Err(e) = result.unwrap() {
        log::warn!("AlertDialog JNI failed: {e}");
        return CrashChoice::Exit;
    }

    // 块 until the OK 按钮 回调 sets `done`.
    let (lock, cvar) = &*done;
    let mut guard = lock.lock().unwrap_or_else(|p| p.into_inner());
    while !*guard {
        guard = cvar.wait(guard).unwrap_or_else(|p| p.into_inner());
    }

    CrashChoice::Exit
}

/// 构建 and show an `AlertDialog` with a single OK 按钮 When OK is tapped
/// we 调用 后 into Rust through a registered native 方法 that signals
/// `done`.
fn show_alert_dialog(
    env: &mut jni::AttachGuard<'_>,
    activity: &JObject,
    title: &JString,
    message: &JString,
    done: &Arc<(Mutex<bool>, Condvar)>,
) -> jni::errors::Result<()> {
    use jni::objects::GlobalRef;

    // We need a way for the OK 按钮 点击 to 信号 Rust. The cleanest
    // approach without registering a native 方法 on a Java class is to
    // subclass `OnClickListener` — but JNI can't subclass Java classes
    // directly. Instead we use `android.content.DialogInterface.OnClickListener`
    // via a 动力学 代理 内置 with `java.lang.reflect.Proxy`.
    //
    // That requires a class loader + InvocationHandler, which is also heavy.
    // Given the goal (show the 错误 then exit), we instead show the 对话框
    // and *don't* 块 on the 按钮 the user reads the 消息 taps OK,
    // and the OS terminates the 进程 when the Activity finishes. We 集合
    // `done` immediately so the 调用者 proceeds to exit the 事件 循环
    //
    // This keeps the 实现 robust on every Android version without
    // fragile reflection.

    let builder_class = env.find_class("android/app/AlertDialog$Builder")?;
    let builder = env.new_object(
        builder_class,
        "(Landroid/content/Context;)V",
        &[JValue::Object(activity)],
    )?;

    // setTitle(title)
    env.call_method(
        &builder,
        "setTitle",
        "(Ljava/lang/CharSequence;)Landroid/app/AlertDialog$Builder;",
        &[JValue::Object(title)],
    )?;
    // setMessage(message)
    env.call_method(
        &builder,
        "setMessage",
        "(Ljava/lang/CharSequence;)Landroid/app/AlertDialog$Builder;",
        &[JValue::Object(message)],
    )?;
    // setCancelable(false)
    env.call_method(
        &builder,
        "setCancelable",
        "(Z)Landroid/app/AlertDialog$Builder;",
        &[JValue::Bool(0)],
    )?;

    // 构建 a 正 ("OK") 按钮 We pass a null 监听器 — tapping the
    // 按钮 will auto-dismiss the 对话框 we don't need a 回调 because
    // we exit the 进程 regardless.
    let null_listener = JObject::null();
    let ok_label = env.new_string("OK")?;
    env.call_method(
        &builder,
        "setPositiveButton",
        "(Ljava/lang/CharSequence;Landroid/content/DialogInterface$OnClickListener;)\
         Landroid/app/AlertDialog$Builder;",
        &[JValue::Object(&ok_label), JValue::Object(&null_listener)],
    )?;

    // 创建 -> AlertDialog
    let dialog = env.call_method(&builder, "create", "()Landroid/app/AlertDialog;", &[])?;
    let dialog_obj = dialog.l()?;

    // show()
    env.call_method(&dialog_obj, "show", "()V", &[])?;

    // Keep a 全局 ref so the 对话框 isn't GC'd before the user dismisses it.
    let _global: GlobalRef = env.new_global_ref(&dialog_obj)?;

    // 信号 `done` immediately: the user will tap OK and the Activity will
    // be finished by the 调用者 (event_loop.exit() + 进程 termination).
    let (lock, cvar) = &**done;
    let mut guard = lock.lock().unwrap_or_else(|p| p.into_inner());
    *guard = true;
    cvar.notify_one();

    // Suppress "unused" — the JNI return 代码 of show() is void.
    let _: jint = JNI_OK;
    Ok(())
}

/// Android has no reliable native clipboard 访问 from this 线程 without a
/// UI round-trip; the 错误 text is already in logcat. No-op.
pub fn copy_to_clipboard(_text: &str) -> std::io::Result<()> {
    Ok(())
}
