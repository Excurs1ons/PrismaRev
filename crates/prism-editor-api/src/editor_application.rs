//! 编辑器门面。
//!
//! 引擎的 `EditorApplication` 是全局静态门面；这里的「全局状态」是显式的
//! [`EditorApplicationState`]，由宿主（Tauri 后端）持有并通过命令读写，避免静态可变状态。
//!
//! | | 这里 |
//! |---|---|
//! | `EditorApplication.isPlaying` / `isPaused` | [`EditorApplicationState::is_playing`] / [`is_paused`] |
//! | `EditorApplication.isPlayingOrWillChangePlaymode` | [`EditorApplicationState::is_playing_or_will_change_playmode`] |
//! | `EditorApplication.isCompiling` / `isUpdating` | 字段 `is_compiling` / `is_updating` |
//! | `EditorApplication.EnterPlaymode` / `ExitPlaymode` | [`EditorApplicationState::enter_playmode`] / [`exit_playmode`] |
//! | `EditorApplication.delayCall` | [`EditorApplicationState::delay_call`] |
//! | `EditorApplication.projectPath` / `dataPath` | [`project_path`] / [`data_path`] |
//! | `EditorApplication.applicationPath` | [`application_path`] |

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 播放模式状态（对应 `isPlaying` / `isPaused` 的组合）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PlayModeState {
    /// `isPlaying == false`。
    Stopped,
    /// `isPlaying == true && isPaused == false`。
    Playing,
    /// `isPlaying == true && isPaused == true`。
    Paused,
}

/// 编辑器全局状态。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EditorApplicationState {
    /// 播放模式。
    pub play_mode: PlayModeState,
    /// 是否正在编译 / 烘焙 = `isCompiling`。
    pub is_compiling: bool,
    /// 是否正在更新 = `isUpdating`。
    pub is_updating: bool,
    /// 编辑器当前打开的场景逻辑名。
    pub active_scene: Option<String>,
    /// 正在播放的场景逻辑名（`enter_playmode` 时记录）。
    pub play_scene: Option<String>,
    /// 待执行的下一个 tick 回调 = `delayCall`（这里只排标签，宿主负责执行）。
    pub delayed_calls: Vec<String>,
}

impl Default for EditorApplicationState {
    fn default() -> Self {
        Self::new()
    }
}

impl EditorApplicationState {
    pub fn new() -> Self {
        Self {
            play_mode: PlayModeState::Stopped,
            is_compiling: false,
            is_updating: false,
            active_scene: None,
            play_scene: None,
            delayed_calls: Vec::new(),
        }
    }

    /// `isPlaying`。
    pub fn is_playing(&self) -> bool {
        !matches!(self.play_mode, PlayModeState::Stopped)
    }

    /// `isPaused`。
    pub fn is_paused(&self) -> bool {
        matches!(self.play_mode, PlayModeState::Paused)
    }

    /// `isPlayingOrWillChangePlaymode`：这里等价于「正在播放」（没有延迟切换窗口）。
    pub fn is_playing_or_will_change_playmode(&self) -> bool {
        self.is_playing()
    }

    /// 进入播放模式 = `EditorApplication.EnterPlaymode`。
    pub fn enter_playmode(&mut self, scene: Option<String>) {
        self.play_mode = PlayModeState::Playing;
        self.play_scene = scene;
    }

    /// 暂停 = `EditorApplication.isPaused = true`。未在播放时无操作。
    pub fn pause(&mut self) {
        if self.is_playing() {
            self.play_mode = PlayModeState::Paused;
        }
    }

    /// 继续 = `EditorApplication.isPaused = false`。未在播放时无操作。
    pub fn resume(&mut self) {
        if self.is_playing() {
            self.play_mode = PlayModeState::Playing;
        }
    }

    /// 退出播放模式 = `EditorApplication.ExitPlaymode`。
    pub fn exit_playmode(&mut self) {
        self.play_mode = PlayModeState::Stopped;
        self.play_scene = None;
    }

    /// 设置编译/烘焙状态。
    pub fn set_compiling(&mut self, compiling: bool) {
        self.is_compiling = compiling;
    }

    /// 设置更新状态。
    pub fn set_updating(&mut self, updating: bool) {
        self.is_updating = updating;
    }

    /// 记录当前打开的场景 = `SceneManager` 层面的 active scene。
    pub fn set_active_scene(&mut self, scene: Option<String>) {
        self.active_scene = scene;
    }

    /// 排一个「下一帧执行」的回调标签 = `delayCall`。
    pub fn delay_call(&mut self, label: impl Into<String>) {
        self.delayed_calls.push(label.into());
    }

    /// 取出并清空待执行回调（宿主在自己的 tick 里调用）。
    pub fn run_delayed_calls(&mut self) -> Vec<String> {
        std::mem::take(&mut self.delayed_calls)
    }
}

// ---------------------------------------------------------------------------
// 路径 / 版本
// ---------------------------------------------------------------------------

/// 项目根（绝对化，尽力） = `EditorApplication.projectPath`。
pub fn project_path(project_root: &Path) -> PathBuf {
    std::fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf())
}

/// 资产数据目录（`<root>/assets`） = `EditorApplication.dataPath`。
pub fn data_path(project_root: &Path) -> PathBuf {
    project_root.join("assets")
}

/// 流式资产目录（这里与 [`data_path`] 相同） = `Application.streamingAssetsPath`。
pub fn streaming_assets_path(project_root: &Path) -> PathBuf {
    project_root.join("assets")
}

/// 编辑器可执行文件所在目录 = `EditorApplication.applicationPath`。
pub fn application_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 引擎版本 = `Application.Version`（这里是 `prism-editor-api` 的版本）。
pub fn engine_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn play_mode_transitions() {
        let mut state = EditorApplicationState::new();
        assert!(!state.is_playing());
        assert!(!state.is_paused());
        assert!(!state.is_playing_or_will_change_playmode());

        state.enter_playmode(Some("intro".into()));
        assert!(state.is_playing());
        assert!(!state.is_paused());
        assert_eq!(state.play_scene.as_deref(), Some("intro"));

        state.pause();
        assert!(state.is_playing());
        assert!(state.is_paused());

        state.resume();
        assert!(!state.is_paused());

        state.exit_playmode();
        assert!(!state.is_playing());
        assert_eq!(state.play_scene, None);

        // 未播放时 pause 无副作用。
        state.pause();
        assert!(!state.is_paused());
    }

    #[test]
    fn delay_call_queue() {
        let mut state = EditorApplicationState::new();
        state.delay_call("repaint");
        state.delay_call("rebuild-schema");
        assert_eq!(state.delayed_calls.len(), 2);
        assert_eq!(
            state.run_delayed_calls(),
            vec!["repaint".to_string(), "rebuild-schema".to_string()]
        );
        assert!(state.delayed_calls.is_empty());
    }

    #[test]
    fn paths_resolve_under_root() {
        let root = Path::new("/tmp/project");
        assert!(data_path(root).ends_with("assets"));
        assert!(streaming_assets_path(root).ends_with("assets"));
        assert_eq!(engine_version(), env!("CARGO_PKG_VERSION"));
        assert!(!application_path().as_os_str().is_empty());
    }
}
