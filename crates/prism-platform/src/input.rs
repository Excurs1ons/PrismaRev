//! Winit → engine domain-type conversions, pointer-lock helpers, and
//! 输入 事件 routing.

use winit::event::WindowEvent;
use winit::window::Window;

/// 与窗口库无关的平台事件快照。
///
/// `prism-app` 和宿主扩展最终应消费此类型，而不是直接依赖 winit 的
/// `WindowEvent`。暂时保留原始事件路由 API，以便分阶段迁移调用方。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlatformEvent {
    CloseRequested,
    Focused(bool),
    Resized {
        width: u32,
        height: u32,
    },
    CursorMoved {
        x: f64,
        y: f64,
    },
    MouseButton {
        button: u16,
        pressed: bool,
    },
    MouseWheel {
        /// 垂直滚动量，**单位是「滚轮行」**（一格传统滚轮 = 1.0）。
        ///
        /// winit 在部分平台/设备上给的是像素增量（触控板、precision 滚轮），
        /// 平台层已按 [`PIXELS_PER_SCROLL_LINE`] 折算成行，所以消费方可以
        /// 统一按「1.0 ≈ 一格」处理，不必知道来源设备。
        delta: f64,
    },
    Key {
        key: u32,
        pressed: bool,
    },
    Touch {
        /// 触点 id（同一根手指的手指按下到抬起保持不变，多点触控据此区分）。
        id: u64,
        x: f64,
        y: f64,
        phase: TouchPhase,
    },
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchPhase {
    Started,
    Moved,
    Ended,
    Cancelled,
}

/// 像素增量折算成滚轮行的换算常量（50 px ≈ 一格）。
///
/// winit 的 [`winit::event::MouseScrollDelta`] 有两个单位互不相同的变体：
/// 传统滚轮报 [`LineDelta`](winit::event::MouseScrollDelta::LineDelta)（一格
/// `1.0`），触控板/高精度滚轮报
/// [`PixelDelta`](winit::event::MouseScrollDelta::PixelDelta)（一格通常几十到
/// 上百像素）。两者直接混用会让同一份缩放在不同平台上快 60–120 倍，所以平台层
/// 在这里统一折算，`PlatformEvent::MouseWheel` 的 `delta` 与
/// `InputManager::scroll_delta` 只以「行」为准。
///
/// 50 是经验值（Windows 的 `WHEEL_DELTA` 与各平台触控板的常见步长都落在这个
/// 量级），不是任何平台的协议常量；只有「同一格滚轮在两种单位下折算结果一致」
/// 是必须保持的契约，调这个数值不会改变缩放曲线的手感定义。
pub const PIXELS_PER_SCROLL_LINE: f64 = 50.0;

/// 把 winit 的像素级滚动增量折算成滚轮行（保留小数，触控板滑动平滑累积）。
pub fn scroll_lines_from_pixels(pixels: f64) -> f64 {
    pixels / PIXELS_PER_SCROLL_LINE
}

/// 将 winit 事件转换成平台无关事件。
pub fn to_platform_event(event: &WindowEvent) -> PlatformEvent {
    match event {
        WindowEvent::CloseRequested => PlatformEvent::CloseRequested,
        WindowEvent::Focused(focused) => PlatformEvent::Focused(*focused),
        WindowEvent::Resized(size) => PlatformEvent::Resized {
            width: size.width,
            height: size.height,
        },
        WindowEvent::CursorMoved { position, .. } => PlatformEvent::CursorMoved {
            x: position.x,
            y: position.y,
        },
        WindowEvent::MouseInput { state, button, .. } => PlatformEvent::MouseButton {
            button: match button {
                winit::event::MouseButton::Left => 0,
                winit::event::MouseButton::Right => 1,
                winit::event::MouseButton::Middle => 2,
                winit::event::MouseButton::Back => 3,
                winit::event::MouseButton::Forward => 4,
                winit::event::MouseButton::Other(v) => *v,
            },
            pressed: *state == winit::event::ElementState::Pressed,
        },
        WindowEvent::MouseWheel { delta, .. } => PlatformEvent::MouseWheel {
            delta: match delta {
                winit::event::MouseScrollDelta::LineDelta(_, y) => *y as f64,
                winit::event::MouseScrollDelta::PixelDelta(pos) => scroll_lines_from_pixels(pos.y),
            },
        },
        WindowEvent::KeyboardInput { event, .. } => PlatformEvent::Key {
            key: match event.physical_key {
                winit::keyboard::PhysicalKey::Code(code) => code as u32,
                winit::keyboard::PhysicalKey::Unidentified(_) => u32::MAX,
            },
            pressed: event.state == winit::event::ElementState::Pressed,
        },
        WindowEvent::Touch(touch) => PlatformEvent::Touch {
            id: touch.id,
            x: touch.location.x,
            y: touch.location.y,
            phase: match touch.phase {
                winit::event::TouchPhase::Started => TouchPhase::Started,
                winit::event::TouchPhase::Moved => TouchPhase::Moved,
                winit::event::TouchPhase::Ended => TouchPhase::Ended,
                winit::event::TouchPhase::Cancelled => TouchPhase::Cancelled,
            },
        },
        _ => PlatformEvent::Other,
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

#[cfg(test)]
mod tests {
    use super::*;
    use winit::dpi::PhysicalSize;

    #[test]
    fn converts_resize_without_exposing_winit_types() {
        let event = WindowEvent::Resized(PhysicalSize::new(1280, 720));
        assert_eq!(
            to_platform_event(&event),
            PlatformEvent::Resized {
                width: 1280,
                height: 720
            }
        );
    }

    #[test]
    fn converts_close_request() {
        assert_eq!(
            to_platform_event(&WindowEvent::CloseRequested),
            PlatformEvent::CloseRequested
        );
    }

    #[test]
    fn converts_touch_with_pointer_id() {
        let event = WindowEvent::Touch(winit::event::Touch {
            device_id: winit::event::DeviceId::dummy(),
            phase: winit::event::TouchPhase::Started,
            location: winit::dpi::PhysicalPosition::new(12.5, 34.0),
            force: None,
            id: 7,
        });
        assert_eq!(
            to_platform_event(&event),
            PlatformEvent::Touch {
                id: 7,
                x: 12.5,
                y: 34.0,
                phase: TouchPhase::Started,
            }
        );
    }

    #[test]
    fn keeps_touch_phase_and_distinct_pointer_ids() {
        let make = |phase, id| {
            WindowEvent::Touch(winit::event::Touch {
                device_id: winit::event::DeviceId::dummy(),
                phase,
                location: winit::dpi::PhysicalPosition::new(1.0, 2.0),
                force: Some(winit::event::Force::Normalized(0.5)),
                id,
            })
        };
        assert_eq!(
            to_platform_event(&make(winit::event::TouchPhase::Moved, 1)),
            PlatformEvent::Touch {
                id: 1,
                x: 1.0,
                y: 2.0,
                phase: TouchPhase::Moved,
            }
        );
        assert_eq!(
            to_platform_event(&make(winit::event::TouchPhase::Cancelled, 2)),
            PlatformEvent::Touch {
                id: 2,
                x: 1.0,
                y: 2.0,
                phase: TouchPhase::Cancelled,
            }
        );
    }

    fn wheel(delta: winit::event::MouseScrollDelta) -> PlatformEvent {
        to_platform_event(&WindowEvent::MouseWheel {
            device_id: winit::event::DeviceId::dummy(),
            delta,
            phase: winit::event::TouchPhase::Moved,
        })
    }

    fn wheel_lines(delta: winit::event::MouseScrollDelta) -> f64 {
        match wheel(delta) {
            PlatformEvent::MouseWheel { delta } => delta,
            other => panic!("expected a wheel event, got {other:?}"),
        }
    }

    #[test]
    fn converts_line_delta_wheel_notches_unchanged() {
        use winit::event::MouseScrollDelta as W;
        assert_eq!(wheel_lines(W::LineDelta(0.0, 1.0)), 1.0);
        assert_eq!(wheel_lines(W::LineDelta(0.0, -2.0)), -2.0);
        // 高精度滚轮会报小数行，直接透传。
        assert_eq!(wheel_lines(W::LineDelta(0.0, 0.25)), 0.25);
    }

    #[test]
    fn converts_pixel_delta_wheel_into_lines() {
        use winit::dpi::PhysicalPosition;
        use winit::event::MouseScrollDelta as W;
        assert_eq!(
            wheel_lines(W::PixelDelta(PhysicalPosition::new(0.0, 100.0))),
            2.0
        );
        assert_eq!(
            wheel_lines(W::PixelDelta(PhysicalPosition::new(0.0, -75.0))),
            -1.5
        );
        assert_eq!(
            wheel_lines(W::PixelDelta(PhysicalPosition::new(0.0, 12.0))),
            12.0 / PIXELS_PER_SCROLL_LINE
        );
    }

    /// 回归守卫：同一格滚轮无论走哪个单位，引擎看到的都是 1.0 行，
    /// 否则 zoom/移动速度会按平台差 60–120 倍。
    #[test]
    fn one_notch_is_one_line_in_either_unit() {
        use winit::dpi::PhysicalPosition;
        use winit::event::MouseScrollDelta as W;
        let line = wheel_lines(W::LineDelta(0.0, 1.0));
        let pixel = wheel_lines(W::PixelDelta(PhysicalPosition::new(
            0.0,
            PIXELS_PER_SCROLL_LINE,
        )));
        assert!(
            (line - pixel).abs() < 1e-9,
            "line delta {line} and one line worth of pixels {pixel} must agree"
        );
    }
}
