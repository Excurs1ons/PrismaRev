//! `UnityEngine.Input` 形状的门面。
//!
//! Unity 的 `Input` 是静态类，游戏代码直接 `Input.GetKey(KeyCode.W)`。本项目
//! 的输入状态在 [`InputManager`]（每帧由平台层填充），这里把它投影成同名的
//! 只读查询入口，让熟悉 Unity 的人一眼能对上：
//!
//! | Unity | 这里 |
//! |---|---|
//! | `Input.GetKey(k)` | [`Input::get_key`] |
//! | `Input.GetKeyDown(k)` | [`Input::get_key_down`] |
//! | `Input.GetKeyUp(k)` | [`Input::get_key_up`] |
//! | `Input.GetMouseButton(b)` | [`Input::get_mouse_button`] |
//! | `Input.GetMouseButtonDown(b)` | [`Input::get_mouse_button_down`] |
//! | `Input.mousePosition` | [`Input::mouse_position`] |
//! | `Input.mouseScrollDelta` | [`Input::mouse_scroll_delta`] |
//! | `Input.anyKey` | [`Input::any_key`] |
//! | `Input.anyKeyDown` | [`Input::any_key_down`] |
//! | `Input.GetAxis("Mouse X"/"Mouse Y"/"Mouse ScrollWheel")` | [`Input::get_axis`] |
//!
//! 差异（刻意）：
//! - 没有 `Input.GetButton("Fire1")` 这类 **Input Manager 命名轴**：本项目没有
//!   `ProjectSettings/InputManager.asset`，键位由调用方直接查 `KeyCode`。
//!   需要命名轴时在游戏代码里写一个小映射即可。
//! - `GetAxis` 只实现鼠标三条内置轴，其余名字返回 `0.0`（不静默“假装有平滑”）。
//! - 没有 `Input.simulateMouseWithTouches` / `multiTouchEnabled` / `imeIsSelected`
//!   等平台开关：触摸走 [`InputManager::touches`]，开关交给平台层。
//! - 没有 `Input.location` / `Input.compass` / `Input.acceleration` 等传感器：
//!   本项目目前没有传感器后端（见 `docs/UNITY_ALIGNMENT.md` §5.3）。

use crate::input::{InputManager, KeyCode, MouseButton, TouchEvent};

/// Unity `Input` 的只读门面（借用当帧的 [`InputManager`]）。
///
/// 构造很便宜：`Input::from(manager)` 拿一个借用，查询全部转发到 `InputManager`。
/// 在每帧 [`InputManager::begin_frame`] 之前查询；`GetKeyDown` / `GetKeyUp`
/// 只在当帧为真。
#[derive(Clone, Copy)]
pub struct Input<'a> {
    manager: &'a InputManager,
}

impl<'a> Input<'a> {
    /// 包装当帧的输入状态。
    pub fn new(manager: &'a InputManager) -> Self {
        Self { manager }
    }

    /// 底层 [`InputManager`]（需要 `pointer_locked` 之类的平台状态时用）。
    pub fn manager(&self) -> &'a InputManager {
        self.manager
    }

    // --- 键盘 -------------------------------------------------------------

    /// 按键是否按住（`Input.GetKey`）。
    pub fn get_key(&self, key: KeyCode) -> bool {
        self.manager.key_held(key)
    }

    /// 按键本帧刚按下（`Input.GetKeyDown`）。
    pub fn get_key_down(&self, key: KeyCode) -> bool {
        self.manager.key_just_pressed(key)
    }

    /// 按键本帧刚释放（`Input.GetKeyUp`）。
    pub fn get_key_up(&self, key: KeyCode) -> bool {
        self.manager.key_just_released(key)
    }

    /// 任意键按住（`Input.anyKey`）。
    pub fn any_key(&self) -> bool {
        self.manager.any_key_held() || self.manager.any_mouse_button_held()
    }

    /// 任意键本帧刚按下（`Input.anyKeyDown`）。
    pub fn any_key_down(&self) -> bool {
        !self.manager.pressed_keys().is_empty() || !self.manager.buttons_just_pressed().is_empty()
    }

    // --- 鼠标 -------------------------------------------------------------

    /// 鼠标键是否按住（`Input.GetMouseButton`）。
    pub fn get_mouse_button(&self, button: MouseButton) -> bool {
        self.manager.mouse_held(button)
    }

    /// 鼠标键本帧刚按下（`Input.GetMouseButtonDown`）。
    pub fn get_mouse_button_down(&self, button: MouseButton) -> bool {
        self.manager.mouse_just_pressed(button)
    }

    /// 鼠标键本帧刚释放（`Input.GetMouseButtonUp`）。
    pub fn get_mouse_button_up(&self, button: MouseButton) -> bool {
        self.manager.mouse_just_released(button)
    }

    /// 指针位置（`Input.mousePosition`）。原点在左下角，y 向上，与 Unity 一致。
    pub fn mouse_position(&self) -> [f64; 2] {
        self.manager.mouse_position()
    }

    /// 本帧滚动增量（`Input.mouseScrollDelta`）：x = 水平，y = 竖直（上为正面）。
    pub fn mouse_scroll_delta(&self) -> [f64; 2] {
        [0.0, self.manager.scroll_delta()]
    }

    /// 命名轴（`Input.GetAxis`）。
    ///
    /// 只支持 Unity 内置的鼠标轴：
    /// - `"Mouse X"` → 本帧水平位移
    /// - `"Mouse Y"` → 本帧竖直位移
    /// - `"Mouse ScrollWheel"` → 本帧滚动增量
    ///
    /// 其它名字返回 `0.0`（本项目没有 Input Manager 命名轴）。
    pub fn get_axis(&self, axis: &str) -> f64 {
        match axis {
            "Mouse X" => self.manager.mouse_delta()[0],
            "Mouse Y" => self.manager.mouse_delta()[1],
            "Mouse ScrollWheel" => self.manager.scroll_delta(),
            _ => 0.0,
        }
    }

    /// `Input.GetAxisRaw` 在本项目里与 [`get_axis`](Self::get_axis) 等价
    /// （没有平滑/缓动层）。
    pub fn get_axis_raw(&self, axis: &str) -> f64 {
        self.get_axis(axis)
    }

    // --- 触摸 -------------------------------------------------------------

    /// 本帧触摸事件（近似 `Input.touches`，但按事件而非按触点列表给出）。
    pub fn touches(&self) -> &'a [TouchEvent] {
        self.manager.touches()
    }

    /// 本帧触摸事件个数（`Input.touchCount` 的近似）。
    pub fn touch_count(&self) -> usize {
        self.manager.touches().len()
    }
}

impl<'a> From<&'a InputManager> for Input<'a> {
    fn from(manager: &'a InputManager) -> Self {
        Self::new(manager)
    }
}

#[cfg(test)]
#[path = "input_facade_tests.rs"]
mod tests;
