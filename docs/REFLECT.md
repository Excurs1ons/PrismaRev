# 反射层与 RTTIBin

PrismaRev 把“工具格式”和“运行时格式”分开：RTTIBin 自带完整 schema，供编辑器、
独立 player 和外部工具使用；shipped RSCN 只存类型 id + schema 指纹，字段布局来自
编译进运行时的 `TypeDesc`。两者共用同一份反射描述。

## 分层

```
prism-reflect-derive   #[derive(Reflect)]  (proc-macro，仅编译期)
        │
        ▼
prism-reflect          Reflect / ReflectType / TypeDesc / Value / Object
        │              TypeRegistry（id → 描述 + 动态构造）
        │              rtti_bin（类型表 + 对象表的二进制）
        ▼
prism-scene            #[derive(Reflect)] 的组件 + ComponentRegistry
                       RSCN v4（type_id + schema_hash + 位置编码）
        │
        ▼
prism-cooker           SceneCooker（.scene.json → RSCN）
prism-engine           SceneLoader（RSCN → ECS）
```

- 反射层 **不依赖 egui**，也不依赖 `prism-engine` / `prism-render`；
  `prism-engine` 依赖它，用户项目（`game`）通过 engine 间接获得。
- `prism-reflect` 不依赖 `serde`。作者格式仍用 JSON + serde；RTTIBin 是编辑器、
  独立 player 与外部工具之间的自描述交换格式。

## 稳定类型 id

`TypeId` 不是 `std::any::TypeId`（每次编译都会变），而是：

```text
fnv1a64(规范化类型名)
```

规范化名字默认是 `module_path!()::TypeName`。组件用
`#[reflect(name = "prism_engine::scene::Camera")]` 固定成场景格式里的名字，
保证 cook / 运行时 / 编辑器三方一致。

**改名 = 换类型**。需要保留旧数据时，在 loader 里按 `TypeTable` 的名字做迁移，
不要复用旧的 id。

## 新增一个可反射组件

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize, Reflect)]
#[reflect(name = "prism_engine::scene::MyComponent", version = 1)]
pub struct MyComponent {
    pub speed: f32,

    // 运行时缓存：不参与反射，反序列化时保留 Default。
    #[reflect(skip)]
    pub cache: u64,

    // 编辑器不可写；反序列化仍可恢复（走 set_field_force）。
    #[reflect(read_only)]
    pub id: u32,

    // schema 里的字段名可以独立于 Rust 字段名。
    #[reflect(rename = "radius")]
    pub range: f32,
}
```

然后：

```rust
registry.register_reflect::<MyComponent>();
```

字段类型必须实现 `ReflectType`：

- 原生数值 / `bool` / `String`
- `Vec<T>` / `Option<T>`（`T: ReflectType`）
- `#[derive(Reflect)]` 的结构体
- `glam::Vec2/Vec3/Vec4/Quat`（内置实现，固定名 `glam::Vec3` 等）

嵌套结构体/glam 类型需要额外注册 **类型描述**（不是组件）：

```rust
registry.register_reflect_type::<glam::Vec3>();
```

`register_builtin_components()` 已注册内置组件用到的 glam 类型。

## 语义约定

- `Reflect: Default`。`from_object` 以 `Default` 起步：
  - 缺失字段 → 保留默认值（旧数据加载新代码）；
  - 未知字段 → 忽略（新数据加载旧代码时由调用方按 `TypeTable` 决定是否拒绝）；
  - 类型 id 不一致 → `ReflectError::TypeIdMismatch`。
- `set_field` 拒绝 `read_only` 字段；`set_field_force` 忽略该限制，供反序列化使用。
- `to_object()` 的字段顺序由 `BTreeMap` 决定，编码结果确定。

## RTTIBin 格式（v1）

小端。完整定义见 `crates/prism-reflect/src/rtti_bin.rs`。

```text
magic   "PRTB" (4)
version u16 = 1
types   u32
  id u64 | kind u8 | name str | version u32 | size u32 | align u32
  fields u16
    name str | field_type
objects u32
  type_id u64 | fields u16
    name str | value
```

- `str` = `u32` 字节长度 + UTF-8。
- 类型表按 `TypeId` 排序；对象字段按名字排序 → 同一输入字节确定。
- `field_type` tag：`0..=11` 原生类型，`12` = `Struct(TypeId)`，
  `13` = `List`，`14` = `Optional`，`15` = `Array`。
- `value` tag：`0` None、`1` Bool、`2` I64、`3` U64、`4` F64、
  `5` Str、`6` List、`7` Object。
- 解码有深度（64）与数量上限，损坏数据返回 `RttiError` 而不是 panic。

### 编解码 API

```rust
let bytes = rtti_bin::encode(&objects, registry.type_registry())?;
let decoded = rtti_bin::decode(&bytes)?;        // 不需要注册任何类型

for object in &decoded.objects {
    registry.apply_object(&mut world, entity, object); // 需要注册
}
```

编辑器可以只 `decode` + 读 `TypeTable` 来显示属性，不依赖游戏侧的组件注册表；
player 则按 `TypeId` 查 `ComponentRegistry` 还原成 ECS 组件。

## 现状与后续

已完成：

- 反射派生、注册表、RTTIBin 编解码与单元/集成测试。
- `TypeDesc::schema_hash()`：与内存布局无关的 schema 指纹（类型名 + version +
  字段名/类型/顺序），用于紧凑运行时格式的版本校验。
- 内置组件的反射：`Active`、`Name`、`LocalTransform`、`Camera`、
  `FlyCameraController`、`DirectionalLight`、`PointLight`、`SpotLight`、
  `Skybox`、`EnvironmentLighting`、`GiConfig`。
- `ComponentRegistry::apply_object` / `extract_object`，可在不知道具体类型
  的代码里读写组件。

### RSCN v4：运行时格式与工具格式分离

RTTIBin 是**交换/工具格式**：完整类型表 + 字段名，供编辑器、独立 player、
外部资产工具读取。它**不进入运行时热路径**，也不按场景重复存一份。

RSCN v4 的 runtime 负载只保留：

```text
type_id u64 | schema_hash u64        // 每个场景只列用到的类型一次
component payload                    // 按 TypeDesc 字段顺序，不含字段名/类型
```

加载时用编译进二进制的 `TypeDesc` 重新计算 `schema_hash`：

- 一致 → 按位置解码（仍经 `set_field`，不依赖 Rust 内存布局）；
- 不一致 → 旧版本走 migration，否则报错；编辑器/player 可退回 RTTIBin。

这样运行时不会带冗余 schema；工具侧仍能无编译期类型读取。`TypeId` 只哈希
类型名，不能替代 `schema_hash`。

### 谁负责 cook

职责按“schema / 构建期 / 运行时”拆开：

- **`prism-scene`**：组件定义、`ComponentRegistry`、`.scene.json` 作者格式、RSCN v4
  编解码。这是 cooker 与运行时共用的唯一 schema；它不依赖 render/audio/asset IO。
- **`prism-cooker`**：构建期编排。`SceneCooker` 解析并校验 `.scene.json`，
  调用 `prism-scene::rscn::encode_scene` 产出 RSCN v4；同时还负责纹理/网格等
  通用 importer/cooker、资产数据库与 `prism-cook` CLI。
- **`prism-build-pipeline`**：批处理入口（GI bake、场景 cook、heightmap）。
  `cook` 子命令直接调用 `SceneCooker`，不把 cooker 链带进游戏运行时。
- **`prism-engine`**：只负责运行时解码。`SceneLoader` 用同一份 `ComponentRegistry`
  把 RSCN 还原成 ECS 组件，并在之后注入 `MeshRef` / `MaterialRef` 等 GPU 绑定。
- **编辑器**：通过同一个 registry 做增量 cook / PIE 前 cook。

`prism-asset` 只保留运行时 `.pak`、`AssetId`/`Handle`、`ResourceManager`，
不再包含 cooker/importer/db/CLI。`prism-cook` 负责非场景资产；
场景资产由 `prism-build-pipeline cook` 或编辑器调用 `SceneCooker`。

### serde 的定位

组件仍 `derive(Serialize, Deserialize)` 是为了 `.scene.json` 作者格式和
`ComponentRegistry::register_reflect_serde` 的 JSON → `Object` 转换。
RSCN 的运行时载荷只走反射字段顺序，不写字段名；RTTIBin 才保留完整字段名与类型表。

