//! 与窗口库无关的事件 / 按键类型。
//!
//! 两个窗口后端（winit、Win32）都把自己的原始事件翻译成这里的
//! [`PlatformEvent`]，`prism-app` 只消费这一层——所以关掉 `winit-backend`
//! 时不会有任何 winit 类型参与编译。
//!
//! [`Key`] 只建模引擎真正用到的物理键（WASD/QE/T、数字行、方向键、
//! 修饰键、F1–F12）；其余键落到 [`Key::Other`]，载荷是**后端原始码**
//! （winit 判别值 / Win32 虚拟键码 `VK_*`），不会被静默丢弃。

/// 物理按键（窗口库无关）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    KeyW,
    KeyA,
    KeyS,
    KeyD,
    KeyQ,
    KeyE,
    /// `T` —— PBR 调试用的色调映射切换快捷键。
    KeyT,
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
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    /// 未建模的物理键：载荷是**后端原始码**（winit 判别值 / Win32 `VK_*`）。
    Other(u32),
}

/// Win32 虚拟键码（`WM_KEYDOWN` / `WM_KEYUP` 的 `wParam`）。
///
/// 这里只抄了 [`Key::from_vk`] 用得到的常量，避免为了几个数字打开
/// `windows-sys` 的整组模块；数值与 `winuser.h` 一致。
mod vk {
    pub const VK_TAB: u32 = 0x09;
    pub const VK_RETURN: u32 = 0x0D;
    pub const VK_SHIFT: u32 = 0x10;
    pub const VK_CONTROL: u32 = 0x11;
    pub const VK_MENU: u32 = 0x12;
    pub const VK_ESCAPE: u32 = 0x1B;
    pub const VK_SPACE: u32 = 0x20;
    pub const VK_LEFT: u32 = 0x25;
    pub const VK_UP: u32 = 0x26;
    pub const VK_RIGHT: u32 = 0x27;
    pub const VK_DOWN: u32 = 0x28;
    pub const VK_LSHIFT: u32 = 0xA0;
    pub const VK_RSHIFT: u32 = 0xA1;
    pub const VK_LCONTROL: u32 = 0xA2;
    pub const VK_RCONTROL: u32 = 0xA3;
    pub const VK_LMENU: u32 = 0xA4;
    pub const VK_RMENU: u32 = 0xA5;
    pub const VK_F1: u32 = 0x70;
}

impl Key {
    /// Win32 虚拟键码 → 中立键。
    ///
    /// 纯数值表（不依赖 `windows-sys`），因此**在非 Windows 主机上也能单测**。
    ///
    /// 已知限制：`WM_KEYDOWN` 对 Shift/Ctrl/Alt 报的是通用 `VK_SHIFT` /
    /// `VK_CONTROL` / `VK_MENU`，左右无法区分，统一按左侧处理。
    pub fn from_vk(vk: u32) -> Self {
        match vk {
            vk::VK_TAB => Key::Tab,
            vk::VK_RETURN => Key::Enter,
            vk::VK_SHIFT | vk::VK_LSHIFT => Key::ShiftLeft,
            vk::VK_RSHIFT => Key::ShiftRight,
            vk::VK_CONTROL | vk::VK_LCONTROL => Key::ControlLeft,
            vk::VK_RCONTROL => Key::ControlRight,
            vk::VK_MENU | vk::VK_LMENU => Key::AltLeft,
            vk::VK_RMENU => Key::AltRight,
            vk::VK_ESCAPE => Key::Escape,
            vk::VK_SPACE => Key::Space,
            vk::VK_LEFT => Key::ArrowLeft,
            vk::VK_UP => Key::ArrowUp,
            vk::VK_RIGHT => Key::ArrowRight,
            vk::VK_DOWN => Key::ArrowDown,
            vk::VK_F1..=0x7B => match vk - vk::VK_F1 {
                0 => Key::F1,
                1 => Key::F2,
                2 => Key::F3,
                3 => Key::F4,
                4 => Key::F5,
                5 => Key::F6,
                6 => Key::F7,
                7 => Key::F8,
                8 => Key::F9,
                9 => Key::F10,
                10 => Key::F11,
                _ => Key::F12,
            },
            // 字母行：VK_* 就是 ASCII 大写码。
            0x30..=0x39 => match vk - 0x30 {
                0 => Key::Digit0,
                1 => Key::Digit1,
                2 => Key::Digit2,
                3 => Key::Digit3,
                4 => Key::Digit4,
                5 => Key::Digit5,
                6 => Key::Digit6,
                7 => Key::Digit7,
                8 => Key::Digit8,
                _ => Key::Digit9,
            },
            0x41..=0x5A => match vk {
                0x57 => Key::KeyW,
                0x41 => Key::KeyA,
                0x53 => Key::KeyS,
                0x44 => Key::KeyD,
                0x51 => Key::KeyQ,
                0x45 => Key::KeyE,
                0x54 => Key::KeyT,
                other => Key::Other(other),
            },
            other => Key::Other(other),
        }
    }

    /// 这个键对应的 Win32 虚拟键码。
    ///
    /// [`Key::Other`] 直接返回原始码；其余返回规范化的 `VK_*`
    /// （左右修饰键分别用 `VK_LSHIFT` / `VK_RSHIFT` 这类区分码）。
    /// 用途：引擎的 `KeyCode` 没有 F1–F12 之类的专用变体，向下传递时需要
    /// 一个稳定的数字载荷。
    pub fn to_vk(self) -> u32 {
        match self {
            Key::Tab => 0x09,
            Key::Enter => 0x0D,
            Key::ShiftLeft => 0xA0,
            Key::ShiftRight => 0xA1,
            Key::ControlLeft => 0xA2,
            Key::ControlRight => 0xA3,
            Key::AltLeft => 0xA4,
            Key::AltRight => 0xA5,
            Key::Escape => 0x1B,
            Key::Space => 0x20,
            Key::ArrowLeft => 0x25,
            Key::ArrowUp => 0x26,
            Key::ArrowRight => 0x27,
            Key::ArrowDown => 0x28,
            Key::Digit0 => 0x30,
            Key::Digit1 => 0x31,
            Key::Digit2 => 0x32,
            Key::Digit3 => 0x33,
            Key::Digit4 => 0x34,
            Key::Digit5 => 0x35,
            Key::Digit6 => 0x36,
            Key::Digit7 => 0x37,
            Key::Digit8 => 0x38,
            Key::Digit9 => 0x39,
            Key::KeyA => 0x41,
            Key::KeyD => 0x44,
            Key::KeyE => 0x45,
            Key::KeyQ => 0x51,
            Key::KeyS => 0x53,
            Key::KeyT => 0x54,
            Key::KeyW => 0x57,
            Key::F1 => 0x70,
            Key::F2 => 0x71,
            Key::F3 => 0x72,
            Key::F4 => 0x73,
            Key::F5 => 0x74,
            Key::F6 => 0x75,
            Key::F7 => 0x76,
            Key::F8 => 0x77,
            Key::F9 => 0x78,
            Key::F10 => 0x79,
            Key::F11 => 0x7A,
            Key::F12 => 0x7B,
            Key::Other(code) => code,
        }
    }
}

/// 修饰键状态（`WindowEvent::ModifiersChanged` / `GetKeyState` 的抽象）。
///
/// `KeyboardInput` / `WM_KEYDOWN` 自身都不携带修饰键，PBR 分量调试快捷键
/// （Shift+1..5）只能靠这份状态区分。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
}

impl Modifiers {
    pub const fn empty() -> Self {
        Self {
            shift: false,
            control: false,
            alt: false,
        }
    }
}

/// 与窗口库无关的平台事件快照。
///
/// `prism-app` 和宿主扩展最终应消费此类型，而不是直接依赖 winit 的
/// `WindowEvent`。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlatformEvent {
    CloseRequested,
    Focused(bool),
    Resized {
        width: u32,
        height: u32,
    },
    CursorMoved {
        /// 窗口客户区内的物理像素坐标。
        x: f64,
        y: f64,
    },
    /// 指针锁定下的**原始设备增量**（相对位移，不是绝对坐标）。
    ///
    /// winit 后端来自 `DeviceEvent::MouseMotion`，Win32 后端来自
    /// 「隐藏 + 每帧回中」的鼠标捕获。只有这一个是增量语义。
    MouseMotion {
        dx: f64,
        dy: f64,
    },
    MouseButton {
        button: u16,
        pressed: bool,
    },
    /// 垂直滚动量，**单位是「滚轮行」**（一格传统滚轮 = 1.0）。
    ///
    /// winit 在部分平台/设备上给的是像素增量（触控板、precision 滚轮），
    /// 平台层已按 [`PIXELS_PER_SCROLL_LINE`] 折算成行，所以消费方可以
    /// 统一按「1.0 ≈ 一格」处理，不必知道来源设备。
    MouseWheel {
        delta: f64,
    },
    Key {
        key: Key,
        pressed: bool,
    },
    /// 修饰键状态变化（Shift/Ctrl/Alt 的当前快照）。
    Modifiers(Modifiers),
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
/// winit 的 [`MouseScrollDelta`](https://docs.rs/winit/latest/winit/event/enum.MouseScrollDelta.html)
/// 有两个单位互不相同的变体：
/// 传统滚轮报 `LineDelta`（一格 `1.0`），触控板/高精度滚轮报 `PixelDelta`
/// （一格通常几十到上百像素）。两者直接混用会让同一份缩放在不同平台上快
/// 60–120 倍，所以平台层在这里统一折算，`PlatformEvent::MouseWheel` 的
/// `delta` 与 `InputManager::scroll_delta` 只以「行」为准。
///
/// 50 是经验值（Windows 的 `WHEEL_DELTA` 与各平台触控板的常见步长都落在这个
/// 量级），不是任何平台的协议常量；只有「同一格滚轮在两种单位下折算结果一致」
/// 是必须保持的契约，调这个数值不会改变缩放曲线的手感定义。
pub const PIXELS_PER_SCROLL_LINE: f64 = 50.0;

/// 把像素级滚动增量折算成滚轮行（保留小数，触控板滑动平滑累积）。
pub fn scroll_lines_from_pixels(pixels: f64) -> f64 {
    pixels / PIXELS_PER_SCROLL_LINE
}

#[cfg(test)]
mod tests {
    use super::*;

    /// VK 映射是手工表：抽查移动/修饰/功能/数字键。
    #[test]
    fn win32_virtual_keys_map_to_native_keys() {
        assert_eq!(Key::from_vk(0x57), Key::KeyW);
        assert_eq!(Key::from_vk(0x41), Key::KeyA);
        assert_eq!(Key::from_vk(0x53), Key::KeyS);
        assert_eq!(Key::from_vk(0x44), Key::KeyD);
        assert_eq!(Key::from_vk(0x51), Key::KeyQ);
        assert_eq!(Key::from_vk(0x45), Key::KeyE);
        assert_eq!(Key::from_vk(0x54), Key::KeyT);
        assert_eq!(Key::from_vk(0x20), Key::Space);
        assert_eq!(Key::from_vk(0x10), Key::ShiftLeft);
        assert_eq!(Key::from_vk(0xA1), Key::ShiftRight);
        assert_eq!(Key::from_vk(0x11), Key::ControlLeft);
        assert_eq!(Key::from_vk(0x1B), Key::Escape);
        assert_eq!(Key::from_vk(0x09), Key::Tab);
        assert_eq!(Key::from_vk(0x0D), Key::Enter);
        assert_eq!(Key::from_vk(0x25), Key::ArrowLeft);
        assert_eq!(Key::from_vk(0x28), Key::ArrowDown);
        for (vk, expected) in [
            (0x30u32, Key::Digit0),
            (0x31, Key::Digit1),
            (0x35, Key::Digit5),
            (0x39, Key::Digit9),
        ] {
            assert_eq!(Key::from_vk(vk), expected, "VK 0x{vk:02x}");
        }
        assert_eq!(Key::from_vk(0x72), Key::F3);
        assert_eq!(Key::from_vk(0x7B), Key::F12);
    }

    /// `to_vk` 与 `from_vk` 必须互为逆映射（`prism-app` 用 `to_vk` 给引擎的
    /// `KeyCode::Other` 造数字载荷，映射漂移会让键被认错）。
    #[test]
    fn key_round_trips_through_virtual_key_codes() {
        for key in [
            Key::KeyW,
            Key::KeyA,
            Key::KeyS,
            Key::KeyD,
            Key::KeyQ,
            Key::KeyE,
            Key::KeyT,
            Key::Space,
            Key::ShiftLeft,
            Key::ShiftRight,
            Key::ControlLeft,
            Key::ControlRight,
            Key::AltLeft,
            Key::AltRight,
            Key::Escape,
            Key::Tab,
            Key::Enter,
            Key::ArrowUp,
            Key::ArrowDown,
            Key::ArrowLeft,
            Key::ArrowRight,
            Key::Digit0,
            Key::Digit1,
            Key::Digit2,
            Key::Digit3,
            Key::Digit4,
            Key::Digit5,
            Key::Digit6,
            Key::Digit7,
            Key::Digit8,
            Key::Digit9,
            Key::F1,
            Key::F6,
            Key::F12,
        ] {
            assert_eq!(Key::from_vk(key.to_vk()), key, "{key:?}");
        }
        assert_eq!(Key::Other(0x24).to_vk(), 0x24);
    }

    /// 未建模的键不能丢：退化为后端原始码。
    #[test]
    fn unmapped_virtual_keys_fall_back_to_raw_code() {
        assert_eq!(Key::from_vk(0x5A), Key::Other(0x5A)); // 'Z'
        assert_eq!(Key::from_vk(0x24), Key::Other(0x24)); // VK_HOME
        assert_eq!(Key::from_vk(0x7C), Key::Other(0x7C)); // VK_F13
    }

    /// 回归守卫：同一格滚轮无论走哪个单位，引擎看到的都是 1.0 行，
    /// 否则 zoom/移动速度会按平台差 60–120 倍。
    #[test]
    fn one_notch_is_one_line_in_either_unit() {
        let line = 1.0;
        let pixel = scroll_lines_from_pixels(PIXELS_PER_SCROLL_LINE);
        assert!(
            (line - pixel).abs() < 1e-9,
            "line delta {line} and one line worth of pixels {pixel} must agree"
        );
        assert_eq!(scroll_lines_from_pixels(100.0), 2.0);
        assert_eq!(scroll_lines_from_pixels(-75.0), -1.5);
    }
}
