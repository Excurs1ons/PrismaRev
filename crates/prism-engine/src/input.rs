/// 抽象按键码（平台无关）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyCode {
    KeyW,
    KeyA,
    KeyS,
    KeyD,
    KeyQ,
    KeyE,
    Space,
    ShiftLeft,
    ShiftRight,
    ControlLeft,
    ControlRight,
    AltLeft,
    AltRight,
    Escape,
    Tab,
    Enter,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Digit0,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    Other(u32),
}

/// 调 or 按钮 状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElementState {
    Pressed,
    Released,
}

/// 鼠标 按钮 抽象
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
    Other(u16),
}

/// 触摸 phase (platform-independent).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchPhase {
    Started,
    Moved,
    Ended,
    Cancelled,
}

/// A single 触摸 事件
#[derive(Clone, Copy, Debug)]
pub struct TouchEvent {
    pub id: u64,
    pub phase: TouchPhase,
    pub position: [f64; 2],
}

/// Per-frame 输入 状态
///
/// Owns the raw 输入 状态 (held keys, 鼠标 position, transient just-pressed
/// events) and the pointer-lock 状态 机 键盘 shortcuts that 触发器
/// app- or editor-level actions are handled by the 调用者 — this 结构体 only
/// tracks which keys are pressed.
///
/// ## 用法
/// 1. 调用 individual handlers (`handle_keyboard`, `handle_mouse_button`,
///    `handle_mouse_move`, `handle_scroll`, `handle_touch`) from the app's
/// 事件 处理器 to route window-system events into the 状态 机
/// 2. 调用 [`begin_frame`](Self::begin_frame) at the 结束 of each 帧 to
/// 清空 transient 状态
/// 3. 查询 helpers (`key_held`, `mouse_delta`, etc.) during the frame's
/// 更新 逻辑
#[derive(Default, Clone)]
pub struct InputManager {
    // Persistent (accumulated across frames)
    keys_held: rustc_hash::FxHashSet<KeyCode>,
    mouse_buttons_held: rustc_hash::FxHashSet<MouseButton>,
    mouse_position: [f64; 2],

    // Transient (cleared each 帧 by begin_frame)
    keys_just_pressed: Vec<KeyCode>,
    keys_just_released: Vec<KeyCode>,
    mouse_just_pressed: Vec<MouseButton>,
    mouse_delta: [f64; 2],
    scroll_delta: f64,
    touches: Vec<TouchEvent>,

    // Pointer-lock 状态
    /// Whether the cursor is 隐藏 + grabbed for FPS-style look.
    pub pointer_locked: bool,
    /// Whether ALT is held and has temporarily released a locked 指针
    pub alt_temp_release: bool,
    /// 集合 when the 窗口 regains focus; the 下一个 left-click focuses the
    /// 窗口 instead of entering 指针 lock.
    pub focus_return_click: bool,
    /// Whether the 指针 was locked before the inspector/visualizer was
    /// opened, so it can be re-locked when the UI closes.
    pub lock_before_inspector: bool,
}

impl InputManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// 调用 at the START of each 帧 to reset transient 状态
    pub fn begin_frame(&mut self) {
        self.keys_just_pressed.clear();
        self.keys_just_released.clear();
        self.mouse_just_pressed.clear();
        self.mouse_delta = [0.0; 2];
        self.scroll_delta = 0.0;
        self.touches.clear();
    }

    // --- 查询 helpers ---

    pub fn key_held(&self, key: KeyCode) -> bool {
        self.keys_held.contains(&key)
    }
    pub fn key_just_pressed(&self, key: KeyCode) -> bool {
        self.keys_just_pressed.contains(&key)
    }
    pub fn key_just_released(&self, key: KeyCode) -> bool {
        self.keys_just_released.contains(&key)
    }
    pub fn mouse_held(&self, button: MouseButton) -> bool {
        self.mouse_buttons_held.contains(&button)
    }
    pub fn mouse_just_pressed(&self, button: MouseButton) -> bool {
        self.mouse_just_pressed.contains(&button)
    }
    pub fn mouse_delta(&self) -> [f64; 2] {
        self.mouse_delta
    }
    pub fn scroll_delta(&self) -> f64 {
        self.scroll_delta
    }
    pub fn mouse_position(&self) -> [f64; 2] {
        self.mouse_position
    }
    pub fn touches(&self) -> &[TouchEvent] {
        &self.touches
    }

    // --- Low-level 事件 handlers 插入 raw data) ---

    pub fn handle_keyboard(&mut self, key: KeyCode, state: ElementState) {
        match state {
            ElementState::Pressed => {
                if self.keys_held.insert(key) {
                    self.keys_just_pressed.push(key);
                }
            }
            ElementState::Released => {
                if self.keys_held.remove(&key) {
                    self.keys_just_released.push(key);
                }
            }
        }
    }

    pub fn handle_mouse_move(&mut self, position: [f64; 2]) {
        self.mouse_delta[0] += position[0] - self.mouse_position[0];
        self.mouse_delta[1] += position[1] - self.mouse_position[1];
        self.mouse_position = position;
    }

    /// 集合 the 指针 position without accumulating delta.
    pub fn set_mouse_position(&mut self, position: [f64; 2]) {
        self.mouse_position = position;
    }

    pub fn handle_mouse_button(&mut self, button: MouseButton, state: ElementState) {
        match state {
            ElementState::Pressed => {
                if self.mouse_buttons_held.insert(button) {
                    self.mouse_just_pressed.push(button);
                }
            }
            ElementState::Released => {
                self.mouse_buttons_held.remove(&button);
            }
        }
    }

    /// Handle a 滚动 事件 `delta_y` is 正 for 滚动 上 (zoom in),
    /// 负 for 滚动 下
    pub fn handle_scroll(&mut self, delta_y: f64) {
        self.scroll_delta += delta_y;
    }

    pub fn handle_touch(&mut self, id: u64, phase: TouchPhase, position: [f64; 2]) {
        self.touches.push(TouchEvent {
            id,
            phase,
            position,
        });
    }

    /// 更新 the pointer-lock 状态 without performing any window-system
    /// operations (cursor grab / 可见性 The 调用者 is responsible for
    /// adjusting the actual cursor 状态 via the 窗口 系统
    ///
    /// Resets accumulated 鼠标 delta so the 视图 doesn't jump after the
    /// 过渡
    pub fn set_locked(&mut self, locked: bool) {
        self.pointer_locked = locked;
        self.begin_frame();
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    fn key_w() -> KeyCode {
        KeyCode::KeyW
    }
    fn key_space() -> KeyCode {
        KeyCode::Space
    }

    #[test]
    fn new_is_empty() {
        let s = InputManager::new();
        assert!(!s.key_held(KeyCode::KeyW));
        assert!(!s.mouse_held(MouseButton::Left));
        assert_eq!(s.mouse_delta(), [0.0; 2]);
        assert_eq!(s.scroll_delta(), 0.0);
        assert_eq!(s.mouse_position(), [0.0; 2]);
        assert!(s.touches().is_empty());
    }

    #[test]
    fn key_press_adds_held_and_just_pressed() {
        let mut s = InputManager::new();
        s.handle_keyboard(key_w(), ElementState::Pressed);
        assert!(s.key_held(KeyCode::KeyW));
        assert!(s.key_just_pressed(KeyCode::KeyW));
    }

    #[test]
    fn key_held_survives_begin_frame() {
        let mut s = InputManager::new();
        s.handle_keyboard(key_w(), ElementState::Pressed);
        s.begin_frame();
        assert!(s.key_held(KeyCode::KeyW));
        assert!(!s.key_just_pressed(KeyCode::KeyW)); // transient cleared
    }

    #[test]
    fn key_just_released_on_release() {
        let mut s = InputManager::new();
        s.handle_keyboard(key_w(), ElementState::Pressed);
        s.begin_frame();
        s.handle_keyboard(key_w(), ElementState::Released);
        assert!(!s.key_held(KeyCode::KeyW));
        assert!(s.key_just_released(KeyCode::KeyW));
    }

    #[test]
    fn duplicate_key_press_does_not_double_just_pressed() {
        let mut s = InputManager::new();
        s.handle_keyboard(key_w(), ElementState::Pressed);
        s.handle_keyboard(key_w(), ElementState::Pressed); // duplicate
        assert!(s.key_held(KeyCode::KeyW));
        assert_eq!(s.keys_just_pressed.len(), 1); // only once
    }

    #[test]
    fn mouse_delta_accumulates_and_resets() {
        let mut s = InputManager::new();
        s.handle_mouse_move([100.0, 200.0]);
        assert_eq!(s.mouse_delta(), [100.0, 200.0]);
        assert_eq!(s.mouse_position(), [100.0, 200.0]);

        s.handle_mouse_move([110.0, 195.0]);
        assert_eq!(s.mouse_delta(), [110.0, 195.0]); // full delta from origin
        assert_eq!(s.mouse_position(), [110.0, 195.0]);

        s.begin_frame();
        assert_eq!(s.mouse_delta(), [0.0, 0.0]);
        assert_eq!(s.mouse_position(), [110.0, 195.0]); // position persists
    }

    #[test]
    fn scroll_delta_y() {
        let mut s = InputManager::new();
        s.handle_scroll(3.0);
        assert!((s.scroll_delta() - 3.0).abs() < 1e-9);
        s.handle_scroll(-1.0);
        assert!((s.scroll_delta() - 2.0).abs() < 1e-9); // accumulated
    }

    #[test]
    fn mouse_button_held_and_just_pressed() {
        let mut s = InputManager::new();
        s.handle_mouse_button(MouseButton::Left, ElementState::Pressed);
        assert!(s.mouse_held(MouseButton::Left));

        s.begin_frame();
        assert!(s.mouse_held(MouseButton::Left));
        assert_eq!(s.mouse_just_pressed.len(), 0); // transient cleared
    }

    #[test]
    fn mouse_button_release_clears_held() {
        let mut s = InputManager::new();
        s.handle_mouse_button(MouseButton::Left, ElementState::Pressed);
        s.begin_frame();
        s.handle_mouse_button(MouseButton::Left, ElementState::Released);
        assert!(!s.mouse_held(MouseButton::Left));
    }

    #[test]
    fn touch_events_accumulate_and_clear() {
        let mut s = InputManager::new();
        s.handle_touch(1, TouchPhase::Started, [10.0, 20.0]);
        s.handle_touch(2, TouchPhase::Moved, [30.0, 40.0]);
        assert_eq!(s.touches().len(), 2);
        assert_eq!(s.touches()[0].id, 1);
        assert_eq!(s.touches()[1].id, 2);

        s.begin_frame();
        assert!(s.touches().is_empty());
    }

    #[test]
    fn begin_frame_clears_all_transient() {
        let mut s = InputManager::new();
        s.handle_keyboard(key_space(), ElementState::Pressed);
        s.handle_mouse_button(MouseButton::Right, ElementState::Pressed);
        s.handle_mouse_move([50.0, 60.0]);
        s.handle_scroll(5.0);

        s.begin_frame();
        assert!(!s.key_just_pressed(KeyCode::Space));
        assert!(s.key_held(KeyCode::Space)); // held persists
        assert_eq!(s.mouse_delta(), [0.0; 2]);
        assert_eq!(s.scroll_delta(), 0.0);
    }
}
