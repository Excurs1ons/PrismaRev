//! 主题 token → GPUI 颜色 / 尺寸的映射。
//!
//! 所有面板都用这里的 helper 取色，避免在 UI 代码里写死颜色（与 Web 编辑器
//! 的 CSS 变量约定一致）。

use gpui::{px, Hsla, Pixels};
use prism_editor_gpui_core::{Rgba, ThemeColors, ThemeToken};

pub fn hsla(color: Rgba) -> Hsla {
    gpui::rgba(
        ((color.r as u32) << 24)
            | ((color.g as u32) << 16)
            | ((color.b as u32) << 8)
            | color.a as u32,
    )
    .into()
}

#[derive(Debug, Clone, Copy)]
pub struct Style {
    pub colors: ThemeColors,
}

impl Style {
    pub fn new(colors: ThemeColors) -> Self {
        Self { colors }
    }

    pub fn color(&self, token: ThemeToken) -> Hsla {
        hsla(self.colors.color(token))
    }

    /// 半透明版本（hover / 高亮底色）。
    pub fn alpha(&self, token: ThemeToken, alpha: f32) -> Hsla {
        self.color(token).opacity(alpha)
    }

    pub fn bg(&self) -> Hsla {
        self.color(ThemeToken::Bg)
    }

    pub fn panel(&self) -> Hsla {
        self.color(ThemeToken::Panel)
    }

    pub fn panel_alt(&self) -> Hsla {
        self.color(ThemeToken::PanelAlt)
    }

    pub fn panel_raised(&self) -> Hsla {
        self.color(ThemeToken::PanelRaised)
    }

    /// 弹出层底色：优先 `panelRaised`，浅色主题里也同样可用。
    pub fn panel_raised_or_panel(&self) -> Hsla {
        self.color(ThemeToken::PanelRaised)
    }

    pub fn row_hover(&self) -> Hsla {
        self.color(ThemeToken::RowHover)
    }

    /// 凹陷底色（输入框 / 日志 / JSON 预览）。
    pub fn bg_sunken(&self) -> Hsla {
        self.color(ThemeToken::BgSunken)
    }

    pub fn chrome(&self) -> Hsla {
        self.color(ThemeToken::Chrome)
    }

    pub fn border(&self) -> Hsla {
        self.color(ThemeToken::Border)
    }

    pub fn border_strong(&self) -> Hsla {
        self.color(ThemeToken::BorderStrong)
    }

    pub fn text(&self) -> Hsla {
        self.color(ThemeToken::Text)
    }

    pub fn text_strong(&self) -> Hsla {
        self.color(ThemeToken::TextStrong)
    }

    pub fn text_soft(&self) -> Hsla {
        self.color(ThemeToken::TextSoft)
    }

    pub fn muted(&self) -> Hsla {
        self.color(ThemeToken::Muted)
    }

    pub fn accent(&self) -> Hsla {
        self.color(ThemeToken::Accent)
    }

    pub fn selection(&self) -> Hsla {
        self.color(ThemeToken::Selection)
    }

    pub fn danger(&self) -> Hsla {
        self.color(ThemeToken::Danger)
    }

    pub fn warning(&self) -> Hsla {
        self.color(ThemeToken::Warning)
    }

    pub fn success(&self) -> Hsla {
        self.color(ThemeToken::Success)
    }
}

pub const ROW_HEIGHT: Pixels = px(22.0);
pub const TAB_HEIGHT: Pixels = px(26.0);
pub const TOOLBAR_HEIGHT: Pixels = px(38.0);
pub const MENU_HEIGHT: Pixels = px(26.0);
pub const STATUS_HEIGHT: Pixels = px(24.0);
pub const SPLITTER: Pixels = px(4.0);
pub const INDENT: Pixels = px(12.0);
pub const FONT_BASE: Pixels = px(12.0);
