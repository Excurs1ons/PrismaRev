//! `UnityEngine.Rendering` 枚举的 Rust 镜像。
//!
//! 这里只放**纯数据**枚举：取值与 Unity 对齐（便于对照、序列化与迁移），
//! 把它们的语义翻译成 Vulkan 状态（清屏、剔除、混合……）是 `prism-render` 的职责。
//!
//! 组件里使用这些枚举时请同时实现 [`ReflectType`]，让反射把它们编码成
//! [`FieldType::U8`]（参考 `ui::TextAlign`），并在
//! `ComponentRegistry::register_builtin_components` 里 `register_reflect_type`，
//! 否则 RTTIBin 编码时会因为缺少类型表条目而失败。

use prism_reflect::{FieldType, ReflectError, ReflectType, Value};
use serde::{Deserialize, Serialize};

/// 相机清屏方式 = `UnityEngine.CameraClearFlags`。
///
/// 数值与 Unity 一致：`(int)Skybox == 1` … `(int)Nothing == 4`（Unity 的 `0` 是
/// 未设置的非法值，这里用 [`Default`] 代替）。序列化形式是变体名
/// （`"Skybox"` / `"SolidColor"` / `"Depth"` / `"Nothing"`），与 `.scene.json` 里
/// 其它枚举（`TextAlign`）一致。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CameraClearFlags {
    /// 用天空盒填充背景。
    #[default]
    Skybox,
    /// 用纯色填充背景（见 `Camera::background_color`）。
    SolidColor,
    /// 只清深度，保留已有颜色（叠加相机用）。
    Depth,
    /// 颜色与深度都不清。
    Nothing,
}

impl CameraClearFlags {
    /// Unity 的枚举数值。
    pub const fn unity_value(self) -> u32 {
        match self {
            Self::Skybox => 1,
            Self::SolidColor => 2,
            Self::Depth => 3,
            Self::Nothing => 4,
        }
    }

    /// 从 Unity 枚举数值还原；未知值返回 `None`。
    pub const fn from_unity_value(value: u32) -> Option<Self> {
        match value {
            1 => Some(Self::Skybox),
            2 => Some(Self::SolidColor),
            3 => Some(Self::Depth),
            4 => Some(Self::Nothing),
            _ => None,
        }
    }

    /// 是否清颜色缓冲（`Skybox` / `SolidColor`）。
    pub const fn clears_color(self) -> bool {
        matches!(self, Self::Skybox | Self::SolidColor)
    }

    /// 是否清深度缓冲（除 `Nothing` 之外都清）。
    pub const fn clears_depth(self) -> bool {
        !matches!(self, Self::Nothing)
    }
}

impl ReflectType for CameraClearFlags {
    fn field_type() -> FieldType {
        FieldType::U8
    }

    fn enum_variants() -> Option<&'static [&'static str]> {
        // 顺序与声明一致；这些字符串就是 `.scene.json` 里的取值。
        Some(&["Skybox", "SolidColor", "Depth", "Nothing"])
    }

    fn to_value(&self) -> Value {
        Value::U64(u64::from(self.unity_value()))
    }

    fn from_value(value: Value) -> Result<Self, ReflectError> {
        let raw = value.as_u64()?;
        let raw = u32::try_from(raw)
            .map_err(|_| ReflectError::BadData(format!("CameraClearFlags 数值越界: {raw}")))?;
        Self::from_unity_value(raw).ok_or_else(|| {
            ReflectError::BadData(format!("invalid CameraClearFlags discriminant {raw}"))
        })
    }
}

/// 光源类型 = `UnityEngine.LightType`（取值与 Unity 一致）。
///
/// Unity 的 Inspector 只让选运行时支持的类型；这里保留完整枚举以便**无损**对照与
/// 迁移，但渲染器目前只实现 [`Spot`](Self::Spot) / [`Directional`](Self::Directional) /
/// [`Point`](Self::Point)（见 [`LightType::is_rendered`]）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LightType {
    /// 聚光（锥形）。
    Spot,
    /// 平行光（无限远，方向由实体变换决定）。
    Directional,
    /// 点光源（全向）。
    #[default]
    Point,
    /// 矩形面光源 = Unity `LightType.Rectangle`（`Area` 的新名字）。
    Rectangle,
    /// 圆形面光源。
    Disc,
    /// 金字塔形聚光。
    Pyramid,
    /// 盒形聚光。
    Box,
    /// 管状光源。
    Tube,
}

impl LightType {
    /// Unity 的枚举数值。
    pub const fn unity_value(self) -> u32 {
        match self {
            Self::Spot => 0,
            Self::Directional => 1,
            Self::Point => 2,
            Self::Rectangle => 3,
            Self::Disc => 4,
            Self::Pyramid => 5,
            Self::Box => 6,
            Self::Tube => 7,
        }
    }

    /// 从 Unity 枚举数值还原；未知值返回 `None`。
    pub const fn from_unity_value(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Spot),
            1 => Some(Self::Directional),
            2 => Some(Self::Point),
            3 => Some(Self::Rectangle),
            4 => Some(Self::Disc),
            5 => Some(Self::Pyramid),
            6 => Some(Self::Box),
            7 => Some(Self::Tube),
            _ => None,
        }
    }

    /// 当前渲染器是否真的会绘制该类型。
    ///
    /// 面光源 / 盒形 / 管状只是 schema 对齐，`collect_lights` 会跳过它们
    /// （Unity 里这类光源也只用于烘焙或需要额外管线支持）。
    pub const fn is_rendered(self) -> bool {
        matches!(self, Self::Spot | Self::Directional | Self::Point)
    }
}

impl ReflectType for LightType {
    fn field_type() -> FieldType {
        FieldType::U8
    }

    fn enum_variants() -> Option<&'static [&'static str]> {
        Some(&[
            "Spot",
            "Directional",
            "Point",
            "Rectangle",
            "Disc",
            "Pyramid",
            "Box",
            "Tube",
        ])
    }

    fn to_value(&self) -> Value {
        Value::U64(u64::from(self.unity_value()))
    }

    fn from_value(value: Value) -> Result<Self, ReflectError> {
        let raw = value.as_u64()?;
        let raw = u32::try_from(raw)
            .map_err(|_| ReflectError::BadData(format!("LightType 数值越界: {raw}")))?;
        Self::from_unity_value(raw)
            .ok_or_else(|| ReflectError::BadData(format!("invalid LightType discriminant {raw}")))
    }
}

/// 阴影模式 = `UnityEngine.LightShadows`。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LightShadows {
    /// 不投阴影。
    #[default]
    None,
    /// 硬阴影（单次采样）。
    Hard,
    /// 软阴影（PCF）。
    Soft,
}

impl LightShadows {
    /// Unity 的枚举数值。
    pub const fn unity_value(self) -> u32 {
        match self {
            Self::None => 0,
            Self::Hard => 1,
            Self::Soft => 2,
        }
    }

    /// 从 Unity 枚举数值还原；未知值返回 `None`。
    pub const fn from_unity_value(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::None),
            1 => Some(Self::Hard),
            2 => Some(Self::Soft),
            _ => None,
        }
    }

    /// 是否投阴影（`Hard` / `Soft`）。
    pub const fn casts_shadows(self) -> bool {
        !matches!(self, Self::None)
    }
}

impl ReflectType for LightShadows {
    fn field_type() -> FieldType {
        FieldType::U8
    }

    fn enum_variants() -> Option<&'static [&'static str]> {
        Some(&["None", "Hard", "Soft"])
    }

    fn to_value(&self) -> Value {
        Value::U64(u64::from(self.unity_value()))
    }

    fn from_value(value: Value) -> Result<Self, ReflectError> {
        let raw = value.as_u64()?;
        let raw = u32::try_from(raw)
            .map_err(|_| ReflectError::BadData(format!("LightShadows 数值越界: {raw}")))?;
        Self::from_unity_value(raw).ok_or_else(|| {
            ReflectError::BadData(format!("invalid LightShadows discriminant {raw}"))
        })
    }
}

/// 光源渲染模式 = `UnityEngine.LightRenderMode`。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LightRenderMode {
    /// 由渲染器决定（逐像素还是逐顶点）。
    #[default]
    Auto,
    /// 强制逐像素。
    ForcePixel,
    /// 强制逐顶点。
    ForceVertex,
}

impl LightRenderMode {
    /// Unity 的枚举数值。
    pub const fn unity_value(self) -> u32 {
        match self {
            Self::Auto => 0,
            Self::ForcePixel => 1,
            Self::ForceVertex => 2,
        }
    }

    /// 从 Unity 枚举数值还原；未知值返回 `None`。
    pub const fn from_unity_value(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Auto),
            1 => Some(Self::ForcePixel),
            2 => Some(Self::ForceVertex),
            _ => None,
        }
    }
}

impl ReflectType for LightRenderMode {
    fn field_type() -> FieldType {
        FieldType::U8
    }

    fn enum_variants() -> Option<&'static [&'static str]> {
        Some(&["Auto", "ForcePixel", "ForceVertex"])
    }

    fn to_value(&self) -> Value {
        Value::U64(u64::from(self.unity_value()))
    }

    fn from_value(value: Value) -> Result<Self, ReflectError> {
        let raw = value.as_u64()?;
        let raw = u32::try_from(raw)
            .map_err(|_| ReflectError::BadData(format!("LightRenderMode 数值越界: {raw}")))?;
        Self::from_unity_value(raw).ok_or_else(|| {
            ReflectError::BadData(format!("invalid LightRenderMode discriminant {raw}"))
        })
    }
}

/// 色温（开尔文）→ 线性 RGB，对应 Unity `Mathf.CorrelatedColorTemperatureToRGB`。
///
/// Unity 的实现是 native 的；这里用常用的黑体辐射近似（Tanner Helland 的分段
/// 拟合），先按 sRGB 得到 0–1 分量、再转成线性空间，并裁到 `[1000, 40000]` K
/// （与 Unity Inspector 的取值范围一致）。结果**不**做归一化：6500 K 接近
/// `(1, 1, 1)`，色温只是乘到 `Light.color` 上的滤色片。
pub fn correlated_color_temperature_to_rgb(kelvin: f32) -> glam::Vec3 {
    let kelvin = if kelvin.is_finite() {
        kelvin.clamp(1000.0, 40000.0)
    } else {
        6500.0
    };
    let t = kelvin / 100.0;

    let srgb = |c: f32| {
        let c = c.clamp(0.0, 255.0) / 255.0;
        // sRGB → 线性：Unity 的灯颜色在渲染里是线性空间。
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };

    let r = if t <= 66.0 {
        255.0
    } else {
        329.698_73 * (t - 60.0).powf(-0.133_204_76)
    };
    let g = if t <= 66.0 {
        99.470_8 * t.ln() - 161.119_57
    } else {
        288.122_17 * (t - 60.0).powf(-0.075_514_846)
    };
    let b = if t >= 66.0 {
        255.0
    } else if t <= 19.0 {
        0.0
    } else {
        138.517_73 * (t - 10.0).ln() - 305.044_8
    };

    glam::Vec3::new(srgb(r), srgb(g), srgb(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use prism_reflect::ReflectType;

    #[test]
    fn unity_values_match_unity_ordering() {
        assert_eq!(CameraClearFlags::Skybox.unity_value(), 1);
        assert_eq!(CameraClearFlags::SolidColor.unity_value(), 2);
        assert_eq!(CameraClearFlags::Depth.unity_value(), 3);
        assert_eq!(CameraClearFlags::Nothing.unity_value(), 4);
    }

    #[test]
    fn from_unity_value_round_trips_and_rejects_unknown() {
        for flags in [
            CameraClearFlags::Skybox,
            CameraClearFlags::SolidColor,
            CameraClearFlags::Depth,
            CameraClearFlags::Nothing,
        ] {
            assert_eq!(
                CameraClearFlags::from_unity_value(flags.unity_value()),
                Some(flags)
            );
        }
        assert_eq!(CameraClearFlags::from_unity_value(0), None);
        assert_eq!(CameraClearFlags::from_unity_value(5), None);
    }

    #[test]
    fn clear_predicates() {
        assert!(CameraClearFlags::Skybox.clears_color());
        assert!(CameraClearFlags::SolidColor.clears_color());
        assert!(!CameraClearFlags::Depth.clears_color());
        assert!(!CameraClearFlags::Nothing.clears_color());

        assert!(CameraClearFlags::Skybox.clears_depth());
        assert!(CameraClearFlags::Depth.clears_depth());
        assert!(!CameraClearFlags::Nothing.clears_depth());
    }

    #[test]
    fn reflect_round_trip_uses_u8() {
        assert_eq!(CameraClearFlags::field_type(), FieldType::U8);
        for flags in [
            CameraClearFlags::Skybox,
            CameraClearFlags::SolidColor,
            CameraClearFlags::Depth,
            CameraClearFlags::Nothing,
        ] {
            let value = flags.to_value();
            assert_eq!(CameraClearFlags::from_value(value).unwrap(), flags);
        }
        assert!(CameraClearFlags::from_value(Value::U64(9)).is_err());
    }

    #[test]
    fn serde_uses_variant_names() {
        let json = serde_json::to_string(&CameraClearFlags::SolidColor).unwrap();
        assert_eq!(json, "\"SolidColor\"");
        let back: CameraClearFlags = serde_json::from_str("\"Depth\"").unwrap();
        assert_eq!(back, CameraClearFlags::Depth);
    }

    #[test]
    fn default_is_skybox() {
        assert_eq!(CameraClearFlags::default(), CameraClearFlags::Skybox);
    }

    #[test]
    fn light_type_matches_unity_ordering() {
        // Unity: Spot=0, Directional=1, Point=2, Rectangle(Area)=3, Disc=4,
        // Pyramid=5, Box=6, Tube=7。
        assert_eq!(LightType::Spot.unity_value(), 0);
        assert_eq!(LightType::Directional.unity_value(), 1);
        assert_eq!(LightType::Point.unity_value(), 2);
        assert_eq!(LightType::Rectangle.unity_value(), 3);
        assert_eq!(LightType::Tube.unity_value(), 7);

        for ty in [
            LightType::Spot,
            LightType::Directional,
            LightType::Point,
            LightType::Rectangle,
            LightType::Disc,
            LightType::Pyramid,
            LightType::Box,
            LightType::Tube,
        ] {
            assert_eq!(LightType::from_unity_value(ty.unity_value()), Some(ty));
        }
        assert_eq!(LightType::from_unity_value(8), None);
        assert_eq!(LightType::default(), LightType::Point);
    }

    #[test]
    fn only_implemented_light_types_are_rendered() {
        assert!(LightType::Spot.is_rendered());
        assert!(LightType::Directional.is_rendered());
        assert!(LightType::Point.is_rendered());
        assert!(!LightType::Rectangle.is_rendered());
        assert!(!LightType::Disc.is_rendered());
        assert!(!LightType::Pyramid.is_rendered());
        assert!(!LightType::Box.is_rendered());
        assert!(!LightType::Tube.is_rendered());
    }

    #[test]
    fn light_shadows_and_render_mode_match_unity() {
        assert_eq!(LightShadows::None.unity_value(), 0);
        assert_eq!(LightShadows::Hard.unity_value(), 1);
        assert_eq!(LightShadows::Soft.unity_value(), 2);
        assert!(!LightShadows::None.casts_shadows());
        assert!(LightShadows::Hard.casts_shadows());
        assert!(LightShadows::Soft.casts_shadows());
        assert_eq!(LightShadows::from_unity_value(3), None);

        assert_eq!(LightRenderMode::Auto.unity_value(), 0);
        assert_eq!(LightRenderMode::ForcePixel.unity_value(), 1);
        assert_eq!(LightRenderMode::ForceVertex.unity_value(), 2);
        assert_eq!(LightRenderMode::from_unity_value(3), None);
    }

    #[test]
    fn light_enums_expose_dropdown_variants() {
        assert_eq!(
            LightType::enum_variants().unwrap(),
            &[
                "Spot",
                "Directional",
                "Point",
                "Rectangle",
                "Disc",
                "Pyramid",
                "Box",
                "Tube"
            ]
        );
        assert_eq!(
            LightShadows::enum_variants().unwrap(),
            &["None", "Hard", "Soft"]
        );
        assert_eq!(
            LightRenderMode::enum_variants().unwrap(),
            &["Auto", "ForcePixel", "ForceVertex"]
        );
    }

    #[test]
    fn light_enums_reflect_round_trip_uses_u8() {
        // 反射用 u8 编码（与 `CameraClearFlags` 一致），这样不需要额外的类型表条目。
        for ty in [LightType::Spot, LightType::Directional, LightType::Point] {
            assert_eq!(ty.to_value(), Value::U64(u64::from(ty.unity_value())));
            assert_eq!(LightType::from_value(ty.to_value()).unwrap(), ty);
        }
        assert_eq!(LightType::field_type(), FieldType::U8);
        assert!(LightType::from_value(Value::U64(99)).is_err());
        assert!(LightShadows::from_value(Value::U64(99)).is_err());
        assert!(LightRenderMode::from_value(Value::U64(99)).is_err());
    }

    #[test]
    fn color_temperature_goes_warm_to_cool() {
        // 低色温偏红、高色温偏蓝，6500 K 接近中性白。
        let warm = correlated_color_temperature_to_rgb(2000.0);
        assert!(warm.x > warm.z, "2000K 应偏暖: {warm:?}");

        let cool = correlated_color_temperature_to_rgb(15000.0);
        assert!(cool.z > cool.x, "15000K 应偏冷: {cool:?}");

        let daylight = correlated_color_temperature_to_rgb(6500.0);
        for c in [daylight.x, daylight.y, daylight.z] {
            assert!((0.9..=1.1).contains(&c), "6500K 应接近白色: {daylight:?}");
        }
    }

    #[test]
    fn color_temperature_clamps_and_falls_back() {
        let lowest = correlated_color_temperature_to_rgb(-100.0);
        assert_eq!(lowest, correlated_color_temperature_to_rgb(1000.0));
        let highest = correlated_color_temperature_to_rgb(1.0e9);
        assert_eq!(highest, correlated_color_temperature_to_rgb(40000.0));
        // 非有限值退化为 6500 K，而不是产生 NaN。
        let nan = correlated_color_temperature_to_rgb(f32::NAN);
        assert!(nan.is_finite());
    }
}
