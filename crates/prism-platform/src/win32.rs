//! Win32 原生窗口后端（`feature = "win32-native"`，仅 Windows）。
//!
//! 目标：Windows 发布包不链接 winit。做法是最小化的 Win32 窗口 +
//! 消息循环——`RegisterClassExW` / `CreateWindowExW` / `PeekMessageW`
//! 家族，外加一个 `WndProc` 把 `WM_*` 翻译成 [`PlatformEvent`]。
//!
//! # 消息循环为什么不是阻塞的 `GetMessageW`
//!
//! 主线程必须**在没有窗口消息时也继续推进模拟**（否则静止画面会冻住：
//! winit 后端为此用 `ControlFlow::Poll`）。`GetMessageW` 在队列空时会无限
//! 阻塞，只有 `SetTimer` + `WM_TIMER` 能给它超时——而 `WM_TIMER` 的
//! 分辨率是系统时钟粒度（~15.6 ms），会把主循环钉在 ~64 Hz，直接给渲染
//! 帧率封顶。
//!
//! 所以这里用 MSDN 推荐给游戏循环的组合：
//! [`PlatformWindow::poll_events`]（`PeekMessageW` 泵，不阻塞）+
//! [`PlatformWindow::wait_events`]（`MsgWaitForMultipleObjectsEx`，有消息
//! 立即返回、否则最多等 `timeout_ms`）。
//!
//! # 明确不做（体积与风险换来的边界，别当 bug 报）
//!
//! - **无 IME / 文本输入**：`WM_CHAR` / `WM_IME_*` 全部忽略，[`Key`] 只表达物理键。
//! - **无拖放**：不注册 `DragAcceptFiles`，不会有 `WM_DROPFILES`。
//! - **无多显示器 DPI 平滑**：进程声明 Per-Monitor-V2，`WM_DPICHANGED` 只按系统
//!   建议的矩形调整窗口位置/大小（`scale_factor()` 读 `GetDpiForWindow`），
//!   但引擎不据此重排 ECS UI。
//! - **无任务栏进度 / 缩略图工具栏 / 跳转列表**：不碰 `ITaskbarList3`。
//! - **无触摸**：不产生 [`PlatformEvent::Touch`]，只有鼠标与键盘。
//! - **左右修饰键**：`WM_KEYDOWN` 报通用 `VK_SHIFT` / `VK_CONTROL` / `VK_MENU`，
//!   左右一律按左侧处理（见 [`crate::Key::from_vk`]）。
//! - **指针锁定**用「隐藏光标 + `ClipCursor` + 每圈把光标拉回客户区中心」
//!   实现：`WM_MOUSEMOVE` 的偏移就是 [`PlatformEvent::MouseMotion`]。
//!   没有 `WM_INPUT`（Raw Input），所以锁定期间不再产生 `CursorMoved`。
//! - **全屏**是 `WS_POPUP` + 主显示器尺寸，不做独占全屏模式切换。
//! - `min_width` / `max_width` 这类限制按**窗口**尺寸生效（`WM_GETMINMAXINFO`），
//!   而 winit 的对应配置是**客户区**尺寸——两者差一圈标题栏/边框。
//!
//! # 已验证程度
//!
//! 本模块**只做过交叉编译验证**（`--target x86_64-pc-windows-gnu`），
//! 没有在真实 Windows 设备上跑过。

use std::collections::VecDeque;
use std::ffi::c_void;
use std::mem::size_of;
use std::sync::{Arc, Mutex, MutexGuard};

use raw_window_handle::{RawDisplayHandle, RawWindowHandle, Win32WindowHandle};
use windows_sys::Win32::Foundation::{
    GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{
    ClientToScreen, InvalidateRect, UpdateWindow, ValidateRect,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::{
    GetDpiForSystem, GetDpiForWindow, SetProcessDpiAwarenessContext,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, VK_CONTROL, VK_MENU, VK_SHIFT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, ClipCursor, CreateWindowExW, DefWindowProcW, DestroyWindow,
    DispatchMessageW, GetClientRect, GetSystemMetrics, GetWindowLongPtrW, LoadCursorW,
    MsgWaitForMultipleObjectsEx, PeekMessageW, PostQuitMessage, RegisterClassExW, SetCursorPos,
    SetWindowLongPtrW, SetWindowPos, ShowCursor, ShowWindow, TranslateMessage, CREATESTRUCTW,
    CS_HREDRAW, CS_OWNDC, CS_VREDRAW, CW_USEDEFAULT, GWLP_USERDATA, IDC_ARROW, MINMAXINFO, MSG,
    MWMO_INPUTAVAILABLE, PM_REMOVE, QS_ALLINPUT, SM_CXSCREEN, SM_CYSCREEN, SWP_NOACTIVATE,
    SWP_NOZORDER, SW_HIDE, SW_MAXIMIZE, SW_SHOW, WM_CLOSE, WM_DESTROY, WM_DPICHANGED,
    WM_ERASEBKGND, WM_GETMINMAXINFO, WM_KEYDOWN, WM_KEYUP, WM_KILLFOCUS, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCCREATE, WM_PAINT,
    WM_QUIT, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SETCURSOR, WM_SETFOCUS, WM_SIZE, WM_SYSKEYDOWN,
    WM_SYSKEYUP, WM_XBUTTONDOWN, WM_XBUTTONUP, WNDCLASSEXW, WS_MAXIMIZEBOX, WS_OVERLAPPEDWINDOW,
    WS_POPUP, WS_THICKFRAME, WS_VISIBLE,
};

use crate::config::WindowConfig;
use crate::event::{Key, Modifiers, PlatformEvent};
use crate::window::PlatformWindow;

const WHEEL_DELTA: f32 = 120.0;
const CLASS_NAME: &str = "PrismaRevWindow";
/// 事件队列上限：窗口消息洪水（高 DPI 鼠标）时也不吃穿内存。
const MAX_QUEUED_EVENTS: usize = 4096;

/// 传给 `WndProc` 的动作——这些 API 会**同步再入** `WndProc`，
/// 所以必须在放开状态锁之后再调用。
enum PostAction {
    Capture,
    ReleaseCapture,
    ApplyDpiRect(RECT),
    /// 把光标拉回客户区中心（指针锁定的「回中」）。
    Recenter(POINT),
}

/// 指针锁定时的客户区中心：`client` 用来算增量（`WM_MOUSEMOVE` 给的是
/// **客户区**坐标），`screen` 用来 `SetCursorPos` 回中（屏幕坐标）。
#[derive(Clone, Copy)]
struct Grab {
    client: POINT,
    screen: POINT,
}

struct WindowState {
    events: VecDeque<PlatformEvent>,
    close_requested: bool,
    /// 指针锁定状态：`Some(..)` 表示「隐藏 + 回中」模式。
    grab: Option<Grab>,
    /// 光标是否由我们隐藏（`ShowCursor` 是计数器语义，只在翻转时调一次）。
    cursor_hidden: bool,
    dpi: u32,
    min_size: Option<(i32, i32)>,
    max_size: Option<(i32, i32)>,
}

impl WindowState {
    fn new(cfg: &WindowConfig) -> Self {
        let min_size = match (cfg.min_width, cfg.min_height) {
            (Some(w), Some(h)) => Some((w as i32, h as i32)),
            _ => None,
        };
        let max_size = match (cfg.max_width, cfg.max_height) {
            (Some(w), Some(h)) => Some((w as i32, h as i32)),
            _ => None,
        };
        Self {
            events: VecDeque::new(),
            close_requested: false,
            grab: None,
            cursor_hidden: false,
            dpi: 96,
            min_size,
            max_size,
        }
    }

    fn push(&mut self, event: PlatformEvent) {
        if self.events.len() < MAX_QUEUED_EVENTS {
            self.events.push_back(event);
        }
    }

    /// `GetKeyState` 的实时修饰键快照（`WM_KEYDOWN` 自身不带修饰键）。
    fn modifiers(&self) -> Modifiers {
        unsafe {
            Modifiers {
                shift: key_down(VK_SHIFT),
                control: key_down(VK_CONTROL),
                alt: key_down(VK_MENU),
            }
        }
    }
}

unsafe fn key_down(vk: u16) -> bool {
    unsafe { GetKeyState(vk as i32) < 0 }
}

fn lock(state: &Mutex<WindowState>) -> MutexGuard<'_, WindowState> {
    // `WndProc` 里没有会 panic 的逻辑；万一毒化了也继续用（丢掉窗口更糟）。
    state.lock().unwrap_or_else(|e| e.into_inner())
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn loword(value: usize) -> u16 {
    (value & 0xffff) as u16
}

fn hiword(value: usize) -> u16 {
    ((value >> 16) & 0xffff) as u16
}

/// 鼠标消息里的有符号客户区坐标（等价于 `GET_X_LPARAM` / `GET_Y_LPARAM`）。
fn signed_x(lparam: isize) -> i32 {
    (lparam as usize & 0xffff) as u16 as i16 as i32
}

fn signed_y(lparam: isize) -> i32 {
    ((lparam as usize >> 16) & 0xffff) as u16 as i16 as i32
}

/// 从 `GWLP_USERDATA` 取回窗口状态（`WM_NCCREATE` 时写入）。
unsafe fn state_of(hwnd: HWND) -> Option<&'static Mutex<WindowState>> {
    let raw = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const Mutex<WindowState>;
    unsafe { raw.as_ref() }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // 创建期：把 `CreateWindowExW` 的 `lpCreateParams`（Arc 的裸指针）存进 USERDATA。
    if msg == WM_NCCREATE {
        let create = unsafe { &*(lparam as *const CREATESTRUCTW) };
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize) };
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }

    let Some(state) = (unsafe { state_of(hwnd) }) else {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    };

    let mut post: Option<PostAction> = None;
    let mut handled = true;

    {
        let mut st = lock(state);
        match msg {
            WM_CLOSE => {
                st.close_requested = true;
                st.push(PlatformEvent::CloseRequested);
            }
            WM_DESTROY => unsafe { PostQuitMessage(0) },
            WM_SIZE => {
                let width = loword(lparam as usize) as u32;
                let height = hiword(lparam as usize) as u32;
                st.push(PlatformEvent::Resized { width, height });
                if st.grab.is_some() {
                    st.grab = unsafe { grab_center(hwnd) };
                }
            }
            WM_PAINT => {
                // 画面由 Vulkan 直接呈现，GDI 只需把脏区域标为有效。
                unsafe { ValidateRect(hwnd, std::ptr::null()) };
            }
            WM_ERASEBKGND => {
                // 已处理（等同返回 1）：不画背景，避免闪白。
            }
            WM_SETFOCUS => st.push(PlatformEvent::Focused(true)),
            WM_KILLFOCUS => st.push(PlatformEvent::Focused(false)),
            WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP => {
                let modifiers = st.modifiers();
                st.push(PlatformEvent::Modifiers(modifiers));
                st.push(PlatformEvent::Key {
                    key: Key::from_vk(wparam as u32),
                    pressed: msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN,
                });
            }
            WM_MOUSEMOVE => {
                let x = signed_x(lparam);
                let y = signed_y(lparam);
                match st.grab {
                    Some(grab) => {
                        // 锁定态：光标被按在客户区中心，偏移量就是原始增量
                        // （两边都是客户区坐标）。
                        let (dx, dy) = (x - grab.client.x, y - grab.client.y);
                        if dx != 0 || dy != 0 {
                            st.push(PlatformEvent::MouseMotion {
                                dx: dx as f64,
                                dy: dy as f64,
                            });
                            // 回中要放在放开状态锁之后：`SetCursorPos` 可能派发
                            // 新的鼠标消息，同线程重入 `WndProc` 会二次加锁。
                            // （回中会再发一条 dx = dy = 0 的移动，被上面的判断吃掉。）
                            post = Some(PostAction::Recenter(grab.screen));
                        }
                    }
                    None => st.push(PlatformEvent::CursorMoved {
                        x: x as f64,
                        y: y as f64,
                    }),
                }
            }
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN => {
                let button = match msg {
                    WM_LBUTTONDOWN => 0,
                    WM_RBUTTONDOWN => 1,
                    _ => 2,
                };
                st.push(PlatformEvent::MouseButton {
                    button,
                    pressed: true,
                });
                post = Some(PostAction::Capture);
            }
            WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP => {
                let button = match msg {
                    WM_LBUTTONUP => 0,
                    WM_RBUTTONUP => 1,
                    _ => 2,
                };
                st.push(PlatformEvent::MouseButton {
                    button,
                    pressed: false,
                });
                post = Some(PostAction::ReleaseCapture);
            }
            WM_XBUTTONDOWN | WM_XBUTTONUP => {
                // XBUTTON1 = 后退键, XBUTTON2 = 前进键。
                let button = if hiword(wparam) == 1 { 3 } else { 4 };
                st.push(PlatformEvent::MouseButton {
                    button,
                    pressed: msg == WM_XBUTTONDOWN,
                });
                post = Some(PostAction::Capture);
            }
            WM_MOUSEWHEEL => {
                // HIWORD(wParam) 是一格 120 的有符号整数倍。
                let notches = hiword(wparam) as i16 as f32 / WHEEL_DELTA;
                st.push(PlatformEvent::MouseWheel {
                    delta: notches as f64,
                });
            }
            WM_SETCURSOR => {
                // 隐藏光标时别让 DC 把类光标画回来（返回 1 = 已处理）。
                if !st.cursor_hidden {
                    handled = false;
                }
            }
            WM_GETMINMAXINFO => {
                let info = unsafe { &mut *(lparam as *mut MINMAXINFO) };
                if let Some((w, h)) = st.min_size {
                    info.ptMinTrackSize.x = w;
                    info.ptMinTrackSize.y = h;
                }
                if let Some((w, h)) = st.max_size {
                    info.ptMaxTrackSize.x = w;
                    info.ptMaxTrackSize.y = h;
                }
            }
            WM_DPICHANGED => {
                st.dpi = hiword(wparam) as u32;
                // 按系统建议的矩形走（不做内容缩放，见模块文档）。
                post = Some(PostAction::ApplyDpiRect(unsafe {
                    *(lparam as *const RECT)
                }));
            }
            _ => handled = false,
        }
    }

    if !handled {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }

    match post {
        Some(PostAction::Capture) => {
            unsafe { SetCapture(hwnd) };
        }
        Some(PostAction::ReleaseCapture) => unsafe {
            ReleaseCapture();
        },
        Some(PostAction::ApplyDpiRect(rect)) => unsafe {
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        },
        Some(PostAction::Recenter(center)) => unsafe {
            SetCursorPos(center.x, center.y);
        },
        None => {}
    }

    // `WM_SETCURSOR` / `WM_ERASEBKGND` 的返回值是约定：1 = 已处理。
    if msg == WM_SETCURSOR || msg == WM_ERASEBKGND {
        1
    } else {
        0
    }
}

/// 客户区中心，同时给出客户区与屏幕两种坐标。
unsafe fn grab_center(hwnd: HWND) -> Option<Grab> {
    let mut rect = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut rect) } == 0 {
        return None;
    }
    let client = POINT {
        x: rect.right / 2,
        y: rect.bottom / 2,
    };
    let mut screen = client;
    if unsafe { ClientToScreen(hwnd, &mut screen) } == 0 {
        return None;
    }
    Some(Grab { client, screen })
}

/// 一个已创建的 Win32 窗口。
pub(crate) struct Win32Window {
    hwnd: HWND,
    hinstance: HINSTANCE,
    /// 与 `GWLP_USERDATA` 共享的窗口状态（`WndProc` 侧用裸指针访问）。
    state: Arc<Mutex<WindowState>>,
    /// `Arc::into_raw` 的指针；`Drop` 时用 `Arc::from_raw` 回收这份引用。
    state_ptr: *mut Mutex<WindowState>,
}

// HWND 是进程级句柄；本类型只在创建它的主线程上使用（跨线程只传
// `SendWindowHandles`）。
unsafe impl Send for Win32Window {}
unsafe impl Sync for Win32Window {}

impl Drop for Win32Window {
    fn drop(&mut self) {
        unsafe {
            if !self.hwnd.is_null() {
                // 先销毁窗口（`WndProc` 还会访问状态），再回收 Arc。
                DestroyWindow(self.hwnd);
            }
            drop(Arc::from_raw(self.state_ptr));
        }
    }
}

fn window_styles(cfg: &WindowConfig) -> (u32, u32) {
    // 无边框 / 全屏：标题栏与可调整大小都不适用，直接用 popup。
    if cfg.fullscreen || !cfg.decorations {
        return (WS_POPUP | WS_VISIBLE, 0);
    }
    let mut style = WS_OVERLAPPEDWINDOW;
    if !cfg.resizable {
        style &= !(WS_THICKFRAME | WS_MAXIMIZEBOX);
    }
    (style, 0)
}

/// 用 Win32 API 创建窗口（不经过 winit）。
pub(crate) fn create_window(cfg: &WindowConfig) -> anyhow::Result<PlatformWindow> {
    unsafe {
        // Per-Monitor-V2：不声明的话进程被当成 DPI-unaware，Windows 会把整块
        // 画面位图拉伸（糊）。重复设置会失败，忽略即可。
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        let hinstance = GetModuleHandleW(std::ptr::null());
        anyhow::ensure!(!hinstance.is_null(), "GetModuleHandleW failed");

        let class_name = wide(CLASS_NAME);
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW | CS_OWNDC,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinstance,
            hIcon: std::ptr::null_mut(),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            // 不画背景：画面完全由 Vulkan 呈现，GDI 重绘只会闪。
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class_name.as_ptr(),
            hIconSm: std::ptr::null_mut(),
        };
        let atom = RegisterClassExW(&class);
        if atom == 0 {
            let error = GetLastError();
            // ERROR_CLASS_ALREADY_EXISTS = 1410：同进程第二次开窗口，正常。
            anyhow::ensure!(error == 1410, "RegisterClassExW failed ({error})");
        }

        // 配置里的宽高按 winit 的 `LogicalSize` 语义解释（逻辑像素），
        // 这里按系统 DPI 折算成客户区物理像素，两个后端开出来的窗口一样大。
        let scale = GetDpiForSystem() as f32 / 96.0;
        let client_w = ((cfg.width.max(1) as f32) * scale).round() as i32;
        let client_h = ((cfg.height.max(1) as f32) * scale).round() as i32;

        let (style, ex_style) = window_styles(cfg);
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: client_w,
            bottom: client_h,
        };
        if cfg.fullscreen {
            // 无边框全屏：直接铺满主显示器（不做独占全屏）。
            rect.right = GetSystemMetrics(SM_CXSCREEN);
            rect.bottom = GetSystemMetrics(SM_CYSCREEN);
        } else {
            AdjustWindowRectEx(&mut rect, style, 0, ex_style);
        }

        let title = wide(&cfg.title);
        let state = Arc::new(Mutex::new(WindowState::new(cfg)));
        let state_ptr = Arc::into_raw(Arc::clone(&state)) as *mut Mutex<WindowState>;

        let (x, y) = match (cfg.position_x, cfg.position_y) {
            (Some(x), Some(y)) => (x, y),
            _ => (CW_USEDEFAULT, CW_USEDEFAULT),
        };

        let hwnd = CreateWindowExW(
            ex_style,
            class_name.as_ptr(),
            title.as_ptr(),
            style,
            x,
            y,
            rect.right - rect.left,
            rect.bottom - rect.top,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinstance,
            state_ptr as *const c_void,
        );
        if hwnd.is_null() {
            // 窗口没建成：收回传出去的 Arc 引用，避免泄漏。
            drop(Arc::from_raw(state_ptr));
            anyhow::bail!("CreateWindowExW failed ({})", GetLastError());
        }

        lock(&state).dpi = GetDpiForWindow(hwnd);

        if cfg.maximized {
            ShowWindow(hwnd, SW_MAXIMIZE);
        } else if cfg.visible {
            ShowWindow(hwnd, SW_SHOW);
        } else {
            ShowWindow(hwnd, SW_HIDE);
        }
        UpdateWindow(hwnd);

        let window = Win32Window {
            hwnd,
            hinstance,
            state,
            state_ptr,
        };
        let (w, h) = window.inner_size();
        log::info!(
            "win32: window created ({w}x{h} client px, visible = {})",
            cfg.visible
        );
        Ok(PlatformWindow::from_win32(window))
    }
}

impl Win32Window {
    pub(crate) fn inner_size(&self) -> (u32, u32) {
        let mut rect = RECT::default();
        if unsafe { GetClientRect(self.hwnd, &mut rect) } == 0 {
            return (0, 0);
        }
        (
            (rect.right - rect.left).max(0) as u32,
            (rect.bottom - rect.top).max(0) as u32,
        )
    }

    pub(crate) fn scale_factor(&self) -> f64 {
        let dpi = lock(&self.state).dpi.max(96);
        dpi as f64 / 96.0
    }

    pub(crate) fn request_redraw(&self) {
        unsafe { InvalidateRect(self.hwnd, std::ptr::null(), 0) };
    }

    pub(crate) fn raw_window_handle(&self) -> RawWindowHandle {
        use std::num::NonZeroIsize;

        let mut handle = Win32WindowHandle::new(
            NonZeroIsize::new(self.hwnd as isize).expect("HWND must be non-zero"),
        );
        // 必须给 `hinstance`：缺它时 `create_surface` 返回
        // `ERROR_INITIALIZATION_FAILED`（editor 原生视口踩过的坑）。
        handle.hinstance = NonZeroIsize::new(self.hinstance as isize);
        RawWindowHandle::Win32(handle)
    }

    pub(crate) fn raw_display_handle(&self) -> RawDisplayHandle {
        RawDisplayHandle::Windows(raw_window_handle::WindowsDisplayHandle::new())
    }

    pub(crate) fn set_cursor_visible(&self, visible: bool) {
        // `ShowCursor` 是计数器语义：只在状态翻转时调一次。调用放在放开
        // 状态锁之后（它可能触发 `WM_SETCURSOR`，同线程重入会二次加锁）。
        let flip = {
            let mut st = lock(&self.state);
            let hidden = st.cursor_hidden;
            if hidden == visible {
                st.cursor_hidden = !visible;
                true
            } else {
                false
            }
        };
        if flip {
            unsafe { ShowCursor(if visible { 1 } else { 0 }) };
        }
    }

    pub(crate) fn set_pointer_confined(&self, confined: bool) -> anyhow::Result<()> {
        if !confined {
            lock(&self.state).grab = None;
            unsafe { ClipCursor(std::ptr::null()) };
            return Ok(());
        }

        let grab = unsafe { grab_center(self.hwnd) }
            .ok_or_else(|| anyhow::anyhow!("GetClientRect/ClientToScreen failed"))?;
        let mut rect = RECT::default();
        anyhow::ensure!(
            unsafe { GetClientRect(self.hwnd, &mut rect) } != 0,
            "GetClientRect failed"
        );
        let mut top_left = POINT {
            x: rect.left,
            y: rect.top,
        };
        let mut bottom_right = POINT {
            x: rect.right,
            y: rect.bottom,
        };
        unsafe {
            anyhow::ensure!(
                ClientToScreen(self.hwnd, &mut top_left) != 0
                    && ClientToScreen(self.hwnd, &mut bottom_right) != 0,
                "ClientToScreen failed"
            );
            let clip = RECT {
                left: top_left.x,
                top: top_left.y,
                right: bottom_right.x,
                bottom: bottom_right.y,
            };
            ClipCursor(&clip);
            SetCursorPos(grab.screen.x, grab.screen.y);
        }
        lock(&self.state).grab = Some(grab);
        Ok(())
    }

    pub(crate) fn close_requested(&self) -> bool {
        lock(&self.state).close_requested
    }

    /// 取出自上次调用以来排队的事件（`PeekMessageW` 泵 + `WndProc` 收集）。
    pub(crate) fn poll_events(&self) -> Vec<PlatformEvent> {
        loop {
            let mut msg = MSG::default();
            let has = unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) };
            if has == 0 {
                break;
            }
            // WM_QUIT 不走 `DispatchMessageW`。
            if msg.message == WM_QUIT {
                lock(&self.state).close_requested = true;
                break;
            }
            unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        let mut st = lock(&self.state);
        st.events.drain(..).collect()
    }

    /// 阻塞直到有消息到达或 `timeout_ms` 到期（消息留给 `poll_events` 取走）。
    pub(crate) fn wait_events(&self, timeout_ms: u32) {
        unsafe {
            MsgWaitForMultipleObjectsEx(
                0,
                std::ptr::null(),
                timeout_ms,
                QS_ALLINPUT,
                MWMO_INPUTAVAILABLE,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_low_and_high_word() {
        assert_eq!(loword(0x0001_0002), 2);
        assert_eq!(hiword(0x0001_0002), 1);
        assert_eq!(signed_x(0x0000_0002), 2);
        assert_eq!(signed_y(0x0001_0002), 1);
        // 负坐标（鼠标移到客户区左上角外）。
        assert_eq!(signed_x(0x0000_FFFE), -2);
        assert_eq!(signed_y(0xFFFF_0000u64 as isize), -1);
        // 滚轮一格 = 1.0 行。
        assert_eq!(hiword(120usize) as i16 as f32 / WHEEL_DELTA, 1.0);
        assert_eq!(
            hiword((-120i16 as u16) as usize) as i16 as f32 / WHEEL_DELTA,
            -1.0
        );
    }

    #[test]
    fn window_styles_drop_resize_frame_when_fixed() {
        let mut cfg = WindowConfig {
            resizable: false,
            ..WindowConfig::default()
        };
        let (style, _) = window_styles(&cfg);
        assert_eq!(style & WS_THICKFRAME, 0);
        assert_eq!(style & WS_MAXIMIZEBOX, 0);
        assert_eq!(
            style,
            WS_OVERLAPPEDWINDOW & !(WS_THICKFRAME | WS_MAXIMIZEBOX)
        );

        cfg.resizable = true;
        let (style, _) = window_styles(&cfg);
        assert_eq!(style, WS_OVERLAPPEDWINDOW);

        cfg.decorations = false;
        let (style, _) = window_styles(&cfg);
        assert_ne!(style & WS_POPUP, 0);
    }
}
