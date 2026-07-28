/// Abstract key code (platform-independent).
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

/// Key or button state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElementState {
    Pressed,
    Released,
}

/// Mouse button abstraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
    Other(u16),
}

/// Touch phase (platform-independent).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchPhase {
    Started,
    Moved,
    Ended,
    Cancelled,
}

/// A single touch event.
#[derive(Clone, Copy, Debug)]
pub struct TouchEvent {
    pub id: u64,
    pub phase: TouchPhase,
    pub position: [f64; 2],
}

/// Per-frame input state.
///
/// Owns the raw input state (held keys, mouse position, transient just-pressed
/// events) and the pointer-lock state machine.  Keyboard shortcuts that trigger
/// app- or editor-level actions are handled by the caller — this struct only
/// tracks which keys are pressed.
///
/// ## Usage
/// 1. Call individual handlers (`handle_keyboard`, `handle_mouse_button`,
///    `handle_mouse_move`, `handle_scroll`, `handle_touch`) from the app's
///    event handler to route window-system events into the state machine.
/// 2. Call [`begin_frame`](Self::begin_frame) at the end of each frame to
///    clear transient state.
/// 3. Query helpers (`key_held`, `mouse_delta`, etc.) during the frame's
///    update logic.
#[derive(Default, Clone)]
pub struct InputManager {
    // Persistent (accumulated across frames)
    keys_held: rustc_hash::FxHashSet<KeyCode>,
    mouse_buttons_held: rustc_hash::FxHashSet<MouseButton>,
    mouse_position: [f64; 2],

    // Transient (cleared each frame by begin_frame)
    keys_just_pressed: Vec<KeyCode>,
    keys_just_released: Vec<KeyCode>,
    mouse_just_pressed: Vec<MouseButton>,
    mouse_delta: [f64; 2],
    scroll_delta: f64,
    touches: Vec<TouchEvent>,

    // Pointer-lock state
    /// Whether the cursor is hidden + grabbed for FPS-style look.
    pub pointer_locked: bool,
    /// Whether ALT is held and has temporarily released a locked pointer.
    pub alt_temp_release: bool,
    /// Set when the window regains focus; the next left-click focuses the
    /// window instead of entering pointer lock.
    pub focus_return_click: bool,
    /// Whether the pointer was locked before the inspector/visualizer was
    /// opened, so it can be re-locked when the UI closes.
    pub lock_before_inspector: bool,
}

impl InputManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Call at the START of each frame to reset transient state.
    pub fn begin_frame(&mut self) {
        self.keys_just_pressed.clear();
        self.keys_just_released.clear();
        self.mouse_just_pressed.clear();
        self.mouse_delta = [0.0; 2];
        self.scroll_delta = 0.0;
        self.touches.clear();
    }

    // --- Query helpers ---

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

    // --- Low-level event handlers (insert raw data) ---

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

    /// Set the pointer position without accumulating delta.
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

    /// Handle a scroll event. `delta_y` is positive for scroll up (zoom in),
    /// negative for scroll down.
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

    /// Update the pointer-lock state without performing any window-system
    /// operations (cursor grab / visibility).  The caller is responsible for
    /// adjusting the actual cursor state via the window system.
    ///
    /// Resets accumulated mouse delta so the view doesn't jump after the
    /// transition.
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
