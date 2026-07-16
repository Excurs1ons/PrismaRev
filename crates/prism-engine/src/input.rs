use winit::event::{ElementState, MouseScrollDelta, TouchPhase};
use winit::keyboard::{KeyCode as WinitKeyCode, PhysicalKey};

/// Abstract key code (maps to winit PhysicalKey for keyboard).
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

impl From<PhysicalKey> for KeyCode {
    fn from(pk: PhysicalKey) -> Self {
        match pk {
            PhysicalKey::Code(c) => match c {
                WinitKeyCode::KeyW => Self::KeyW,
                WinitKeyCode::KeyA => Self::KeyA,
                WinitKeyCode::KeyS => Self::KeyS,
                WinitKeyCode::KeyD => Self::KeyD,
                WinitKeyCode::KeyQ => Self::KeyQ,
                WinitKeyCode::KeyE => Self::KeyE,
                WinitKeyCode::Space => Self::Space,
                WinitKeyCode::ShiftLeft => Self::ShiftLeft,
                WinitKeyCode::ShiftRight => Self::ShiftRight,
                WinitKeyCode::ControlLeft => Self::ControlLeft,
                WinitKeyCode::ControlRight => Self::ControlRight,
                WinitKeyCode::Escape => Self::Escape,
                WinitKeyCode::Tab => Self::Tab,
                WinitKeyCode::Enter => Self::Enter,
                WinitKeyCode::ArrowUp => Self::ArrowUp,
                WinitKeyCode::ArrowDown => Self::ArrowDown,
                WinitKeyCode::ArrowLeft => Self::ArrowLeft,
                WinitKeyCode::ArrowRight => Self::ArrowRight,
                WinitKeyCode::Digit0 => Self::Digit0,
                WinitKeyCode::Digit1 => Self::Digit1,
                WinitKeyCode::Digit2 => Self::Digit2,
                WinitKeyCode::Digit3 => Self::Digit3,
                WinitKeyCode::Digit4 => Self::Digit4,
                WinitKeyCode::Digit5 => Self::Digit5,
                WinitKeyCode::Digit6 => Self::Digit6,
                WinitKeyCode::Digit7 => Self::Digit7,
                WinitKeyCode::Digit8 => Self::Digit8,
                WinitKeyCode::Digit9 => Self::Digit9,
                _ => Self::Other(c as u32),
            },
            PhysicalKey::Unidentified(_) => Self::Other(0),
        }
    }
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

impl From<winit::event::MouseButton> for MouseButton {
    fn from(b: winit::event::MouseButton) -> Self {
        match b {
            winit::event::MouseButton::Left => Self::Left,
            winit::event::MouseButton::Right => Self::Right,
            winit::event::MouseButton::Middle => Self::Middle,
            winit::event::MouseButton::Back => Self::Back,
            winit::event::MouseButton::Forward => Self::Forward,
            winit::event::MouseButton::Other(val) => Self::Other(val),
        }
    }
}

/// A single touch event (for mobile support).
#[derive(Clone, Copy, Debug)]
pub struct TouchEvent {
    pub id: u64,
    pub phase: TouchPhase,
    pub position: [f64; 2],
}

/// Per-frame input snapshot (ECS Resource).
#[derive(Default)]
pub struct InputState {
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
}

impl InputState {
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

    // --- Event handlers (called by App) ---
    pub fn handle_keyboard(&mut self, physical_key: PhysicalKey, state: ElementState) {
        let key = KeyCode::from(physical_key);
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
    ///
    /// Call on touch-start so the first subsequent move produces a correct
    /// delta instead of a jump from the last known position.
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

    pub fn handle_scroll(&mut self, delta: MouseScrollDelta) {
        match delta {
            MouseScrollDelta::LineDelta(_x, y) => self.scroll_delta += y as f64,
            MouseScrollDelta::PixelDelta(pos) => self.scroll_delta += pos.y,
        }
    }

    pub fn handle_touch(&mut self, id: u64, phase: TouchPhase, position: [f64; 2]) {
        self.touches.push(TouchEvent {
            id,
            phase,
            position,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::event::{ElementState, MouseScrollDelta, TouchPhase};
    use winit::keyboard::{KeyCode as WinitKeyCode, NativeKeyCode, PhysicalKey};

    fn phys(key: WinitKeyCode) -> PhysicalKey {
        PhysicalKey::Code(key)
    }

    #[test]
    fn new_is_empty() {
        let s = InputState::new();
        assert!(!s.key_held(KeyCode::KeyW));
        assert!(!s.mouse_held(MouseButton::Left));
        assert_eq!(s.mouse_delta(), [0.0; 2]);
        assert_eq!(s.scroll_delta(), 0.0);
        assert_eq!(s.mouse_position(), [0.0; 2]);
        assert!(s.touches().is_empty());
    }

    #[test]
    fn key_press_adds_held_and_just_pressed() {
        let mut s = InputState::new();
        s.handle_keyboard(phys(WinitKeyCode::KeyW), ElementState::Pressed);
        assert!(s.key_held(KeyCode::KeyW));
        assert!(s.key_just_pressed(KeyCode::KeyW));
    }

    #[test]
    fn key_held_survives_begin_frame() {
        let mut s = InputState::new();
        s.handle_keyboard(phys(WinitKeyCode::KeyW), ElementState::Pressed);
        s.begin_frame();
        assert!(s.key_held(KeyCode::KeyW));
        assert!(!s.key_just_pressed(KeyCode::KeyW)); // transient cleared
    }

    #[test]
    fn key_just_released_on_release() {
        let mut s = InputState::new();
        s.handle_keyboard(phys(WinitKeyCode::KeyW), ElementState::Pressed);
        s.begin_frame();
        s.handle_keyboard(phys(WinitKeyCode::KeyW), ElementState::Released);
        assert!(!s.key_held(KeyCode::KeyW));
        assert!(s.key_just_released(KeyCode::KeyW));
    }

    #[test]
    fn duplicate_key_press_does_not_double_just_pressed() {
        let mut s = InputState::new();
        s.handle_keyboard(phys(WinitKeyCode::KeyW), ElementState::Pressed);
        s.handle_keyboard(phys(WinitKeyCode::KeyW), ElementState::Pressed); // duplicate
        assert!(s.key_held(KeyCode::KeyW));
        assert_eq!(s.keys_just_pressed.len(), 1); // only once
    }

    #[test]
    fn mouse_delta_accumulates_and_resets() {
        let mut s = InputState::new();
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
    fn scroll_line_delta() {
        let mut s = InputState::new();
        s.handle_scroll(MouseScrollDelta::LineDelta(0.0, 3.0));
        assert!((s.scroll_delta() - 3.0).abs() < 1e-9);
        s.handle_scroll(MouseScrollDelta::LineDelta(0.0, -1.0));
        assert!((s.scroll_delta() - 2.0).abs() < 1e-9); // accumulated
    }

    #[test]
    fn scroll_pixel_delta() {
        let mut s = InputState::new();
        s.handle_scroll(MouseScrollDelta::PixelDelta(
            winit::dpi::PhysicalPosition::new(0.0, 42.0),
        ));
        assert!((s.scroll_delta() - 42.0).abs() < 1e-9);
    }

    #[test]
    fn mouse_button_held_and_just_pressed() {
        let mut s = InputState::new();
        s.handle_mouse_button(MouseButton::Left, ElementState::Pressed);
        assert!(s.mouse_held(MouseButton::Left));

        s.begin_frame();
        assert!(s.mouse_held(MouseButton::Left));
        assert_eq!(s.mouse_just_pressed.len(), 0); // transient cleared
    }

    #[test]
    fn mouse_button_release_clears_held() {
        let mut s = InputState::new();
        s.handle_mouse_button(MouseButton::Left, ElementState::Pressed);
        s.begin_frame();
        s.handle_mouse_button(MouseButton::Left, ElementState::Released);
        assert!(!s.mouse_held(MouseButton::Left));
    }

    #[test]
    fn touch_events_accumulate_and_clear() {
        let mut s = InputState::new();
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
        let mut s = InputState::new();
        s.handle_keyboard(phys(WinitKeyCode::Space), ElementState::Pressed);
        s.handle_mouse_button(MouseButton::Right, ElementState::Pressed);
        s.handle_mouse_move([50.0, 60.0]);
        s.handle_scroll(MouseScrollDelta::LineDelta(0.0, 5.0));

        s.begin_frame();
        assert!(!s.key_just_pressed(KeyCode::Space));
        assert!(s.key_held(KeyCode::Space)); // held persists
        assert_eq!(s.mouse_delta(), [0.0; 2]);
        assert_eq!(s.scroll_delta(), 0.0);
    }

    #[test]
    fn key_code_from_physical_all_mapped() {
        let cases = [
            (WinitKeyCode::KeyW, KeyCode::KeyW),
            (WinitKeyCode::KeyA, KeyCode::KeyA),
            (WinitKeyCode::KeyS, KeyCode::KeyS),
            (WinitKeyCode::KeyD, KeyCode::KeyD),
            (WinitKeyCode::KeyQ, KeyCode::KeyQ),
            (WinitKeyCode::KeyE, KeyCode::KeyE),
            (WinitKeyCode::Space, KeyCode::Space),
            (WinitKeyCode::ShiftLeft, KeyCode::ShiftLeft),
            (WinitKeyCode::ShiftRight, KeyCode::ShiftRight),
            (WinitKeyCode::ControlLeft, KeyCode::ControlLeft),
            (WinitKeyCode::ControlRight, KeyCode::ControlRight),
            (WinitKeyCode::Escape, KeyCode::Escape),
            (WinitKeyCode::Tab, KeyCode::Tab),
            (WinitKeyCode::Enter, KeyCode::Enter),
            (WinitKeyCode::ArrowUp, KeyCode::ArrowUp),
            (WinitKeyCode::ArrowDown, KeyCode::ArrowDown),
            (WinitKeyCode::ArrowLeft, KeyCode::ArrowLeft),
            (WinitKeyCode::ArrowRight, KeyCode::ArrowRight),
            (WinitKeyCode::Digit0, KeyCode::Digit0),
            (WinitKeyCode::Digit1, KeyCode::Digit1),
            (WinitKeyCode::Digit2, KeyCode::Digit2),
            (WinitKeyCode::Digit3, KeyCode::Digit3),
            (WinitKeyCode::Digit4, KeyCode::Digit4),
            (WinitKeyCode::Digit5, KeyCode::Digit5),
            (WinitKeyCode::Digit6, KeyCode::Digit6),
            (WinitKeyCode::Digit7, KeyCode::Digit7),
            (WinitKeyCode::Digit8, KeyCode::Digit8),
            (WinitKeyCode::Digit9, KeyCode::Digit9),
        ];
        for (winit_key, expected) in &cases {
            let pk = PhysicalKey::Code(*winit_key);
            assert_eq!(KeyCode::from(pk), *expected, "mismatch for {:?}", winit_key);
        }
    }

    #[test]
    fn key_code_unknown_winit_key_maps_to_other() {
        let pk = PhysicalKey::Code(WinitKeyCode::F1); // not in our enum
        let kc = KeyCode::from(pk);
        assert_eq!(kc, KeyCode::Other(WinitKeyCode::F1 as u32));
    }

    #[test]
    fn key_code_unidentified_maps_to_other_zero() {
        let pk = PhysicalKey::Unidentified(NativeKeyCode::Unidentified);
        assert_eq!(KeyCode::from(pk), KeyCode::Other(0));
    }

    #[test]
    fn mouse_button_from_winit_all_mapped() {
        assert_eq!(
            MouseButton::from(winit::event::MouseButton::Left),
            MouseButton::Left
        );
        assert_eq!(
            MouseButton::from(winit::event::MouseButton::Right),
            MouseButton::Right
        );
        assert_eq!(
            MouseButton::from(winit::event::MouseButton::Middle),
            MouseButton::Middle
        );
        let back = MouseButton::from(winit::event::MouseButton::Back);
        assert_eq!(back, MouseButton::Back);
    }
}
