//! Unity `PropertyDrawer` / `PropertyAttribute` 形状的 schema 投影。
//!
//! Unity 里 `[Range]` / `[TextArea]` / `[ColorUsage]` 这类**特性**（attribute）由对应的
//! `PropertyDrawer` 渲染成滑动条 / 多行文本框 / 取色盘；`[CustomPropertyDrawer]` 则把
//! 某个字段或某个类型整体交给自定义绘制器。
//!
//! 这一层只输出**声明式 DTO**（渲染器 + 特性列表），真正的 UI 在 `editor/` 的 Vue
//! （[`FieldEditor.vue`](../../../editor/src/components/FieldEditor.vue)）里。这样既拿到
//! Unity 的扩展体验，又保持 `prism-editor-api` 不链接 UI / engine / Vulkan。
//!
//! | Unity | 这里 |
//! |---|---|
//! | `[Range(min, max)]` | [`PropertyDrawer::range`] → [`PropertyRenderer::Slider`] |
//! | `[Min]` / `[Max]` | [`PropertyDrawer::min`] / [`PropertyDrawer::max`] |
//! | `[Multiline(lines)]` | [`PropertyDrawer::multiline`] → [`PropertyRenderer::TextArea`] |
//! | `[TextArea(min, max)]` | [`PropertyDrawer::text_area`] |
//! | `[ColorUsage(showAlpha, hdr)]` | [`PropertyDrawer::color_usage`] → [`PropertyRenderer::Color`] |
//! | `[Header]` / `[Tooltip]` / `[Delayed]` | [`PropertyDrawer::with_header`] / [`PropertyDrawer::with_tooltip`] / [`PropertyDrawer::delayed`] |
//! | `[CustomPropertyDrawer(typeof(X))]` | [`PropertyDrawerRegistry::register_type`] |
//! | `[CustomPropertyDrawer]` 到某字段 | [`PropertyDrawerRegistry::register_custom`] |
//!
//! 内置组件的绘制器声明集中在 [`PropertyDrawerRegistry::with_builtin`]，由
//! [`crate::schema::build_schema`] 应用；宿主也可以构造自己的注册表并用
//! [`crate::schema::build_schema_with`] 追加声明。

use serde::{Deserialize, Serialize};

/// 字段渲染器 = Unity 里选中哪个 `PropertyDrawer`。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PropertyRenderer {
    /// 按 `FieldSchema.kind` 的默认控件（文本框 / 复选框 / 下拉框 / 数值框）。
    #[default]
    Default,
    /// `[Range]` 的滑动条。
    Slider,
    /// `[TextArea]` / `[Multiline]` 的多行文本框。
    TextArea,
    /// `[ColorUsage]` 的取色盘（即使字段名不像颜色也强制用）。
    Color,
    /// `[CustomPropertyDrawer]`：按 [`PropertyDrawer::custom`] 的名字找前端组件。
    Custom,
}

/// 一条 Unity 特性（`PropertyAttribute`）。
///
/// 序列化成 `{ "type": "range", "min": 0.0, "max": 5.0 }` 这类带标签的对象，
/// 前端按 `type` 分发，未知类型直接忽略（保持向前兼容）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PropertyAttribute {
    /// `[Range(min, max)]`：滑动条 + 数值框。
    Range { min: f32, max: f32 },
    /// `[Min(min)]`：下限（不显示滑动条）。
    Min { min: f32 },
    /// `[Max(max)]`：上限（不显示滑动条）。
    Max { max: f32 },
    /// `[Multiline(lines)]`：固定行数的多行文本框。
    Multiline { lines: usize },
    /// `[TextArea(minLines, maxLines)]`：可伸缩的多行文本框。
    TextArea { min_lines: usize, max_lines: usize },
    /// `[ColorUsage(showAlpha, hdr)]`：取色盘是否显示 alpha / 支持 HDR。
    ColorUsage { show_alpha: bool, hdr: bool },
    /// `[Header(text)]`：字段上方的分组标题。
    Header { text: String },
    /// `[Tooltip(text)]`：悬停提示。
    Tooltip { text: String },
    /// `[Delayed]`：只在回车 / 失焦时提交，而不是每次输入。
    Delayed,
}

/// 一个字段的绘制声明（渲染器 + 特性列表）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PropertyDrawer {
    /// 选中哪个绘制器。
    pub renderer: PropertyRenderer,
    /// 特性列表（顺序即声明顺序）。
    pub attributes: Vec<PropertyAttribute>,
    /// `renderer == Custom` 时的前端组件名（如 `TransformEditor`）。
    pub custom: Option<String>,
}

impl PropertyDrawer {
    /// 用指定渲染器开一条空声明。
    pub fn new(renderer: PropertyRenderer) -> Self {
        Self {
            renderer,
            attributes: Vec::new(),
            custom: None,
        }
    }

    /// `[Range(min, max)]` → 滑动条。`max <= min` 时视为无效并退回默认控件。
    pub fn range(min: f32, max: f32) -> Self {
        if !(min.is_finite() && max.is_finite() && max > min) {
            return Self::default();
        }
        Self::new(PropertyRenderer::Slider).attribute(PropertyAttribute::Range { min, max })
    }

    /// `[Min(min)]`：只限制下限。
    pub fn min(min: f32) -> Self {
        Self::new(PropertyRenderer::Default).attribute(PropertyAttribute::Min { min })
    }

    /// `[Max(max)]`：只限制上限。
    pub fn max(max: f32) -> Self {
        Self::new(PropertyRenderer::Default).attribute(PropertyAttribute::Max { max })
    }

    /// `[Multiline(lines)]` → 多行文本框。
    pub fn multiline(lines: usize) -> Self {
        Self::new(PropertyRenderer::TextArea).attribute(PropertyAttribute::Multiline {
            lines: lines.max(1),
        })
    }

    /// `[TextArea(minLines, maxLines)]` → 可伸缩多行文本框。
    pub fn text_area(min_lines: usize, max_lines: usize) -> Self {
        let min_lines = min_lines.max(1);
        let max_lines = max_lines.max(min_lines);
        Self::new(PropertyRenderer::TextArea).attribute(PropertyAttribute::TextArea {
            min_lines,
            max_lines,
        })
    }

    /// `[ColorUsage(showAlpha, hdr)]` → 取色盘。
    pub fn color_usage(show_alpha: bool, hdr: bool) -> Self {
        Self::new(PropertyRenderer::Color)
            .attribute(PropertyAttribute::ColorUsage { show_alpha, hdr })
    }

    /// `[CustomPropertyDrawer]`：交给名为 `renderer` 的前端组件。
    pub fn custom(renderer: impl Into<String>) -> Self {
        Self {
            renderer: PropertyRenderer::Custom,
            attributes: Vec::new(),
            custom: Some(renderer.into()),
        }
    }

    /// 追加一条特性。
    pub fn attribute(mut self, attribute: PropertyAttribute) -> Self {
        // 重复的同类特性只保留最后一条（Unity 也只有一个 Range/Tooltip）。
        self.attributes.retain(|existing| {
            std::mem::discriminant(existing) != std::mem::discriminant(&attribute)
        });
        self.attributes.push(attribute);
        self
    }

    /// `[Header(text)]`。
    pub fn with_header(self, text: impl Into<String>) -> Self {
        self.attribute(PropertyAttribute::Header { text: text.into() })
    }

    /// `[Tooltip(text)]`。
    pub fn with_tooltip(self, text: impl Into<String>) -> Self {
        self.attribute(PropertyAttribute::Tooltip { text: text.into() })
    }

    /// `[Delayed]`。
    pub fn delayed(self) -> Self {
        self.attribute(PropertyAttribute::Delayed)
    }

    /// `[Range]` 的上下界（没有则 `None`）。
    pub fn range_bounds(&self) -> Option<(f32, f32)> {
        self.attributes
            .iter()
            .find_map(|attribute| match attribute {
                PropertyAttribute::Range { min, max } => Some((*min, *max)),
                _ => None,
            })
    }

    /// 下限（`[Range]` 或 `[Min]`）。
    pub fn lower_bound(&self) -> Option<f32> {
        self.attributes
            .iter()
            .find_map(|attribute| match attribute {
                PropertyAttribute::Range { min, .. } | PropertyAttribute::Min { min } => Some(*min),
                _ => None,
            })
    }

    /// 上限（`[Range]` 或 `[Max]`）。
    pub fn upper_bound(&self) -> Option<f32> {
        self.attributes
            .iter()
            .find_map(|attribute| match attribute {
                PropertyAttribute::Range { max, .. } | PropertyAttribute::Max { max } => Some(*max),
                _ => None,
            })
    }

    /// 多行文本框的行数范围。
    pub fn line_range(&self) -> Option<(usize, usize)> {
        self.attributes
            .iter()
            .find_map(|attribute| match attribute {
                PropertyAttribute::Multiline { lines } => Some((*lines, *lines)),
                PropertyAttribute::TextArea {
                    min_lines,
                    max_lines,
                } => Some((*min_lines, *max_lines)),
                _ => None,
            })
    }

    /// `[ColorUsage]` 的 `(show_alpha, hdr)`。
    pub fn color_usage_flags(&self) -> Option<(bool, bool)> {
        self.attributes
            .iter()
            .find_map(|attribute| match attribute {
                PropertyAttribute::ColorUsage { show_alpha, hdr } => Some((*show_alpha, *hdr)),
                _ => None,
            })
    }

    /// `[Tooltip]` 文字。
    pub fn tooltip(&self) -> Option<&str> {
        self.attributes
            .iter()
            .find_map(|attribute| match attribute {
                PropertyAttribute::Tooltip { text } => Some(text.as_str()),
                _ => None,
            })
    }

    /// `[Header]` 文字。
    pub fn header(&self) -> Option<&str> {
        self.attributes
            .iter()
            .find_map(|attribute| match attribute {
                PropertyAttribute::Header { text } => Some(text.as_str()),
                _ => None,
            })
    }

    /// 是否带 `[Delayed]`。
    pub fn is_delayed(&self) -> bool {
        self.attributes
            .iter()
            .any(|attribute| matches!(attribute, PropertyAttribute::Delayed))
    }
}

/// 一条绘制声明的作用域。
#[derive(Debug, Clone, PartialEq)]
struct DrawerEntry {
    /// 声明者类型（`#[reflect(name = ...)]` 的规范名）。
    type_name: String,
    /// `Some(field)` = 只作用于该字段；`None` = 作用于整个类型
    /// （`[CustomPropertyDrawer(typeof(X))]`）。
    field: Option<String>,
    drawer: PropertyDrawer,
}

/// 字段绘制器注册表 = Unity `PropertyDrawer` 的查找表。
///
/// 查找顺序：**字段级声明优先于类型级**；同类声明后注册的覆盖先注册的
/// （与 Unity 允许派生/后置特性覆盖一致）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PropertyDrawerRegistry {
    entries: Vec<DrawerEntry>,
}

impl PropertyDrawerRegistry {
    /// 空注册表（没有声明时 schema 的 `drawer` 全是 `None`）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 内置组件字段的绘制器声明（[`crate::schema::build_schema`] 使用）。
    ///
    /// 只声明**有明确数值/展示语义**的字段，不替所有字段猜控件：
    /// `Camera` 的曝光/视场/背景色、`Light` 的强度/范围/聚光角/阴影强度、
    /// `Skybox` 的旋转/强度、`Text` 的正文。
    pub fn with_builtin() -> Self {
        let mut registry = Self::new();

        let camera = "prism_engine::scene::Camera";
        registry.register(
            camera,
            "fov_y_degrees",
            PropertyDrawer::range(1.0, 179.0).with_tooltip("垂直视场角（度）= Camera.fieldOfView"),
        );
        registry.register(
            camera,
            "exposure",
            PropertyDrawer::range(0.0, 5.0).with_tooltip("曝光倍率，1.0 = 不缩放"),
        );
        registry.register(
            camera,
            "background_color",
            PropertyDrawer::color_usage(true, false)
                .with_tooltip("纯色背景（clearFlags = SolidColor 时生效）"),
        );
        registry.register(camera, "orthographic_size", PropertyDrawer::min(0.0001));
        registry.register(camera, "near", PropertyDrawer::min(0.0001));
        registry.register(camera, "far", PropertyDrawer::min(0.0001));

        let light = "prism_engine::scene::Light";
        registry.register(light, "intensity", PropertyDrawer::min(0.0));
        registry.register(light, "range", PropertyDrawer::min(0.0));
        registry.register(light, "spot_angle", PropertyDrawer::range(1.0, 179.0));
        registry.register(light, "inner_spot_angle", PropertyDrawer::range(1.0, 179.0));
        registry.register(
            light,
            "shadow_strength",
            PropertyDrawer::range(0.0, 1.0).with_tooltip("阴影强度，0 = 无阴影、1 = 全黑"),
        );
        registry.register(light, "color_temperature", PropertyDrawer::min(0.0));

        let skybox = "prism_engine::scene::Skybox";
        registry.register(
            skybox,
            "rotation_degrees",
            PropertyDrawer::range(0.0, 360.0),
        );
        registry.register(skybox, "intensity", PropertyDrawer::min(0.0));

        registry.register(
            "prism_engine::ui::Text",
            "content",
            PropertyDrawer::text_area(2, 6),
        );

        registry
    }

    /// 给某个类型的某个字段登记绘制器 = `[CustomPropertyDrawer]` 到字段。
    pub fn register(
        &mut self,
        type_name: impl Into<String>,
        field: impl Into<String>,
        drawer: PropertyDrawer,
    ) {
        let type_name = type_name.into();
        let field = field.into();
        self.entries.retain(|entry| {
            !(entry.type_name == type_name && entry.field.as_deref() == Some(field.as_str()))
        });
        self.entries.push(DrawerEntry {
            type_name,
            field: Some(field),
            drawer,
        });
    }

    /// 给某个类型整体登记绘制器 = `[CustomPropertyDrawer(typeof(X))]`。
    ///
    /// 字段级声明仍会覆盖它（Unity 里字段上的特性优先于类型级绘制器）。
    pub fn register_type(&mut self, type_name: impl Into<String>, drawer: PropertyDrawer) {
        let type_name = type_name.into();
        self.entries
            .retain(|entry| !(entry.type_name == type_name && entry.field.is_none()));
        self.entries.push(DrawerEntry {
            type_name,
            field: None,
            drawer,
        });
    }

    /// 给某个字段登记自定义前端组件（`[CustomPropertyDrawer]`）。
    pub fn register_custom(
        &mut self,
        type_name: impl Into<String>,
        field: impl Into<String>,
        renderer: impl Into<String>,
    ) {
        self.register(type_name, field, PropertyDrawer::custom(renderer));
    }

    /// 查某个字段该用哪个绘制器。
    ///
    /// 查找顺序：
    /// 1. 字段级声明（`declaring_type` + `field`）；
    /// 2. 字段值类型级声明（`value_type`，对齐 Unity `[CustomPropertyDrawer(typeof(X))]`）。
    pub fn drawer_for_field(
        &self,
        declaring_type: &str,
        field: &str,
        value_type: Option<&str>,
    ) -> Option<&PropertyDrawer> {
        self.entries
            .iter()
            .rev()
            .find(|entry| {
                entry.type_name == declaring_type && entry.field.as_deref() == Some(field)
            })
            .or_else(|| {
                value_type.and_then(|val_ty| {
                    self.entries
                        .iter()
                        .rev()
                        .find(|entry| entry.type_name == val_ty && entry.field.is_none())
                })
            })
            .map(|entry| &entry.drawer)
    }

    /// 查某个声明者类型的具体字段（不传值类型）。
    pub fn drawer_for(&self, type_name: &str, field: &str) -> Option<&PropertyDrawer> {
        self.drawer_for_field(type_name, field, None)
    }

    /// 查针对某个类型本身的绘制器（`[CustomPropertyDrawer(typeof(X))]`）。
    pub fn drawer_for_type(&self, type_name: &str) -> Option<&PropertyDrawer> {
        self.entries
            .iter()
            .rev()
            .find(|entry| entry.type_name == type_name && entry.field.is_none())
            .map(|entry| &entry.drawer)
    }

    /// 已登记的声明条数（测试 / 诊断用）。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否没有任何声明。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_sets_slider_and_bounds() {
        let drawer = PropertyDrawer::range(0.0, 5.0).with_tooltip("exposure");
        assert_eq!(drawer.renderer, PropertyRenderer::Slider);
        assert_eq!(drawer.range_bounds(), Some((0.0, 5.0)));
        assert_eq!(drawer.lower_bound(), Some(0.0));
        assert_eq!(drawer.upper_bound(), Some(5.0));
        assert_eq!(drawer.tooltip(), Some("exposure"));
    }

    #[test]
    fn invalid_range_falls_back_to_default() {
        let drawer = PropertyDrawer::range(2.0, 2.0);
        assert_eq!(drawer.renderer, PropertyRenderer::Default);
        assert!(drawer.attributes.is_empty());
    }

    #[test]
    fn text_area_and_multiline_report_line_ranges() {
        assert_eq!(PropertyDrawer::multiline(3).line_range(), Some((3, 3)));
        assert_eq!(PropertyDrawer::text_area(2, 6).line_range(), Some((2, 6)));
        // min > max 会被规整成 max = min。
        assert_eq!(PropertyDrawer::text_area(4, 1).line_range(), Some((4, 4)));
        assert_eq!(PropertyDrawer::text_area(0, 0).line_range(), Some((1, 1)));
    }

    #[test]
    fn duplicate_attributes_keep_the_last_one() {
        let drawer = PropertyDrawer::range(0.0, 1.0)
            .with_tooltip("first")
            .with_tooltip("second");
        assert_eq!(drawer.tooltip(), Some("second"));
        // Range 只有一条。
        assert_eq!(
            drawer
                .attributes
                .iter()
                .filter(|a| matches!(a, PropertyAttribute::Range { .. }))
                .count(),
            1
        );
    }

    #[test]
    fn delayed_flag_is_detected() {
        assert!(!PropertyDrawer::range(0.0, 1.0).is_delayed());
        assert!(PropertyDrawer::range(0.0, 1.0).delayed().is_delayed());
    }

    #[test]
    fn lookup_prefers_field_scope_over_type_scope() {
        let mut registry = PropertyDrawerRegistry::new();
        // 给类型 demo::X 登记类型级绘制器（Unity `[CustomPropertyDrawer(typeof(X))]`）
        registry.register_type("demo::X", PropertyDrawer::custom("WholeType"));

        // 宿主包含 demo::X 类型的值字段：命中类型级绘制器
        assert_eq!(
            registry
                .drawer_for_field("demo::Host", "x_field", Some("demo::X"))
                .map(|d| d.renderer),
            Some(PropertyRenderer::Custom)
        );

        // 给特定字段登记字段级绘制器：覆盖类型级
        registry.register("demo::Host", "x_field", PropertyDrawer::range(0.0, 1.0));
        assert_eq!(
            registry
                .drawer_for_field("demo::Host", "x_field", Some("demo::X"))
                .map(|d| d.renderer),
            Some(PropertyRenderer::Slider)
        );

        // 同一宿主的其它同类型字段仍走类型级声明
        assert_eq!(
            registry
                .drawer_for_field("demo::Host", "other_x", Some("demo::X"))
                .and_then(|d| d.custom.as_deref()),
            Some("WholeType")
        );

        // 其它值类型的字段不受影响
        assert!(registry
            .drawer_for_field("demo::Host", "y_field", Some("demo::Y"))
            .is_none());
    }

    #[test]
    fn later_registration_overrides_earlier() {
        let mut registry = PropertyDrawerRegistry::new();
        registry.register("demo::X", "f", PropertyDrawer::range(0.0, 1.0));
        registry.register("demo::X", "f", PropertyDrawer::text_area(2, 4));
        assert_eq!(
            registry.drawer_for("demo::X", "f").map(|d| d.renderer),
            Some(PropertyRenderer::TextArea)
        );
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn builtin_declarations_cover_key_fields() {
        let registry = PropertyDrawerRegistry::with_builtin();
        assert!(!registry.is_empty());

        let camera = "prism_engine::scene::Camera";
        let exposure = registry.drawer_for(camera, "exposure").expect("exposure");
        assert_eq!(exposure.renderer, PropertyRenderer::Slider);
        assert_eq!(exposure.range_bounds(), Some((0.0, 5.0)));

        let background = registry
            .drawer_for(camera, "background_color")
            .expect("background_color");
        assert_eq!(background.renderer, PropertyRenderer::Color);
        assert_eq!(background.color_usage_flags(), Some((true, false)));

        let text = registry
            .drawer_for("prism_engine::ui::Text", "content")
            .expect("Text.content");
        assert_eq!(text.renderer, PropertyRenderer::TextArea);
        assert_eq!(text.line_range(), Some((2, 6)));

        assert!(registry
            .drawer_for("prism_engine::scene::Light", "shadow_strength")
            .is_some());
        // 未声明的字段没有绘制器。
        assert!(registry.drawer_for(camera, "aspect").is_none());
    }

    #[test]
    fn serializes_to_tagged_camel_case_objects() {
        let drawer = PropertyDrawer::range(1.0, 179.0).with_tooltip("fov");
        let json = serde_json::to_value(&drawer).expect("serialize");
        assert_eq!(json["renderer"], "slider");
        assert_eq!(json["attributes"][0]["type"], "range");
        assert_eq!(json["attributes"][0]["min"], 1.0);
        assert_eq!(json["attributes"][0]["max"], 179.0);
        assert_eq!(json["attributes"][1]["type"], "tooltip");

        let text_area = serde_json::to_value(PropertyDrawer::text_area(2, 6)).unwrap();
        assert_eq!(text_area["attributes"][0]["type"], "textArea");
        assert_eq!(text_area["attributes"][0]["minLines"], 2);
        assert_eq!(text_area["attributes"][0]["maxLines"], 6);
    }
}
