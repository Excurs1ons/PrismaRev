//! UI ECS 组件 —— 数据驱动 UI 的组件定义
//!
//! # 理念
//! UI 元素 = Entity + [`Node`] + [`Style`] + [`ComputedLayout`] + (可选 [`Text`])
//! Layout System 读取 Style → 写入 ComputedLayout
//! Render System 读取 ComputedLayout + Text → 生成绘制命令

// ── 标记组件 ─────────────────────────────────────────────────

/// **标记**：此 Entity 是 UI 元素。Layout System 只查询有此组件的实体。
#[derive(Clone, Copy, Debug)]
pub struct Node;

/// **父子关系**：UI 子元素指向父元素 Entity。
#[derive(Clone, Copy, Debug)]
pub struct UiParent(pub prism_ecs::Entity);

// ── 锚点系统 ─────────────────────────────────────────────────

/// 锚点角 —— 定义子元素边相对于父边的位置（0..=1 归一化）。
#[derive(Clone, Copy, Debug)]
pub struct Anchors {
    /// 水平起始角（0 = 父左，1 = 父右）
    pub min_x: f32,
    /// 垂直起始角（0 = 父顶，1 = 父底）
    pub min_y: f32,
    /// 水平结束角（0 = 父左，1 = 父右）
    pub max_x: f32,
    /// 垂直结束角（0 = 父顶，1 = 父底）
    pub max_y: f32,
}

impl Anchors {
    /// 填满父容器
    pub const STRETCH: Self = Self {
        min_x: 0.0,
        min_y: 0.0,
        max_x: 1.0,
        max_y: 1.0,
    };
    /// 左上角
    pub const TOP_LEFT: Self = Self {
        min_x: 0.0,
        min_y: 0.0,
        max_x: 0.0,
        max_y: 0.0,
    };
    /// 居中
    pub const CENTER: Self = Self {
        min_x: 0.5,
        min_y: 0.5,
        max_x: 0.5,
        max_y: 0.5,
    };
    /// 底部居中
    pub const BOTTOM_CENTER: Self = Self {
        min_x: 0.5,
        min_y: 1.0,
        max_x: 0.5,
        max_y: 1.0,
    };
}

impl Default for Anchors {
    fn default() -> Self {
        Self::STRETCH
    }
}

/// Pivot —— 旋转/缩放/定位的中心点（归一化 0..=1）。
#[derive(Clone, Copy, Debug)]
pub struct Pivot {
    pub x: f32,
    pub y: f32,
}

impl Pivot {
    pub const CENTER: Self = Self { x: 0.5, y: 0.5 };
    pub const TOP_LEFT: Self = Self { x: 0.0, y: 0.0 };
}

impl Default for Pivot {
    fn default() -> Self {
        Self::CENTER
    }
}

/// 边距（像素）。
#[derive(Clone, Copy, Debug, Default)]
pub struct Margin {
    pub left: f32,
    pub right: f32,
    pub top: f32,
    pub bottom: f32,
}

// ── Style ─────────────────────────────────────────────────────

/// UI 元素的视觉和布局属性。
#[derive(Clone, Debug)]
pub struct Style {
    /// 固定宽度（px）。`None` = 由锚点决定。
    pub width: Option<f32>,
    /// 固定高度（px）。`None` = 由锚点决定。
    pub height: Option<f32>,
    /// 锚点。
    pub anchors: Anchors,
    /// 枢轴（归一化，0..=1）。
    pub pivot: Pivot,
    /// 边距偏移（px）。
    pub margin: Margin,
    /// 背景色（RGBA，0..=1）。`[0;4]` = 透明。
    pub background: [f32; 4],
    /// 边框圆角（px）。
    pub border_radius: f32,
    /// 可见性。
    pub visible: bool,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            width: None,
            height: None,
            anchors: Anchors::STRETCH,
            pivot: Pivot::CENTER,
            margin: Margin::default(),
            background: [0.0; 4],
            border_radius: 0.0,
            visible: true,
        }
    }
}

impl Style {
    /// 创建全屏填充的透明 UI 元素。
    pub fn fullscreen() -> Self {
        Self::default()
    }

    /// 创建一个固定大小的 UI 元素（锚点居中）。
    pub fn fixed(w: f32, h: f32) -> Self {
        Self {
            width: Some(w),
            height: Some(h),
            anchors: Anchors::CENTER,
            pivot: Pivot::CENTER,
            ..Default::default()
        }
    }
}

// ── ComputedLayout ────────────────────────────────────────────

/// **Layout System 输出**：最终屏幕空间矩形（像素）。
#[derive(Clone, Copy, Debug)]
pub struct ComputedLayout {
    /// 屏幕空间边界 `[left, top, width, height]`
    pub rect: [f32; 4],
}

impl ComputedLayout {
    pub fn left(&self) -> f32 {
        self.rect[0]
    }
    pub fn top(&self) -> f32 {
        self.rect[1]
    }
    pub fn width(&self) -> f32 {
        self.rect[2]
    }
    pub fn height(&self) -> f32 {
        self.rect[3]
    }
}

// ── Text ──────────────────────────────────────────────────────

/// 文本内容。
#[derive(Clone, Debug)]
pub struct Text {
    pub content: String,
    pub font_size: f32,
    pub color: [f32; 4],
    pub alignment: TextAlign,
}

impl Default for Text {
    fn default() -> Self {
        Self {
            content: String::new(),
            font_size: 16.0,
            color: [1.0; 4],
            alignment: TextAlign::Center,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub enum TextAlign {
    #[default]
    Center,
    Left,
    Right,
}

// ── Interaction ───────────────────────────────────────────────

/// 交互状态（由 Input System 每帧更新）。
#[derive(Clone, Copy, Debug, Default)]
pub struct Interaction {
    pub hovered: bool,
    pub pressed: bool,
    /// 一帧有效，input system 设置后下一帧清零。
    pub clicked: bool,
}
