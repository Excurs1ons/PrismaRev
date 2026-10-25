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
}
