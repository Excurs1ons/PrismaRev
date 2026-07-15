use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{Emitter, Manager, Wry};
#[cfg(mobile)]
use tauri_plugin_fs::FsExt;

// ==================== 1. 前后端通信演示 ====================

/// 基础命令:接收参数,返回字符串
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

/// 返回结构体的命令:演示 Rust -> JS 的复杂数据传递
#[derive(Serialize)]
struct ServerInfo {
    rust_version: String,
    timestamp: u64,
    message: String,
}

#[tauri::command]
fn get_server_info() -> ServerInfo {
    ServerInfo {
        rust_version: env!("CARGO_PKG_RUST_VERSION", "unknown").to_string(),
        timestamp: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        message: "这条数据来自 Rust 后端".to_string(),
    }
}

/// Rust 主动向前端发送事件(演示后端 -> 前端的事件推送)
#[tauri::command]
fn send_event_to_frontend(app: tauri::AppHandle<Wry>) -> Result<(), String> {
    // 延迟 500ms 后发事件,模拟异步通知场景
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(500));
        let _ = handle.emit(
            "rust-event",
            serde_json::json!({
                "title": "来自 Rust 的事件",
                "body": format!("事件触发时间戳: {}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis()),
            }),
        );
    });
    Ok(())
}

// ==================== 2. 文件系统演示(官方 tauri-plugin-fs) ====================

/// 在 app 私有目录写入文件,返回写入路径
#[tauri::command]
fn write_demo_file(app: tauri::AppHandle<Wry>, content: String) -> Result<String, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("获取目录失败: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建目录失败: {e}"))?;
    let file_path = dir.join("demo.txt");
    std::fs::write(&file_path, &content).map_err(|e| format!("写入失败: {e}"))?;
    Ok(file_path.to_string_lossy().to_string())
}

/// 读取刚才写入的文件
#[tauri::command]
fn read_demo_file(app: tauri::AppHandle<Wry>) -> Result<String, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("获取目录失败: {e}"))?;
    let file_path = dir.join("demo.txt");
    std::fs::read_to_string(&file_path).map_err(|e| format!("读取失败: {e}"))
}

// ==================== 3. 自定义 Mobile Plugin:原生 API 访问 ====================
//
// 这个插件通过 Kotlin 调用 Android 原生 API,演示:
//   - toggle_immersive:  切换全屏沉浸模式(隐藏/显示状态栏)
//   - set_status_bar_color: 修改状态栏颜色
//   - vibrate:  震动(硬件访问)
//   - get_device_info: 获取设备信息(机型/Android版本/CPU/屏幕)

#[cfg(mobile)]
use tauri::plugin::PluginHandle;

#[cfg(mobile)]
const NATIVE_PLUGIN_IDENTIFIER: &str = "com.example.tauriandroidapp";

/// 封装对 Kotlin plugin 的调用
#[cfg(mobile)]
struct NativeApi(PluginHandle<Wry>);

#[cfg(mobile)]
impl NativeApi {
    fn call<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        payload: impl serde::Serialize,
    ) -> Result<T, String> {
        self.0
            .run_mobile_plugin(method, payload)
            .map_err(|e| e.to_string())
    }
}

/// 切换全屏沉浸模式
#[tauri::command]
#[cfg(mobile)]
fn toggle_immersive(state: tauri::State<NativeApi>) -> Result<bool, String> {
    let result: serde_json::Value = state.call("toggle_immersive", ())?;
    result
        .get("immersive")
        .and_then(|v| v.as_bool())
        .ok_or("Kotlin 返回格式异常".to_string())
}

/// 设置状态栏颜色 (hex, 如 "#FF5722")
#[tauri::command]
#[cfg(mobile)]
fn set_status_bar_color(
    state: tauri::State<NativeApi>,
    color: String,
) -> Result<(), String> {
    state.call::<serde_json::Value>("set_status_bar_color", serde_json::json!({ "color": color }))?;
    Ok(())
}

/// 震动(毫秒)
#[tauri::command]
#[cfg(mobile)]
fn vibrate(state: tauri::State<NativeApi>, duration: u64) -> Result<(), String> {
    state.call::<serde_json::Value>("vibrate", serde_json::json!({ "duration": duration }))?;
    Ok(())
}

/// 获取设备信息
#[tauri::command]
#[cfg(mobile)]
fn get_device_info(state: tauri::State<NativeApi>) -> Result<serde_json::Value, String> {
    state.call("get_device_info", ())
}

// Desktop 桩实现:让命令在桌面端也能编译,调用时返回提示
#[cfg(not(mobile))]
#[tauri::command]
fn toggle_immersive() -> Result<bool, String> {
    Err("全屏模式仅在 Android 端可用".into())
}

#[cfg(not(mobile))]
#[tauri::command]
fn set_status_bar_color(_color: String) -> Result<(), String> {
    Err("状态栏控制仅在 Android 端可用".into())
}

#[cfg(not(mobile))]
#[tauri::command]
fn vibrate(_duration: u64) -> Result<(), String> {
    Err("震动仅在 Android 端可用".into())
}

#[cfg(not(mobile))]
#[tauri::command]
fn get_device_info() -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({
        "platform": "desktop",
        "message": "设备信息仅在 Android 端可获取"
    }))
}

/// 启动游戏:移动端通过 Kotlin plugin 启动 GameActivity;桌面端直接拉起 prismarev 二进制
#[cfg(mobile)]
#[tauri::command]
fn launch_game(state: tauri::State<NativeApi>) -> Result<(), String> {
    state.call::<serde_json::Value>("launch_game", ()).map(|_| ())
}

#[cfg(not(mobile))]
#[tauri::command]
fn launch_game() -> Result<(), String> {
    std::process::Command::new("prismarev")
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 初始化自定义原生插件(mobile 端注册 Kotlin plugin)
pub fn init_native_plugin() -> tauri::plugin::TauriPlugin<Wry> {
    tauri::plugin::Builder::<Wry>::new("native-api")
        .setup(|_app, _api| {
            #[cfg(target_os = "android")]
            {
                let handle = _api.register_android_plugin(NATIVE_PLUGIN_IDENTIFIER, "NativePlugin")?;
                _app.manage(NativeApi(handle));
            }
            Ok(())
        })
        .build()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(init_native_plugin())
        // 允许前端访问 app 数据目录(fs 插件 scope)
        .setup(|app| {
            #[cfg(mobile)]
            {
                let data_dir = app
                    .path()
                    .app_data_dir()
                    .unwrap_or_else(|_| std::path::PathBuf::from("."));
                if let Some(fs_scope) = app.try_fs_scope() {
                    let _ = fs_scope.allow_directory(data_dir, true);
                }
            }
            let _ = app;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            greet,
            get_server_info,
            send_event_to_frontend,
            write_demo_file,
            read_demo_file,
            toggle_immersive,
            set_status_bar_color,
            vibrate,
            get_device_info,
            launch_game,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
