//! # prism-game-api —— 与引擎实现无关的 gameplay API 层
//!
//! 对齐 Unity 的脚本绑定层：**同一份 gameplay 代码**既能被静态链接进
//! Player，也能被编译成动态库由编辑器宿主加载。区别只在 [`GameWorld`]
//! 背后接的是谁：
//!
//! - **host**（当前实现）：直接借用宿主自己的 `prism_ecs::World`，零序列化。
//!   静态 Player、以及编辑器宿主自己跑的画面内项目代码走这条。
//! - **plugin**（后续）：通过宿主下发的 C 函数表读写，组件数据以 RTTIBin
//!   字节流跨边界。动态库里的 gameplay 代码走这条。
//!
//! 两条路径暴露**完全相同的函数签名**，所以项目代码不需要写两套。
//!
//! ## 边界规则（不要破坏）
//!
//! - 本 crate **不依赖** `prism-engine` / `prism-render` / `prism-app`。
//!   一旦这里出现它们的依赖，插件就必须链接第二份引擎，`World` 类型身份
//!   分裂，ABI 立刻变成 UB。
//! - 组件类型不在这里镜像：`Style` / `Text` / `Name` / `Transform` 等直接来自
//!   `prism-scene`（纯数据 crate），项目自己依赖即可。
//! - 跨边界只传 [`EntityHandle`] 这类 `#[repr(C)]` POD 与字节流，不传引用、
//!   不传 `&mut World`、不传 trait object。

use prism_ecs::{Component, Entity, World};
use prism_reflect::Reflect;

/// 跨 ABI 的实体句柄。
///
/// 与 [`prism_ecs::Entity`] 同构，但**不共享类型身份**：插件里的句柄只是
/// 宿主槽位的编号 + 代数，宿主会重新校验它是否仍然存活。
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct EntityHandle {
    pub id: u32,
    pub generation: u32,
}

impl EntityHandle {
    /// 空句柄；宿主必须拒绝它。
    pub const PLACEHOLDER: Self = Self {
        id: u32::MAX,
        generation: u32::MAX,
    };

    pub fn from_entity(entity: Entity) -> Self {
        Self {
            id: entity.id(),
            generation: entity.generation(),
        }
    }

    pub fn to_entity(self) -> Entity {
        Entity::from_raw(self.id, self.generation)
    }
}

/// 每帧下发给 gameplay 代码的输入快照。
///
/// 固定大小、`#[repr(C)]`，可以整块跨 ABI 传。按键用位图而不是 `Vec`，
/// 避免在边界上处理堆分配与释放责任。
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct InputSnapshot {
    /// 光标位置（像素，左上原点）。
    pub cursor: [f32; 2],
    /// 当前绘图区域尺寸（像素）。
    pub screen: [f32; 2],
    pub left_clicked: bool,
    pub left_held: bool,
    /// 按键位图：`keys[i / 64] & (1 << (i % 64))`。
    /// 键位编号由宿主定义（见 `prism-app` 的映射），插件只做查表。
    pub keys: [u64; Self::KEY_WORDS],
}

impl Default for InputSnapshot {
    fn default() -> Self {
        Self {
            cursor: [0.0; 2],
            screen: [0.0; 2],
            left_clicked: false,
            left_held: false,
            keys: [0; Self::KEY_WORDS],
        }
    }
}

impl InputSnapshot {
    /// 支持的键位数量。
    pub const KEY_COUNT: usize = 256;
    const KEY_WORDS: usize = Self::KEY_COUNT / 64;

    pub fn set_key(&mut self, index: usize, down: bool) {
        if index >= Self::KEY_COUNT {
            return;
        }
        let mask = 1u64 << (index % 64);
        let word = &mut self.keys[index / 64];
        if down {
            *word |= mask;
        } else {
            *word &= !mask;
        }
    }

    pub fn key(&self, index: usize) -> bool {
        index < Self::KEY_COUNT && (self.keys[index / 64] & (1u64 << (index % 64))) != 0
    }

    /// 本帧是否有任意键按下（intro 的「任意键跳过」用它）。
    pub fn any_key(&self) -> bool {
        self.keys.iter().any(|word| *word != 0)
    }
}

/// 一条 2D UI 四边形。
///
/// gameplay 代码不直接改渲染侧的 `UiDrawList`，而是提交这种中性描述，
/// 由宿主转换。这样插件既不需要链接 `prism-render`，也不用关心批次排序。
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct UiQuad {
    /// `[x, y, width, height]`，屏幕像素、左上原点。
    pub rect: [f32; 4],
    /// `[u0, v0, u1, v1]`。
    pub uv: [f32; 4],
    /// RGBA，线性空间。
    pub color: [f32; 4],
    /// 绘制顺序（越大越靠上）。
    pub depth: f32,
}

/// gameplay 代码看到的世界。
///
/// 所有方法都在**宿主的**世界上操作；插件后端会把同样的调用翻译成宿主
/// 回调，所以调用方无需区分自己是怎么被编译的。
pub struct GameWorld<'a> {
    world: &'a mut World,
    input: InputSnapshot,
    quads: Vec<UiQuad>,
}

impl<'a> GameWorld<'a> {
    /// 直接借用宿主世界（静态 Player / 编辑器宿主内）。
    pub fn host(world: &'a mut World) -> Self {
        Self {
            world,
            input: InputSnapshot::default(),
            quads: Vec::new(),
        }
    }

    // ------------------------------------------------------------------
    // 输入
    // ------------------------------------------------------------------

    pub fn input(&self) -> &InputSnapshot {
        &self.input
    }

    pub fn set_input(&mut self, input: InputSnapshot) {
        self.input = input;
    }

    pub fn screen_size(&self) -> [f32; 2] {
        self.input.screen
    }

    // ------------------------------------------------------------------
    // 实体
    // ------------------------------------------------------------------

    pub fn spawn(&mut self) -> EntityHandle {
        EntityHandle::from_entity(self.world.spawn())
    }

    pub fn despawn(&mut self, entity: EntityHandle) {
        self.world.despawn(entity.to_entity());
    }

    pub fn is_alive(&self, entity: EntityHandle) -> bool {
        self.world.is_alive(entity.to_entity())
    }

    /// 拥有组件 `T` 的全部实体（`Active` 语义与 `World::query` 一致）。
    pub fn query<T: Component + Reflect>(&self) -> Vec<EntityHandle> {
        self.world
            .query::<T>()
            .map(|(entity, _)| EntityHandle::from_entity(entity))
            .collect()
    }

    // ------------------------------------------------------------------
    // 组件
    // ------------------------------------------------------------------

    pub fn get<T: Component + Reflect + Clone>(&self, entity: EntityHandle) -> Option<T> {
        self.world.get::<T>(entity.to_entity()).cloned()
    }

    /// 写入（无则新增）组件。句柄失效时返回 `false`。
    pub fn set<T: Component + Reflect>(&mut self, entity: EntityHandle, value: T) -> bool {
        let entity = entity.to_entity();
        if !self.world.is_alive(entity) {
            return false;
        }
        self.world.insert(entity, value);
        true
    }

    /// 移除组件，返回是否确实存在过。
    pub fn remove<T: Component + Reflect>(&mut self, entity: EntityHandle) -> bool {
        self.world.remove::<T>(entity.to_entity()).is_some()
    }

    // ------------------------------------------------------------------
    // 2D 绘制
    // ------------------------------------------------------------------

    /// 提交一条 UI 四边形；宿主在 tick 结束后取走并合并进自己的绘制列表。
    pub fn draw_quad(&mut self, quad: UiQuad) {
        self.quads.push(quad);
    }

    /// 取走本帧累积的四边形（宿主侧调用）。
    pub fn take_quads(&mut self) -> Vec<UiQuad> {
        std::mem::take(&mut self.quads)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Reflect, Clone, Debug, Default, PartialEq)]
    struct Health {
        value: f32,
    }

    #[test]
    fn components_round_trip_through_handles() {
        let mut world = World::new();
        let mut api = GameWorld::host(&mut world);

        let entity = api.spawn();
        assert!(api.is_alive(entity));
        assert!(api.set(entity, Health { value: 3.0 }));
        assert_eq!(api.get::<Health>(entity), Some(Health { value: 3.0 }));
        assert_eq!(api.query::<Health>(), vec![entity]);

        assert!(api.remove::<Health>(entity));
        assert_eq!(api.get::<Health>(entity), None);
        assert!(api.query::<Health>().is_empty());
    }

    #[test]
    fn writes_to_dead_handles_are_rejected() {
        let mut world = World::new();
        let mut api = GameWorld::host(&mut world);

        let entity = api.spawn();
        api.despawn(entity);
        assert!(!api.is_alive(entity));
        assert!(!api.set(entity, Health { value: 1.0 }));
        assert_eq!(api.get::<Health>(entity), None);
    }

    #[test]
    fn recycled_slot_does_not_revive_stale_handle() {
        let mut world = World::new();
        let mut api = GameWorld::host(&mut world);

        let stale = api.spawn();
        api.despawn(stale);
        let fresh = api.spawn();

        // 同一个槽位被复用，但代数不同：旧句柄必须仍然失效。
        assert_eq!(stale.id, fresh.id);
        assert_ne!(stale.generation, fresh.generation);
        assert!(!api.is_alive(stale));
        assert!(api.is_alive(fresh));
    }

    #[test]
    fn quads_are_drained_once() {
        let mut world = World::new();
        let mut api = GameWorld::host(&mut world);

        api.draw_quad(UiQuad::default());
        assert_eq!(api.take_quads().len(), 1);
        assert!(api.take_quads().is_empty());
    }

    #[test]
    fn key_bits_round_trip() {
        let mut input = InputSnapshot::default();
        assert!(!input.any_key());

        input.set_key(7, true);
        input.set_key(200, true);
        assert!(input.key(7));
        assert!(input.key(200));
        assert!(!input.key(8));
        assert!(input.any_key());

        input.set_key(7, false);
        assert!(!input.key(7));
        assert!(input.key(200));

        // 越界写入被忽略，不会 panic。
        input.set_key(usize::MAX, true);
        assert!(!input.key(usize::MAX));
    }
}
