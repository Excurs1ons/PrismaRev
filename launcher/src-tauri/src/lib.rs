//! PrismaRev Hub 的 Tauri 外壳。
//!
//! 桌面端是 Unity Hub 形态的工程管理窗口（工程列表 / 新建 / 添加 / 打开
//! 编辑器），工程逻辑在 [`hub`]；Android 端仍然只负责拉起游戏 Activity。

#[cfg(not(mobile))]
mod game_launcher;
#[cfg(not(mobile))]
mod hub;

#[cfg(not(mobile))]
use hub::{
    hub_add_project, hub_create_project, hub_editor_info, hub_environment, hub_open_project,
    hub_projects, hub_remove_project, hub_reveal_project, HubState,
};
use tauri::Manager;
use tauri::Wry;

// ==================== Android: 自定义 Native Plugin ====================
//
// 通过 Kotlin 调用 Android 原生 API：
//   - toggle_immersive / set_status_bar_color / vibrate / get_device_info
//   - launch_game：启动 GameActivity

#[cfg(mobile)]
use tauri::plugin::PluginHandle;

#[cfg(mobile)]
const NATIVE_PLUGIN_IDENTIFIER: &str = "com.example.tauriandroidapp";

/// 封装对 Kotlin plugin 的调用。
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

#[tauri::command]
#[cfg(mobile)]
fn toggle_immersive(state: tauri::State<NativeApi>) -> Result<bool, String> {
    let result: serde_json::Value = state.call("toggle_immersive", ())?;
    result
        .get("immersive")
        .and_then(|v| v.as_bool())
        .ok_or("Kotlin 返回格式异常".to_string())
}

#[tauri::command]
#[cfg(mobile)]
fn set_status_bar_color(state: tauri::State<NativeApi>, color: String) -> Result<(), String> {
    state.call::<serde_json::Value>(
        "set_status_bar_color",
        serde_json::json!({ "color": color }),
    )?;
    Ok(())
}

#[tauri::command]
#[cfg(mobile)]
fn vibrate(state: tauri::State<NativeApi>, duration: u64) -> Result<(), String> {
    state.call::<serde_json::Value>("vibrate", serde_json::json!({ "duration": duration }))?;
    Ok(())
}

#[tauri::command]
#[cfg(mobile)]
fn get_device_info(state: tauri::State<NativeApi>) -> Result<serde_json::Value, String> {
    state.call("get_device_info", ())
}

// Desktop 桩实现：让命令在桌面端也能编译，调用时返回提示。
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

/// 启动游戏：移动端通过 Kotlin plugin 启动 GameActivity；桌面端拉起
/// `prisma` 二进制。`config` 为可选 LaunchConfig JSON（场景 / 日志级别等），
/// 经 `PRISMREV_LAUNCH_CONFIG` env（桌面）或 files 目录文件（Android）传递。
#[cfg(mobile)]
#[tauri::command]
fn launch_game(state: tauri::State<NativeApi>, config: Option<String>) -> Result<(), String> {
    state
        .call::<serde_json::Value>("launch_game", serde_json::json!({ "config": config }))
        .map(|_| ())
}

/// 桌面端：二进制由 [`game_launcher::resolve`] 定位（不走 `PATH`），
/// 已有一个游戏在跑时直接拒绝，避免同时开两个实例。
#[cfg(not(mobile))]
#[tauri::command]
fn launch_game(
    state: tauri::State<'_, game_launcher::RunningGame>,
    config: Option<String>,
) -> Result<(), String> {
    state.launch(config)
}

/// 前端据此决定渲染 Hub 还是 Android 的「开始游戏」页。
#[cfg(mobile)]
#[tauri::command]
fn hub_environment() -> serde_json::Value {
    serde_json::json!({
        "platform": "mobile",
        "editor": { "found": false, "path": null, "version": "", "hint": null },
        "repoRoot": null,
    })
}

/// 初始化自定义原生插件（mobile 端注册 Kotlin plugin）。
pub fn init_native_plugin() -> tauri::plugin::TauriPlugin<Wry> {
    tauri::plugin::Builder::<Wry>::new("native-api")
        .setup(|_app, _api| {
            #[cfg(target_os = "android")]
            {
                let handle =
                    _api.register_android_plugin(NATIVE_PLUGIN_IDENTIFIER, "NativePlugin")?;
                _app.manage(NativeApi(handle));
            }
            Ok(())
        })
        .build()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(init_native_plugin());

    #[cfg(not(mobile))]
    let builder = builder
        .manage(game_launcher::RunningGame::default())
        .setup(|app| {
            let state = HubState::load(app.handle()).map_err(Box::<dyn std::error::Error>::from)?;
            app.manage(state);
            Ok(())
        });

    builder
        .invoke_handler(tauri::generate_handler![
            launch_game,
            hub_environment,
            toggle_immersive,
            set_status_bar_color,
            vibrate,
            get_device_info,
            #[cfg(not(mobile))]
            hub_editor_info,
            #[cfg(not(mobile))]
            hub_projects,
            #[cfg(not(mobile))]
            hub_add_project,
            #[cfg(not(mobile))]
            hub_remove_project,
            #[cfg(not(mobile))]
            hub_create_project,
            #[cfg(not(mobile))]
            hub_open_project,
            #[cfg(not(mobile))]
            hub_reveal_project,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
