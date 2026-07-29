//! Push-constant 布局 shared between the PBR 渲染器 and `pbr.frag`.
//!
//! Kept in its own 模块 so the byte 布局 can be unit-tested against the
//! GLSL `layout(push_constant)` 块 in `shaders/pbr.frag`.

/// Selectable PBR 调试 visualization modes.
///
/// The numeric values 匹配 the `debug_mode` 推送 常量 consumed by
/// `pbr.frag` and the 叠加 按钮 order.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DebugMode {
    Final = 0,
    Albedo = 1,
    Specular = 2,
    Reflection = 3,
    Ambient = 4,
    Normal = 5,
}

impl DebugMode {
    /// All modes in overlay-button order.
    pub const ALL: [DebugMode; 6] = [
        DebugMode::Final,
        DebugMode::Albedo,
        DebugMode::Specular,
        DebugMode::Reflection,
        DebugMode::Ambient,
        DebugMode::Normal,
    ];

    /// Short 标签 used by the 叠加 UI.
    pub fn label(self) -> &'static str {
        match self {
            DebugMode::Final => "Final",
            DebugMode::Albedo => "Albedo",
            DebugMode::Specular => "Specular",
            DebugMode::Reflection => "Reflect",
            DebugMode::Ambient => "Ambient",
            DebugMode::Normal => "Normal",
        }
    }

    /// 转换 a `u32` (e.g. from 推送 constants / 输入 to a `DebugMode`,
    /// clamping out-of-range values to `Final`.
    pub fn from_u32(v: u32) -> Self {
        match v {
            0 => DebugMode::Final,
            1 => DebugMode::Albedo,
            2 => DebugMode::Specular,
            3 => DebugMode::Reflection,
            4 => DebugMode::Ambient,
            5 => DebugMode::Normal,
            _ => DebugMode::Final,
        }
    }
}

/// 坐标系 空间 used by the 法线 调试 众数
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NormalSpace {
    World = 0,
    View = 1,
    Tangent = 2,
}

impl NormalSpace {
    /// Cycle to the 下一个 空间 世界 → 视图 → 切线 → 世界
    pub fn next(self) -> NormalSpace {
        match self {
            NormalSpace::World => NormalSpace::View,
            NormalSpace::View => NormalSpace::Tangent,
            NormalSpace::Tangent => NormalSpace::World,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            NormalSpace::World => "World",
            NormalSpace::View => "View",
            NormalSpace::Tangent => "Tangent",
        }
    }

    /// 转换 a `u32` to a `NormalSpace`, clamping out-of-range to 世界
    pub fn from_u32(v: u32) -> Self {
        match v {
            0 => NormalSpace::World,
            1 => NormalSpace::View,
            2 => NormalSpace::Tangent,
            _ => NormalSpace::World,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_mode_values() {
        assert_eq!(DebugMode::Final as u32, 0);
        assert_eq!(DebugMode::Normal as u32, 5);
    }

    #[test]
    fn normal_space_cycle() {
        assert_eq!(NormalSpace::World.next(), NormalSpace::View);
        assert_eq!(NormalSpace::View.next(), NormalSpace::Tangent);
        assert_eq!(NormalSpace::Tangent.next(), NormalSpace::World);
    }
}
