//! Winit → engine domain-type conversions, pointer-lock helpers, and
//! 输入 事件 routing.

use prism_engine::input::{InputManager, KeyCode as EngKeyCode, MouseButton as EngMouseButton};
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

// ===========================================================================
// Winit → engine domain-type conversions
// ===========================================================================

/// 转换 a winit [`PhysicalKey`] to an engine [`KeyCode`].
pub fn winit_key_to_engine(pk: winit::keyboard::PhysicalKey) -> EngKeyCode {
    use winit::keyboard::KeyCode as Wk;
    match pk {
        winit::keyboard::PhysicalKey::Code(c) => match c {
            Wk::KeyW => EngKeyCode::KeyW,
            Wk::KeyA => EngKeyCode::KeyA,
            Wk::KeyS => EngKeyCode::KeyS,
            Wk::KeyD => EngKeyCode::KeyD,
            Wk::KeyQ => EngKeyCode::KeyQ,
            Wk::KeyE => EngKeyCode::KeyE,
            Wk::Space => EngKeyCode::Space,
            Wk::ShiftLeft => EngKeyCode::ShiftLeft,
            Wk::ShiftRight => EngKeyCode::ShiftRight,
            Wk::ControlLeft => EngKeyCode::ControlLeft,
            Wk::ControlRight => EngKeyCode::ControlRight,
            Wk::AltLeft => EngKeyCode::AltLeft,
            Wk::AltRight => EngKeyCode::AltRight,
            Wk::Escape => EngKeyCode::Escape,
            Wk::Tab => EngKeyCode::Tab,
            Wk::Enter => EngKeyCode::Enter,
            Wk::ArrowUp => EngKeyCode::ArrowUp,
            Wk::ArrowDown => EngKeyCode::ArrowDown,
            Wk::ArrowLeft => EngKeyCode::ArrowLeft,
            Wk::ArrowRight => EngKeyCode::ArrowRight,
            Wk::Digit0 => EngKeyCode::Digit0,
            Wk::Digit1 => EngKeyCode::Digit1,
            Wk::Digit2 => EngKeyCode::Digit2,
            Wk::Digit3 => EngKeyCode::Digit3,
            Wk::Digit4 => EngKeyCode::Digit4,
            Wk::Digit5 => EngKeyCode::Digit5,
            Wk::Digit6 => EngKeyCode::Digit6,
            Wk::Digit7 => EngKeyCode::Digit7,
            Wk::Digit8 => EngKeyCode::Digit8,
            Wk::Digit9 => EngKeyCode::Digit9,
            _ => EngKeyCode::Other(c as u32),
        },
        winit::keyboard::PhysicalKey::Unidentified(_) => EngKeyCode::Other(0),
    }
}

/// 转换 a winit [`MouseButton`] to an engine [`MouseButton`].
pub fn winit_mouse_button_to_engine(b: winit::event::MouseButton) -> EngMouseButton {
    match b {
        winit::event::MouseButton::Left => EngMouseButton::Left,
        winit::event::MouseButton::Right => EngMouseButton::Right,
        winit::event::MouseButton::Middle => EngMouseButton::Middle,
        winit::event::MouseButton::Back => EngMouseButton::Back,
        winit::event::MouseButton::Forward => EngMouseButton::Forward,
        winit::event::MouseButton::Other(v) => EngMouseButton::Other(v),
    }
}

/// 转换 a winit [`ElementState`] to an engine [`ElementState`].
pub fn winit_state_to_engine(s: winit::event::ElementState) -> prism_engine::input::ElementState {
    match s {
        winit::event::ElementState::Pressed => prism_engine::input::ElementState::Pressed,
        winit::event::ElementState::Released => prism_engine::input::ElementState::Released,
    }
}

// ===========================================================================
// Pointer-lock helpers (cursor grab / 可见性
// ===========================================================================

/// Grab the cursor (hide + confine) for FPS-style 指针 lock.
pub fn grab_pointer(window: &Window) {
    window.set_cursor_visible(false);
    if let Err(e) = window.set_cursor_grab(winit::window::CursorGrabMode::Confined) {
        log::warn!("failed to grab cursor (pointer lock): {e}");
    }
}

/// 释放 the cursor (show + unconfine).
pub fn release_pointer(window: &Window) {
    window.set_cursor_visible(true);
    if let Err(e) = window.set_cursor_grab(winit::window::CursorGrabMode::None) {
        log::warn!("failed to release cursor grab: {e}");
    }
}

// ===========================================================================
// 输入 事件 routing (winit → engine)
// ===========================================================================

/// Route a winit [`WindowEvent`] through the engine's [`InputManager`],
/// updating raw 输入 状态 and managing pointer-lock transitions that require
/// window-system operations.
pub fn handle_input_event(
    input: &mut InputManager,
    window: &Window,
    event_loop: &ActiveEventLoop,
    event: &WindowEvent,
) {
    match event {
        WindowEvent::CloseRequested => {
            log::info!("close requested, exiting");
            event_loop.exit();
        }

        WindowEvent::Focused(false) => {
            input.focus_return_click = false;
            if input.pointer_locked {
                release_pointer(window);
                input.set_locked(false);
            }
        }
        WindowEvent::Focused(true) => {
            input.focus_return_click = true;
            if input.pointer_locked {
                release_pointer(window);
                input.set_locked(false);
            }
        }

        WindowEvent::MouseInput { state, button, .. } => {
            if *state == winit::event::ElementState::Pressed
                && *button == winit::event::MouseButton::Left
                && !input.pointer_locked
                && input.focus_return_click
            {
                input.focus_return_click = false;
            } else if *state == winit::event::ElementState::Pressed
                && *button == winit::event::MouseButton::Left
                && !input.pointer_locked
            {
                grab_pointer(window);
                input.set_locked(true);
                return;
            }
            input.handle_mouse_button(
                winit_mouse_button_to_engine(*button),
                winit_state_to_engine(*state),
            );
        }

        WindowEvent::CursorMoved { position, .. } => {
            input.handle_mouse_move([position.x, position.y]);
        }

        WindowEvent::MouseWheel { delta, .. } => match delta {
            winit::event::MouseScrollDelta::LineDelta(_x, y) => {
                input.handle_scroll(*y as f64);
            }
            winit::event::MouseScrollDelta::PixelDelta(pos) => {
                input.handle_scroll(pos.y);
            }
        },

        WindowEvent::Touch(touch) => {
            let pos = [touch.location.x, touch.location.y];
            match touch.phase {
                winit::event::TouchPhase::Started => {
                    input.set_mouse_position(pos);
                    input.handle_mouse_button(
                        EngMouseButton::Left,
                        prism_engine::input::ElementState::Pressed,
                    );
                }
                winit::event::TouchPhase::Moved => {
                    input.handle_mouse_move(pos);
                }
                winit::event::TouchPhase::Ended | winit::event::TouchPhase::Cancelled => {
                    input.handle_mouse_button(
                        EngMouseButton::Left,
                        prism_engine::input::ElementState::Released,
                    );
                }
            }
        }

        WindowEvent::KeyboardInput {
            event:
                winit::event::KeyEvent {
                    physical_key,
                    state,
                    ..
                },
            ..
        } => {
            if *state == winit::event::ElementState::Pressed {
                if let winit::keyboard::PhysicalKey::Code(code) = physical_key {
                    if *code == winit::keyboard::KeyCode::Escape {
                        if input.pointer_locked {
                            release_pointer(window);
                            input.set_locked(false);
                            input.alt_temp_release = false;
                        }
                        input.handle_keyboard(
                            winit_key_to_engine(*physical_key),
                            winit_state_to_engine(*state),
                        );
                        return;
                    }
                    if *code == winit::keyboard::KeyCode::AltLeft
                        || *code == winit::keyboard::KeyCode::AltRight
                    {
                        if input.pointer_locked {
                            release_pointer(window);
                            input.set_locked(false);
                            input.alt_temp_release = true;
                        }
                        input.handle_keyboard(
                            winit_key_to_engine(*physical_key),
                            winit_state_to_engine(*state),
                        );
                        return;
                    }
                }
            }
            if *state == winit::event::ElementState::Released {
                if let winit::keyboard::PhysicalKey::Code(code) = physical_key {
                    if (*code == winit::keyboard::KeyCode::AltLeft
                        || *code == winit::keyboard::KeyCode::AltRight)
                        && input.alt_temp_release
                    {
                        grab_pointer(window);
                        input.set_locked(true);
                        input.alt_temp_release = false;
                    }
                }
            }
            input.handle_keyboard(
                winit_key_to_engine(*physical_key),
                winit_state_to_engine(*state),
            );
        }

        _ => {}
    }
}
