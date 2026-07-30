//! UI 系统 —— 基于 ECS 的声明式 UI
//!
//! # 架构
//!
//! ```text
//! UI 实体 = Node + Style + ComputedLayout + (Text | Interaction)
//!
//! [Layout System]    读取 Style → 写入 ComputedLayout
//! [Render System]    读取 ComputedLayout + Text → 生成 UiDrawList
//! ```
//!
//! 每个 UI 元素是一个 ECS Entity，通过组件组合定义外观和行为。
//! Panel 基类封装了常用的 UI Entity 创建逻辑。
//!
//! # 集成到 Engine
//!
//! ```ignore
//! engine.schedule_mut().add_systems(
//!     crate::app::ScheduleLabel::Update,
//!     crate::ui::ui_layout_system,
//! );
//! engine.schedule_mut().add_systems(
//!     crate::app::ScheduleLabel::PostRender,
//!     crate::ui::ui_render_system,
//! );
//! ```

mod components;
mod input;
mod layout;
mod panel_base;
mod render;

pub use components::*;
pub use input::{ui_input_system, UiInputState};
pub use layout::{ui_layout_system, ScreenSize};
pub use panel_base::PanelBase;
pub use render::{
    convert_ui_draw_list_to_overlay, ui_render_system, UiDrawList, UiQuad, UiTextCmd,
};

/// UI 面板接口（将逐步被 ECS 组件方案取代）。
pub trait Panel {
    /// 每帧更新。返回 `false` 表示面板请求关闭。
    fn on_update(&mut self, _dt: f32) -> bool {
        true
    }
    /// 处理输入事件。
    fn on_event(&mut self, _event: &()) {}
    /// 面板关闭时的清理回调。
    fn on_close(&mut self) {}
}
