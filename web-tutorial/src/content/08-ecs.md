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

`World` 用 `HashMap<TypeId, Box<dyn ErasedPool>>` 按类型存组件池。每个池的具体实现是**稀疏集**（`ComponentPool<T>`），用 `dense + sparse` 双数组取代朴素的 `HashMap`，获得更好的缓存局部性：

```rust
pub struct World {
    entities: Vec<u32>,                 // 槽位当前 generation
    free: Vec<u32>,                     // 可回收槽位
    pools: HashMap<TypeId, Box<dyn ErasedPool>>,  // 每类型一个池
    resources: HashMap<TypeId, Box<dyn Any>>,     // 单例资源（如 Camera）
}

// 每个组件池的内部实现：稀疏集
struct ComponentPool<T> {
    dense: Vec<(u32, T)>,  // (entity_id, component) 紧凑排列
    sparse: Vec<u32>,       // entity_id → dense 数组中的下标
    // SPARSE_NONE sentinel 表示该实体没有此组件
}
```

`dense` 是 `(entity_id, value)` 对的紧凑数组，迭代时连续访问，对 CPU 缓存友好（data-oriented 的核心体现）。`sparse` 用 `entity_id` 直接索引，O(1) 定位组件。添加/删除组件是 O(1) 的池操作；`despawn` 时遍历所有池删掉该实体，并存入「下一个 generation」以便回收：

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

## 原理探微：数据导向设计

ECS 被称作「数据导向」架构。data-oriented 不只是一个 buzzword——它直接决定了 `ComponentPool` 为什么用 `dense + sparse` 而不是 `HashMap`。

### 缓存行与遍历速度

假设你有 10,000 个实体，其中 5,000 个有 `Transform` 组件。用 `HashMap<u32, Transform>` 存储时，迭代这 5,000 个组件需要**随机访问 5,000 次不同的内存地址**，每次都可能 cache miss。而稀疏集的 `dense` 数组把 5,000 个 `(entity_id, Transform)` 对**连续排列**，一次 `for` 循环预加载整条缓存行：

```
dense: [(0, T), (3, T), (7, T), (19, T), ...]
       ↑cache line 1   ↑cache line 2   ↑cache line 3  → 全部命中 L1
```

这就是 `query::<Transform>()` 为什么快到可以每帧调用——内存读取模式对缓存友好。

### SoA vs AoS

ECS 本质上是一种 **SoA（Structure of Arrays）** 模式：

```
OOP: GameObject { Transform t, Mesh m, Material mat }  ← AoS，不同组件混合
ECS: Transform 池: [T, T, T, T, ...]                   ← SoA，同组件连续
     Mesh     池: [M, M, M, ...]
     Material 池: [Mat, Mat, ...]
```

SoA 让 CPU 的预取器（prefetcher）可以线性预测：遍历 `Transform` 池时只加载 `Transform` 数据，不会把无关的 `Mesh`/`Material` 拉进缓存。系统只读它需要的组件，不浪费带宽。

### 空查询（null query）

引擎实现了一个微优化：如果 `query::<T>()` 时 `T` 类型的池不存在，直接返回空迭代器，而不是插入一个空池。这避免了每次 `query` 都污染 `pools` HashMap：

```rust
pub fn query<T: Component>(&self) -> impl Iterator<Item = (Entity, &T)> {
    let pool = match self.pools.get(&TypeId::of::<T>()) {
        Some(p) => p,
        None => return QueryNone::new(),  // 空迭代器，不移入集合
    };
    // ... 正常迭代
}
```

## 原理探微：稀疏集查询算法

`query2<A, B>` 的算法不是「遍历实体检查是否同时有 A 和 B」——那样是 O(E) 且每次都要 HashMap 查找。稀疏集的查询利用了一个关键性质：**A 的 `dense` 数组就是 A 的所有实体列表**。

算法步骤：

```
query2<A, B>:
  1. 取两个池：选 dense 更小的那个做外层循环（比如 A 池 50 项 < B 池 2000 项）
  2. 遍历 A.dense: 对每个 (entity_id, &A):
     a. 查 B.sparse[entity_id] → dense 下标（O(1)）
     b. 如果 != SPARSE_NONE → 从 B.dense 拿到 &B
     c. yield (entity, &A, &B)
```

选择 **dense 更小的池做外层循环**是核心优化：如果 B 有 2000 项而 A 只有 50 项，只需做 50 次 B.sparse 查找（O(1)），而不是 2000 次。这就是稀疏集相比朴素「遍历所有实体」的绝对优势——**查询时间正比于匹配数，而不是总实体数**。

```rust id=ecs-query
pub fn query2<A: Component, B: Component>(
    &self,
) -> impl Iterator<Item = (Entity, &A, &B)> {
    let pool_a = self.pools.get(&TypeId::of::<A>()).map(downcast);
    let pool_b = self.pools.get(&TypeId::of::<B>()).map(downcast);
    // 取两池交集——选 dense 小的做外层
    match (pool_a, pool_b) {
        (Some(a), Some(b)) => {
            let iter = a.dense.iter().filter_map(move |&(id, ref ca)| {
                let idx = *b.sparse.get(id as usize)?;
                if idx == SPARSE_NONE { return None; }
                let (_, ref cb) = b.dense[idx as usize];
                Some((Entity::new(id, 0), ca, cb))
            });
            // ...
        }
        _ => QueryNone::new(),  // 任一池不存在 → 空结果
    }
}
```

## Query：系统的数据入口

系统通过 `query` 拿到「同时拥有某些组件」的实体切片。`query2` / `query3` 支持多组件交集：

```rust id=ecs-query
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
