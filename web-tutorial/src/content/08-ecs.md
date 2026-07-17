# 08 · ECS 内核设计

图形管线解决「怎么画」，但引擎还要回答「画什么、谁来动」。PrismaRev 用 **ECS（Entity-Component-System）**——一种数据导向架构，而不是传统「GameObject 继承树」。ECS 内核在 `prism-ecs/src/lib.rs`，仅 626 行，却支撑了全部游戏逻辑。

:::info 为什么是 ECS 而非 OOP
Rust 的所有权模型不喜欢「对象互相持有引用」的 OOP 树。ECS 把对象拆成：**Entity（整数句柄）+ Component（纯数据）+ System（函数）**。数据连续存储、系统批量处理，既契合所有权，又对缓存友好（data-oriented）。
:::

## Entity：一个轻量整数句柄

实体不是对象，只是一个 `(id, generation)` 对。generation 在槽位回收时自增，使**过期句柄**与**新句柄**可区分：

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Entity {
    id: u32,
    generation: u32,
}
```

## Component：任何 `'static` 数据都是组件

引擎用** blanket impl** 免去了 derive 样板——任意纯数据自动是组件：

```rust
pub trait Component: 'static {}
impl<T: 'static> Component for T {}
```

所以你写的 `struct Transform { matrix: [[f32;4];4] }` 天然就是组件，无需 `#[derive(Component)]`。

## World：类型擦除的稀疏池

`World` 用 `HashMap<TypeId, Box<dyn ErasedPool>>` 按类型存组件池。每个池是「entity id → 值」的稀疏映射，所以实体可以**任意组合**组件（无需 archetype）：

```rust
pub struct World {
    entities: Vec<u32>,                 // 槽位当前 generation
    free: Vec<u32>,                     // 可回收槽位
    pools: HashMap<TypeId, Box<dyn ErasedPool>>,  // 每类型一个池
    resources: HashMap<TypeId, Box<dyn Any>>,     // 单例资源（如 Camera）
}
```

添加/删除组件是 O(1) 的池操作；`despawn` 时遍历所有池删掉该实体，并存入「下一个 generation」以便回收：

```rust
pub fn insert<T: Component>(&mut self, entity: Entity, component: T) {
    let pool = self.pools
        .entry(TypeId::of::<T>())
        .or_insert_with(|| Box::new(ComponentPool::<T>::new()));
    pool.as_mut().insert(entity.id, component);
}
```

:::warn 类型擦除需要 unsafe 下转
`dyn ErasedPool` 存的是类型擦除的池，`get::<T>()` 内部用 `downcast` 转回 `ComponentPool<T>`。这是引擎里少数的 `unsafe` 之一，但被 `TypeId` 严格保护——类型不匹配会直接返回 `None`，不会 UB。
:::

## Query：系统的数据入口

系统通过 `query` 拿到「同时拥有某些组件」的实体切片。`query2` / `query3` 支持多组件交集：

```rust
pub fn query2<A: Component, B: Component>(
    &self,
) -> impl Iterator<Item = (Entity, &A, &B)> {
    let pool_a = self.pools.get(&TypeId::of::<A>()).map(downcast);
    let pool_b = self.pools.get(&TypeId::of::<B>()).map(downcast);
    // 取两池交集，重建 (Entity, &A, &B)
}
```

:::tip 为什么多组件查询是交集
「拥有 Transform 且拥有 Mesh 且拥有 Material 的实体」才需要被渲染系统处理。ECS 的威力正在于：系统只声明它关心的组件组合，World 负责筛出匹配的实体——逻辑与数据彻底解耦。
:::

## 交互演示

下面用一张数据流图展示：Entity 持有组件 → System 用 Query 取出组件切片 → 写回结果（如录制命令）。点击不同按钮高亮不同部分：

（在页面下方查看交互演示）

:::exercise
1. 读 `crates/prism-ecs/src/lib.rs` 的全部 `query*` 方法，列出引擎支持几种组件组合查询。
2. 用 `World` 写一个最小例子：spawn 3 个实体，分别给其中 2 个加 `Transform`，用 `query::<Transform>()` 打印，验证第 3 个不在结果里。
3. 给 `World` 加一个 `resource` 读写示例（提示：`resources: HashMap<TypeId, Box<dyn Any>>`），理解「单例」与「实体组件」的区别。
4. 运行 `cargo test -p prism-ecs`，看 spawn/despawn/generation 的单元测试如何验证句柄回收的正确性。
:::

下一章，我们用 ECS 真正驱动渲染——相机、变换、Blinn-Phong 光照。
