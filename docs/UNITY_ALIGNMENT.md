# PrismaRev × Unity API 对齐计划（UNITY_ALIGNMENT）

> 目标：尽可能把 PrismaRev 的**运行时 API**与**编辑器 API**重构/补全到 Unity 的概念与命名，
> 以便熟悉 Unity 的使用者（和未来的工具链）能零成本迁移。
>
> 状态：**进行中（P1 起）**。本文件是权威计划；机器生成的清单/矩阵见文末附录。

---

## 0. 范围

- 参考基线：Unity 官方托管源码 `UnityCsReference`，tag **`7000.0.0a12`**（Unity 7 alpha）。
- **XR 相关全部排除**（`Modules/XR`、`Modules/XREditor`、`UnityEngine.XR*` 等）。
- “全量铺开”= 尽力覆盖，允许分阶段、允许对不可能 1:1 的概念做**语义等价**而非逐字照搬。

---

## 1. 参考基线（机器清点）

见 `docs/unity_api_inventory.md`：**8,987** 个 `.cs`、**7,071** 个 public 类型、
**308** 个命名空间（XR 已排除）。类型最多的命名空间：

| namespace | 类型数 | | namespace | 类型数 |
|---|---:|---|---|---:|
| `UnityEngine` | 925 | | `UnityEditorInternal` | 88 |
| `UnityEditor` | 741 | | `Unity.Properties` | 81 |
| `UnityEngine.UIElements` | 507 | | `UnityEditor.Experimental.GraphView` | 75 |
| `UnityEngine.Rendering` | 228 | | `Unity.Mathematics` | 74 |
| `Unity.Entities` | 222 | | `Unity.Collections` | 105 |
| `UnityEngine.PlayerLoop` | 143 | | `Unity.Jobs` | 18 |
| `UnityEditor.Search` | 117 | | `Unity.Burst` | 21 |

**重要事实**：Unity 的*核心*运行时类（`GameObject`/`Transform`/`Component`）在本仓库里是
**原生绑定（bindings）**，没有完整托管实现。因此运行时对齐是**概念/API 形状**对齐，
不是逐行移植。DOTS 侧（`Modules/Entities` 161 文件 + Collections/Mathematics/Burst/Jobs）
在本仓库里是**完整托管源码**，可直接对照。

### 现状差距（机械匹配，仅名称）

`docs/unity_gap_matrix.md`：在范围内 **5,056** 个类型里，按**名称**能对上 PrismaRev 现有
public 类型的只有 **43** 个（≈0.9%）。这既是现状，也是后续所有批次的工作清单。

---

## 2. 已锁定决策

1. **运行时走 ECS / DOTS 轨**。Rust 只做数据导向 ECS，因此运行时 API 以
   `Unity.Entities` / `Unity.Collections` / `Unity.Jobs` / `Unity.Mathematics` 为蓝本，
   **不**模拟 `MonoBehaviour` 那套 OOP 生命周期。
2. **`MonoBehaviour` 类脚本系统不实用，不做**。游戏逻辑继续用 Rust（system + 组件）。
3. **`PropertyDrawer` / `EditorWindow` 要做**——它们是**对 Tauri Web UI 的封装**，不是内嵌 C#。
4. **允许重命名已序列化组件/字段**，并**必须写迁移**（旧 `.scene.json` / RSCN 自动升级）。
5. 编辑器数据层仍受架构约束：`prism-editor-api` **不链接** engine/render/Vulkan；
   `PropertyDrawer`/`EditorWindow` 只是 **schema/DTO 投影**，真正 UI 在 `editor/`（Vue）。

---

## 3. 架构策略（双轨）

### 3.1 运行时轨：ECS / DOTS

| Unity（DOTS） | PrismaRev 现状 | 计划 |
|---|---|---|
| `Entity` | `prism_ecs::Entity` | 已对齐（保留） |
| `World` | `prism_ecs::World` | 已对齐；补 `EntityManager` 外观 |
| `EntityManager` | 无（`World` 兼） | **新增**：`CreateEntity/DestroyEntity/GetComponentData/SetComponentData/HasComponent/RemoveComponent/AddComponentData` |
| `ComponentSystemBase` / `ISystem` | 无（`prism_engine::Subsystem`） | **新增** trait 外观，桥到 `Subsystem` |
| `ComponentSystemGroup` | `ScheduleLabel` / 调度 | **新增** 分组外观（Init/Simulation/Presentation） |
| `LocalTransform` | `scene::LocalTransform` | 已同名；**字段** `translation/rotation/scale` → `Position/Rotation/Scale` |
| `LocalToWorld` | `scene::WorldTransform` | **重命名** `WorldTransform` → `LocalToWorld`（字段 `0` → `Value`） |
| `Parent` | `scene::Parent` | 已同名；字段 → `Value` |
| `Child`（buffer） | `scene::Children` | **重命名** `Children` → `Child`（元素 `Value`） |
| `Disabled` / `IEnableableComponent` | `scene::Active`（语义相反） | 保留 `Active`，或补 `Disabled` 兼容 tag（见 §4） |
| `DynamicBuffer<T>` | `Children(Vec<Entity>)` | 逐步引入 `DynamicBuffer` 形状 |
| `BlobAssetReference<T>` | 无 | 后期（P5+） |
| `IJob*` / `JobHandle` | 无（用 rayon/同步） | 后期（并行作业，P6） |
| `float3/quat/math` | `glam` | **不替换**；提供 `math` 兼容别名/转换层 |

> 设计原则：**PrismaRev 的核心数据结构（`World`/`Entity`/`Component`）保留**，
> 在其上叠加一层**DOTS 形状的门面（facade）**，避免破坏现有系统与渲染器。
> 只有在迁移收益明确（如 `LocalToWorld`/`Child`）时才真正重命名。

### 3.2 编辑器轨：Web UI 封装

| UnityEditor 概念 | PrismaRev 落点 | 形式 |
|---|---|---|
| `AssetDatabase.*` | `prism_editor_api::asset_database` | Rust 模块（包装 `assets.rs`/`asset_ops.rs`） |
| `AssetImporter.GetAtPath` / `TextureImporter` 等 | `prism_editor_api::asset_importer` | Rust 模块（按扩展名投影导入器类名 / 主资产类型）+ `AssetInfo.importer` |
| `PrefabUtility.*` | `prism_editor_api::prefab_utility` | Rust 模块（包装 `scene.rs` 预制体函数） |
| `Selection.*` | `prism_editor_api::selection` + 前端 store | 前后端各一半（前端有真实选中态） |
| `Undo.*` | `prism_editor_api::undo` + 前端历史栈 | 同上（前端已有按字段合并的撤销） |
| `EditorApplication.*` | `prism_editor_api::editor_application` | 包装 `play.rs`/`settings.rs` |
| `BuildPipeline` | `prism_editor_api::build_profile` | 构建 profile / 目标组 / 场景构建顺序（执行仍在 `editor/src-tauri`） |
| `PropertyDrawer` / `[CustomPropertyDrawer]` | `prism_editor_api::property_drawer` | 注册表声明“字段渲染器”，经 schema 的 `drawer` 传给 Vue |
| `EditorWindow` / `[MenuItem]` | **面板/命令注册表** | `PANELS` + `MENUS`（前端）由后端 DTO 驱动 |
| `EditorUtility` | `prism_editor_api::editor_utility` | 包装 reveal/日志/对话框 |
| `SerializedObject`/`SerializedProperty` | `reflect_edit` + schema | 已有反射写入，补 Unity 命名外观 |

> `PropertyDrawer`/`EditorWindow` 的“脚本”形态：**不是**用户写 C#，而是
> 后端返回**声明式 DTO**（字段引用哪个渲染器、窗口声明哪个 slot/菜单项），
> 前端 Vue 消费。这样既得到 Unity 的扩展体验，又不违背后端不链接 UI 的约束。

---

## 4. 命名与迁移策略

- **令牌来源**：场景组件的 canonical 名是 `#[reflect(name = "prism_engine::scene::X")]`，
  同时是 `.scene.json` 的 `components` 键、RSCN 的 `type_id` 输入。
- **重命名 = 破坏性**：会同时改变 reflect 名、JSON 键、RSCN `type_id`。
  必须配套**迁移**，否则 `from_object` 会“忽略未知字段 → 静默丢数据”。
- **迁移机制**（新增于 `prism-editor-api`，纯数据、可单测）：
  1. `.scene.json` 顶层 `version`（当前缺省 `1`）递增；
  2. `migrate_doc(&mut Value)` 按版本逐级重命名**组件键**与**字段键**；
  3. `read_doc` 读入后先迁移再返回；`write_doc` 总是写最新版本；
  4. 引擎开发加载器（JSON→ECS）同样先迁移（复用同一函数）。
- **RSCN**：`type_id` 变了，`.rscn` 由 cooker 重烤即可；`schema_hash` 不匹配是硬错误，
  与重命名无冲突（重烤即可）。
- **字段重命名的 serde 归属**：新版结构体用 `#[serde(rename_all = "PascalCase")]`
  或逐字段 `#[serde(rename = "…")]` 输出 Unity 形状；迁移负责把旧小写键改成新键。

### 已确认的重命名映射（累进，见各批次）

| 旧（PrismaRev） | 新（Unity/DOTS 形状） | 影响 | 状态 |
|---|---|---|---|
| `scene::WorldTransform` | `scene::LocalToWorld` | 组件名（未序列化，无需迁移）；新增 `value()` 访问器对应 DOTS `.Value` | ✅ P1 |
| `scene::Children` | `scene::Child` | 组件名（未序列化，无需迁移）；新增 `value()`/`value_mut()` | ✅ P1 |
| `LocalTransform.translation` | `LocalTransform.position`（及作者 JSON `transform.position`） | **已序列化** → 迁移 v1→v2 | ✅ P1 |
| `scene::Active` | （保留；视需要补 `Disabled`） | 语义相反，谨慎 | ⏳ 未定 |

> Rust 侧的 `LocalToWorld` / `Child` / `Parent` 仍是 **newtype（tuple）**：`.0` 就是 DOTS 的
> `.Value`，另提供 `value()` 访问器。作者 JSON 的 `transform` 与 `LocalTransform` 组件载荷
> 现在统一使用 `position` / `rotation` / `scale`。

---

## 5. 范式映射（速查）

### 5.1 运行时

| Unity | PrismaRev |
|---|---|
| `GameObject` | `Entity`（+ 组件集合） |
| `Transform`（层次/父子） | `Parent`/`Child` + `LocalTransform`/`LocalToWorld` |
| `Object.name` | `scene::Name` |
| `Object.Destroy/Instantiate` | `World::despawn`/`spawn` + 预制体实例化 |
| `Object.activeSelf` / `enabled` | `scene::Active`（+ 未来 `Disabled`） |
| `Camera` | `scene::Camera` |
| `Light`（Directional/Point/Spot） | `scene::DirectionalLight`/`PointLight`/`SpotLight` |
| `RenderSettings.skybox` | `scene::Skybox` |
| `Lightmapping`/GI | `scene::GiConfig` + IBL |
| `MeshRenderer`/`MeshFilter` | `scene::MeshRenderer`（`mesh_path`/`material_path`） |
| `Input.GetKey/GetMouseButton/GetAxis` | `prism_engine::input_facade::Input`（借用 `InputManager`） |

### 5.2 编辑器

| UnityEditor | PrismaRev |
|---|---|
| `EditorGUI*` / `EditorGUILayout*`（IMGUI） | Vue 组件 + `FieldEditor.vue`/`NumberField.vue`/`ColorField.vue` |
| `SerializedProperty` | `schema.rs` 的 `FieldSchema` + `reflect_edit` |
| `PropertyDrawer` | schema 的“字段渲染器”声明（后端 DTO） |
| `EditorWindow`/`Overlay` | dock 面板（`PANELS`）+ 浮动窗口 |
| `[MenuItem]` | `MENUS`（`App.vue` computed） |
| `Undo.RecordObject` | 前端历史栈（按字段合并） |
| `Selection.objects` | 前端选中态 + `selection` 模块 |
| `AssetDatabase` | `asset_database` 模块 |
| `PrefabUtility` | `prefab_utility` 模块 |
| `EditorApplication.isPlaying` | `play.rs` 预览进程状态 |
| `BuildPipeline` | `build_profile` 模块（profile / 目标组 / 顺序；执行在 Tauri） |

### 5.3 明确不适用（N/A）

- `MonoBehaviour` 及其消息（`Awake/Start/Update/OnEnable/OnDisable/OnDestroy`）——
  由 ECS system + 调度代替，**不提供**同名 C# 形状。
- C# 协程 / `yield` / `PlayerLoop` 的托管形状——由 system 顺序代替。
- 托管 GC / `ScriptableObject` 的托管生命周期——用 Rust 资源（`World::insert_resource`）。
- Input Manager **命名轴**（`Input.GetButton("Fire1")`）与传感器/文本输入
  （`Input.location` / `compass` / `acceleration` / `inputString` / IME）——
  没有 `InputManager.asset`，键位直接查 `KeyCode`；鼠标三条内置轴由
  `Input::get_axis` 提供，触摸走 `Input::touches`。
- XR 全家（按范围排除）。

---

## 6. 分批计划

> 每批都可独立 `cargo build` / `cargo test` 验证。**先文档（P0，本文件）→ 再实现。**

- **P0 计划与清单**（本文件 + 两份生成附录）— ✅ 完成。
- **P1 运行时核心** — ✅ 完成（见 §9）。`prism-ecs` 补 `EntityManager` /
  `ISystem`/`SystemBase`/`SystemGroup`；`prism-scene` 变换与层次重命名为
  `LocalToWorld`/`Child`/`Parent`；作者格式字段 `translation`→`position` +
  **迁移框架**（`prism-scene::migrate`）。
- **P2 编辑器核心** — ✅ 完成（见 §9）。`prism-editor-api` 新增六个 Unity 形状模块：
  `asset_database` / `prefab_utility` / `selection` / `undo` / `editor_application` /
  `scene_management`（与既有导出同名的函数只在模块路径下提供，避免根命名冲突）。
- **P3 渲染/光照/相机** — 🔄 进行中。P3a（相机）与 P3b（光照：渲染枚举、
  `Light` 组件、统一收集 + 聚光进 GPU、逐渲染器 `ShadowCastingMode`、编辑器光源预设、
  P3c（相机行为）见 §9 待办。
- **P4 编辑器扩展** — ✅ 完成（见 §9）。P4a `PropertyDrawer` schema 扩展点与
  P4b `EditorWindow` 面板注册表 DTO 已落地。
- **P5 资产流水线** — 🔄 进行中。P5a `AssetImporter` 形状与 P5b `Build`/`Build.Profile` 已落地（见 §9）；剩余 `Compilation` 形状。
- **P6 并行/集合**：`Unity.Collections`/`Unity.Jobs` 形状（`NativeArray`/`IJob*`/`JobHandle`）。
- **P7 UI/输入/音频/动画/物理** — 🔄 进行中（见 §9）。P7a `UnityEngine.Input`
  门面（`prism_engine::input_facade::Input`）已落地；其余 `UIElements`/`UI`/
  `Audio`/`Animation`/`Physics` 按其体量单独排期（`UIElements` 507 类型最大）。

---

## 7. 实施约定

- 运行时：**保留核心结构**，叠加 Unity 形状门面；确需重命名时走 §4 迁移。
- 编辑器：`prism-editor-api` 保持 UI/Vulkan/engine-free；新模块只做 DTO 投影与文件 IO。
- 每个 Unity 形状 API 都要有**简洁文档注释**指明其 Unity 对应物（便于对照）。
- 保持向后兼容别名一整个批次（例如 `WorldTransform` 保留为 `LocalToWorld` 的
  `#[deprecated]` 别名），下一批再删。
- 不新增 UI 框架依赖；不把 cooker/build 拉进运行时链。

## 8. 验证

- `cargo check -p prism-ecs -p prism-scene -p prism-reflect -p prism-editor-api`
- `cargo test -p prism-ecs -p prism-scene -p prism-editor-api`（含迁移单测）
- `cd projects/prisma && cargo tree | grep -i egui`（必须为空）
- 编辑器前端：`cd editor && pnpm build`
- **本机（Termux/Android）完整验证的方法**（2026-10-25 打通，取代此前"引擎无法编译"的结论）：
  ```bash
  # 1) 假 pkg-config：让 alsa-sys 通过（cargo check 不链接，不需要真的 libasound）
  export PKG_CONFIG=/tmp/opencode/fakepc/bin/pkg-config-any
  # 2) `.spv` 是 gitignore 的：从主仓库或 CI artifact 拷进工作区
  cp <main-repo>/assets/shaders/*.spv assets/shaders/
  # 3) 现在这些都能过
  cargo check --workspace --all-targets
  (cd projects/prisma && cargo check --all-targets)
  (cd projects/prisma-sponza && cargo check --all-targets)
  (cd editor/src-tauri && cargo check)
  (cd launcher/src-tauri && cargo check)
  ```
  限制：`cargo build` 生成二进制时仍会因缺 `libasound` 在**链接**阶段失败；Android 需要 NDK。

- **着色器（2026-10-25 验证）**：官方 release 有 `linux-aarch64` 构建，
  在 aarch64 的 **glibc** 用户态（本项目宿主的 Ubuntu-on-Termux）上可直接运行：
  ```bash
  # 版本必须与 CI 一致，否则 reflection JSON 会漂移（见下）
  curl -fsSLO https://github.com/shader-slang/slang/releases/download/v2026.13.1/slang-2026.13.1-linux-aarch64.tar.gz
  tar xzf slang-2026.13.1-linux-aarch64.tar.gz
  export PATH=$PWD/bin:$PATH LD_LIBRARY_PATH=$PWD/lib
  apt-get install -y spirv-tools      # compile.sh 的 fix_spirv.py 需要 spirv-dis
  bash assets/shaders/compile.sh
  ```
  注意两点：
  1. **纯 Termux（bionic）跑不了**这个 glibc 二进制；那种环境仍用 CI 的 `spirv` artifact。
  2. 本地重编会让 `assets/shaders/reflection/*.json` 出现文本漂移（新版 slangc 多输出
     `combined` / `bindlessSpaceIndex` 等字段）。已验证这些额外字段**不影响**
     `xtask shader-bindgen` 的产物（生成的 Rust bindings 与提交版本逐字节相同），
     所以改着色器时**不必**连带提交 reflection 的文本漂移，只提交变动的 bindings 即可。

---

## 9. 进度（Progress）

### P1 — 运行时核心 ✅

**1) `prism-ecs`：DOTS 形状门面（纯加法，不动核心结构）**
- `crates/prism-ecs/src/entity_manager.rs` — `EntityManager<'w>`：`create_entity` /
  `destroy_entity` / `exists` / `is_enabled` / `set_enabled` / `add_component` /
  `set_component_data` / `get_component_data(_mut)` / `has_component` / `remove_component`
  （对应 `EntityManager.CreateEntity/DestroyEntity/Exists/SetEnabled/AddComponent/...`）。
  另有 `World::entity_manager()` 便捷构造。
- `crates/prism-ecs/src/systems.rs` — `ISystem`（`on_create`/`on_start_running`/
  `on_stop_running`/`on_update`/`on_destroy`）、闭包 blanket impl、`SystemBase<F>`、
  `SystemGroup`（`initialization()`/`simulation()`/`presentation()`，维护 running 状态迁移）、
  别名 `ComponentSystemGroup`，以及组名常量。
- 6 个新单测；`cargo test -p prism-ecs` → **18 passed**。

**2) `prism-scene`：变换/层次命名对齐（未序列化，无需迁移）**
- `WorldTransform` → `LocalToWorld`（+ `value()` 访问器）。
- `Children` → `Child`（+ `value()`/`value_mut()`）；`Parent` 加 `value()`。
- 影响 17 个文件（engine/scene/build-pipeline/scene），纯标识符替换。

**3) 作者格式字段重命名 + 迁移框架（`translation` → `position`）**
- 新增 `crates/prism-scene/src/migrate.rs`：
  - `SCENE_DOC_VERSION = 2`；`doc_version()`（缺失/0 视为 1）；
  - `migrate_scene_doc(&mut Value)`，幂等、拒绝“未来版本”；
  - v1→v2：实体 `transform.translation` → `transform.position`，以及
    `prism_engine::scene::LocalTransform` 组件载荷内的同名字段。
  - 7 个单测。
- `SceneJson::parse(text)` / `SceneJson::from_value(value)`：**先迁移再反序列化**。
- 三个读取路径全部改走它：`prism-cooker::cooker::scene`、`prism-engine::scene::loader`、
  `prism-editor-api::scene::{read_doc, validate_doc}`。
- 写路径统一写 `SCENE_DOC_VERSION`（`prism-editor-api` 14 处硬编码 `"version": 1` 全部替换）。
- 同步改名：`TransformJson.translation`→`position`、`LocalTransform.translation`→`position`、
  `rscn` 的 `DecodedEntity`、loader 的 `ParsedEntity`、`scene_state` 的快照 JSON 键、
  `presets::EntityPreset.translation`→`position`，以及编辑器前端
  `Inspector.vue` / `types.ts` / `App.vue`（并 `pnpm fixture` 重新生成 `editor/public/demo.json`）。
- **磁盘上现有的 `assets/scenes/*.scene.json` 仍是 v1**（用 `translation`）——这正是迁移的
  用途：读取时透明升级，编辑器保存时写成 v2。

**验证**
- ✅ `cargo test -p prism-ecs -p prism-scene -p prism-editor-api -p prism-cooker`
  （18 / 17 / 51 / 109+6+3 全绿）。
- ✅ `cd editor && pnpm build`（`vue-tsc --noEmit` + vite build 通过）。
- ⚠️ `prism-engine` / `prism-build-pipeline` 在**本机（Termux/Android）无法编译**：依赖链
  `prism-engine → prism-audio → firewheel(cpal) → alsa-sys`，需系统 `alsa`，本机缺 `pkg-config`
  与 `alsa.pc`。引擎侧改动是对称的机械重命名（`LocalTransform`/`TransformJson`/
  `ParsedEntity`/快照键），需在桌面主机上 `cargo check -p prism-engine` 复核。

### P2 — 编辑器核心 ✅

全部落在 `crates/prism-editor-api/src/`（保持 UI / Vulkan / engine-free，只做 DTO 投影 + 文件 IO）。
每个模块顶部都有「Unity API → 这里」的对照表，并注明差异。

**1) `asset_database.rs`（`UnityEditor.AssetDatabase`）**
`asset_path_to_guid` / `guid_to_asset_path` / `is_valid_guid`（**路径派生的 32 位十六进制
GUID**，非 `.meta`）、`get_all_asset_paths`、`find_assets`（`t:<kind>` + 名字子串 AND 过滤，
`l:` 忽略）、`contains`、`is_valid_folder`、`get_main_asset_type_at_path`、
`get_asset_dependency_hash`（路径+大小+mtime 指纹）、`create_folder`、`delete_asset`、
`move_asset` / `rename_asset` / `copy_asset`（新增递归复制；与 Unity 一致不改写引用）、
`import_asset`、`refresh` / `save_assets`、`load_asset_at_path`、`asset_absolute_path`、
`ASSETS_FOLDER`。

**2) `prefab_utility.rs`（`UnityEditor.PrefabUtility`）** — 架在 `scene.rs` 原语之上
`save_as_prefab_asset`（= `extract_subtree`）、`save_as_prefab_asset_and_connect`
（提取 + **就地替换成实例**、返回 `PrefabConnect{asset,scene,path}`）、`instantiate_prefab`、
`apply_prefab_instance`（写回预制体文件）、`revert_prefab_instance`、`sync_prefab_instances`、
`unpack_prefab_instance`（`PrefabUnpackMode` 两取值等价）、`get_prefab_asset_type`
（Regular/Variant/NotAPrefab）、`get_prefab_instance_status`、`is_part_of_prefab_instance`、
`get_nearest_prefab_instance_root`、`get_corresponding_object_from_source`、
`get_prefab_instance_roots` / `prefab_instance_infos`、`read_prefab`、`DEFAULT_PREFAB_FOLDER`。

**3) `selection.rs`（`UnityEditor.Selection`）**
`SelectionState{indices, active}` + `objects`/`game_objects`/`active_object`/`contains`
（`active` 的 getter 与 setter，setter 是「唯一选中」语义）、`add`/`remove`/`toggle`/
`set_indices`/`retain_valid`、`get_filtered`（`SelectionMode`：All/Root/PrefabRoot/HasComponent）。

**4) `undo.rs`（`UnityEditor.Undo`）**
泛型 `UndoStack<T>`（默认上限 128）+ `UndoRecord<T>`：`record_object`、
`register_created_object_undo`、`register_full_object_hierarchy_undo`、`perform_undo`/
`perform_redo`（返回应恢复的快照）、`can_undo`/`can_redo`/`undo_count`/`redo_count`、
`undo_label`/`redo_label`、`collapse_undo_operations`、`clear_all`；别名
`DocUndoStack = UndoStack<serde_json::Value>`。

**5) `editor_application.rs`（`UnityEditor.EditorApplication`）**
`EditorApplicationState{play_mode: PlayModeState, is_compiling, is_updating, active_scene,
play_scene, delayed_calls}` + `is_playing`/`is_paused`/`is_playing_or_will_change_playmode`、
`enter_playmode`/`pause`/`resume`/`exit_playmode`、`delay_call`/`run_delayed_calls`；
路径 `project_path`/`data_path`/`streaming_assets_path`/`application_path`、`engine_version`。

**6) `scene_management.rs`（`SceneManager` + `EditorSceneManager`）**
`Scene` DTO（name/path/guid/is_dirty/is_loaded/root_count/entity_count/build_index）、
`SceneManagerState{loaded, active}`（`load_scene`/`unload_scene`/`set_active_scene`/
`scene_count`/`get_scene_at`/`active_scene`，`LoadSceneMode{Single,Additive}`）；
编辑器侧 `new_scene` / `open_scene` / `open_scene_at_path` / `save_scene` / `save_scene_as` /
`save_open_scenes` / `scene_count` / `get_scene_at` / `get_loaded_scenes` / `resolve_scene` /
`scene_from_doc`（后三者会**读取文档**填充实体/根计数）。

**验证**：`cargo test -p prism-editor-api` → **73 passed**（51 旧 + 22 新）；
`cargo check --workspace --exclude prism-engine --exclude prism-build-pipeline
--exclude prism-app --exclude prism-editor-tool --exclude prism-audio` → 干净无警告。

**发现（测试暴露的真实约束）**：`scene::revert_prefab_instance` 删除子树后仍要求场景
「至少保留一个实体」（`SceneJson::validate`），因此**把场景里唯一实体还原成实例会失败**。
这是既有 `delete_entities` 的不变量，本次未改动其语义，只在测试里用一个额外的兄弟实体体现。

### P2 接线：Tauri 命令层 + Vue 前端 ✅

把门面真正接到编辑器上（`editor/src-tauri/src/lib.rs` 新增 27 个命令，`editor/src/api.ts`
同名包装，`editor/src/types.ts` 补 DTO）。

**AssetDatabase（11 个命令）**
`editor_asset_guid` / `editor_asset_path_from_guid` / `editor_asset_all_paths` /
`editor_find_assets` / `editor_asset_main_type` / `editor_asset_dependency_hash` /
`editor_asset_contains` / `editor_asset_is_valid_folder` / `editor_copy_asset` /
`editor_import_asset`（返回 `AssetImportInfo{path,kind}`）/ `editor_refresh_assets`。

**PrefabUtility（6 个）**
`editor_apply_prefab_instance` / `editor_unpack_prefab_instance`（`mode` =
`outermost_root`|`completely`）/ `editor_prefab_asset_type` / `editor_prefab_instance_infos` /
`editor_nearest_prefab_instance_root` / `editor_is_part_of_prefab_instance`。

**SceneManagement（6 个）**
`editor_scenes_meta` / `editor_scene_meta_count` / `editor_scene_meta_at` /
`editor_resolve_scene` / `editor_new_scene` / `editor_scene_from_doc`。

**Selection（2 个，无状态）**
`editor_selection_filtered`（给定下标集合上过滤 = `Selection.GetFiltered`）/
`editor_selection_all_matching`（对文档全部实体过滤，「选择所有同类型」）。

**EditorApplication（2 个）**
`editor_engine_version` / `editor_paths`（`EditorPaths{project,data,streamingAssets,application}`）。

**前端实际用起来的**
- 资产右键菜单新增 **复制资产**（`CopyAsset` + `-2` 去重后缀，等价 Unity `Ctrl+D`）
  与 **复制 GUID**。
- 资产检索支持 **Unity 过滤语法**：查询里出现 `t:`/`l:`/`a:` 时改走
  `editor_find_assets`（多词 AND + 分类条件），否则保持本地子串过滤以维持跟手；
  后端不可用（纯浏览器预览）时自动退回本地过滤。
- 「应用到预制体」从「`extractSubtree` + `writeDocument`」两步改为一次
  `api.applyPrefabInstance`（即 `PrefabUtility.ApplyPrefabInstance`）。
- 「关于」面板显示 `editor_engine_version`。

**刻意没有搬到后端的**：`SelectionState` 与 `UndoStack` 的**状态机**仍由前端（Vue 响应式）
持有——拖动/输入要求零往返，且现有撤销已按字段合并连续输入。Rust 侧的门面保留为
API 完整性 + 单测覆盖；前端只消费它们的**无状态**部分（见上）。

**验证**
- ✅ `cd editor && pnpm build`（`vue-tsc --noEmit` + vite build）通过。
- ✅ 命令调用形状用一次性 `crates/prism-editor-api/examples/wire_check.rs` 逐条镜像做过
  类型检查（已删除）——它当场抓出一个真 bug：`get_main_asset_type_at_path` 返回
  `&'static str` 而非 `String`，命令层已改为 `.map(str::to_string)`。
- ⚠️ `editor/src-tauri` 在 Termux 上**无法编译**（`glib-2.0`/webkit2gtk 缺失），
  需在桌面主机 `cd editor/src-tauri && cargo check` 复核。

### P3a-1 — 相机 schema 对齐（prism-scene）✅

**prism-scene（纯加法）**
- 新增 `crates/prism-scene/src/render_enums.rs`：`CameraClearFlags`
  （`Skybox` / `SolidColor` / `Depth` / `Nothing`，取值与 Unity 的 1–4 一致），
  附 `unity_value()` / `from_unity_value()` / `clears_color()` / `clears_depth()`；
  反射走 `ReflectType → FieldType::U8`（与 `ui::TextAlign` 同一套编码，因此**不需要**
  `register_reflect_type`）；serde 用变体名，和 `.scene.json` 里其它枚举一致。
- `Camera` 追加 5 个字段（对齐 `UnityEngine.Camera`）：
  `clear_flags`、`background_color: [f32;4]`、`orthographic`、
  `orthographic_size`、`depth`。全部有 serde 默认值 → **老文档不需要迁移**
  （`SCENE_DOC_VERSION` 保持 2）。
- `impl Camera`：`projection_matrix(aspect)`（透视/正交，Vulkan y-flip + 深度 `[0,1]`，
  `aspect` 非有限时退化为 1.0）、`world_to_camera_matrix()` / `camera_to_world_matrix()`
  （对应 Unity `worldToCameraMatrix` / `cameraToWorldMatrix`）。
- 新增 `components_tests.rs`（8 个测试）：y-flip、`near→0` / `far→1`、正交半高/半宽、
  退化 aspect、矩阵互逆、默认值、JSON 往返、**缺字段的老 JSON 仍可反序列化**。
- `prism-engine/src/engine.rs` 里逐字段构造 `Camera` 的 demo 场景改用
  `..Camera::default()`（新增字段后仍可编译）。

**验证**
- ✅ `cargo test -p prism-scene` → 31 passed。
- ✅ `cargo check --workspace --all-targets`、`projects/prisma`、`projects/prisma-sponza`、
  `crates/xtask`、`editor/src-tauri`、`launcher/src-tauri` 全部通过。
- ✅ `cd editor && pnpm fixture && pnpm build`（fixture 因 Camera 新增字段而更新）。
- **本机编译方案（2026-10-25 打通，此前"引擎无法编译"的结论作废）**：Termux 上缺
  `alsa.pc` 与 `slangc`，用
  1. 一个假 `pkg-config`（`PKG_CONFIG=...`，`--modversion` 返回 99.0.0）+ 假 `alsa.pc`；
  2. 从主仓库 `assets/shaders/*.spv` 拷入 worktree（`.spv` 是 gitignore 的）
  之后 `cargo check` 全链路可用。**注意 `cargo check` 不链接**：真正 `cargo build`
  二进制仍会因缺 `libasound` 在链接阶段失败，Android 交叉编译需要 NDK。

**这一批刻意没做**（后续 PR）
- 引擎侧接线：`projection_matrix` 真正被用上、无 `FlyCameraController` 时回退到
  `LocalToWorld`、`Camera.main`/`allCameras`、清屏色跟随相机。
- 检查器里 `clear_flags` 仍是 `u8` 数字框；下拉框需要 `FieldSchema.variants`。

**已知遗留**
- `clear_flags` 的 `Depth` / `Nothing` **语义**还没接进 `ForwardPass`（当前总是清色+清深度）。
- 相机朝向偏移（`FlyCameraController.yaw = 0` 看向 **+X**，单位四元数看向 **−Z**，
  差 `−π/2`）仍未用测试钉住。

### P3a-2 — 引擎接线 ✅

P3a-1 只加了字段；这一批让引擎真的用上它们。

- `scene/systems/camera.rs`：`perspective()` 委托 `Camera::projection_matrix`
  （透视/正交都走同一处，测试在 `prism-scene`）；`compute_camera_output`
  **不再强制要求 `FlyCameraController`** —— 缺失时直接用 `LocalToWorld` 的逆作为 view，
  也就是 Unity 里"相机由 Transform 驱动"的形状，场景只放 `Camera` + 变换即可出画面；
  新增 `all_cameras()`（按 `depth` 升序 = `Camera.allCameras`）、
  `main_camera()`（`depth` 最大 = `Camera.main`）、`clear_color()` + `DEFAULT_CLEAR_COLOR`。
- **取景与清屏同源（#11）**：新增 `active_camera()`＝`depth` 最大的**可渲染**启用相机
  （缺 `LocalToWorld` 才往低调）。`compute_camera_output()`、清屏色、aspect 写回、
  飞行输入都从它取实体，`clear_color(world, entity)` 直接接收 `CameraOutput::entity` ——
  否则会出现「A 的视图 + B 的背景色 / aspect 写到 B / 输入移动 B」。
- `render_system.rs`：`SceneChanges` 新增 `clear_color`，由 `collect_scene_changes`
  从活动相机取；删掉写死的 `const CLEAR_COLOR` —— 清屏色现在真的跟着
  `Camera.background_color` 走（复用已有的 `FrameInput.clear_color` 通路，零着色器改动）。

**行为变化**：以前没有 `FlyCameraController` 的相机实体会落到 `fallback_camera_output`
的固定灰底；现在会按实体朝向正常取景。这是 Unity 语义，但属于可见变化。

**验证**
- ✅ `cargo test -p prism-engine` → 87 passed（+1 ignored）。
- ✅ `python3 scripts/pr_ready.py` 全绿。

### P3a-3 — 枚举下拉框（属性面板）✅

`FieldSchema` 之前只给了 `u8` 这个 kind，枚举字段在检查器里只能看到 1/2/3/4 的数字框。
这一批把「变体名」从反射层一路带到前端（Unity 里 `EnumPopup` 的位置）。

- `prism-reflect`：`ReflectType` 新增带默认实现的
  `enum_variants() -> Option<&'static [&'static str]>`；`FieldDesc` 新增同名字段，
  `#[derive(Reflect)]` 为每个字段生成 `<FieldTy as ReflectType>::enum_variants()`。
  该字段**不参与 `schema_hash`**：纯展示元数据，改标签不该让已 cook 的 `.rscn` 失效。
- `prism-scene`：`CameraClearFlags` 暴露 `["Skybox","SolidColor","Depth","Nothing"]`，
  `TextAlign` 暴露 `["Center","Left","Right"]`（后者证明不是给 Camera 特判）。
- `prism-editor-api`：`FieldSchema` 新增 `variants: Option<Vec<String>>`。
- 前端：`FieldEditor.vue` 在 `variants` 非空时渲染 `<select>`，写回的仍是作者 JSON 里的
  字符串 —— 与 `editor_set_component_field` 的「先用 serde 还原组件再反射写入」路径一致，
  所以不需要额外的转换层。

**验证**
- ✅ `cargo test -p prism-editor-api -p prism-scene -p prism-reflect`
  → 74 / 31 / 13，其中 `schema_exposes_enum_variants_for_dropdowns` 同时断言
  `Camera.clear_flags`、`Text.alignment` 和「普通数值字段没有 variants」。
- ✅ `pnpm fixture` 后 fixture 里两处枚举都带 `variants`；`pnpm build` 通过。

### P3b-1a — 渲染枚举（`LightType` / `LightShadows` / `LightRenderMode`）✅

P3b（光照）的第一块：先把 Unity 的光照枚举做成**纯数据镜像**，这样 `Light` 组件、
引擎收集器和编辑器下拉框都有一份共同定义。

**prism-scene（`render_enums.rs`，纯加法）**
- `LightType`：取值与 `UnityEngine.LightType` 一致 ——
  `Spot=0` / `Directional=1` / `Point=2` / `Rectangle=3` / `Disc=4` /
  `Pyramid=5` / `Box=6` / `Tube=7`；`Default` 是 `Point`（Unity `new Light()`）。
  附 `unity_value()` / `from_unity_value()` / `is_rendered()`
  （面光源、盒形、管状只是 schema 对照，渲染器会跳过）。
- `LightShadows`：`None=0` / `Hard=1` / `Soft=2`，附 `casts_shadows()`。
- `LightRenderMode`：`Auto=0` / `ForcePixel=1` / `ForceVertex=2`。
- 三者都实现 `ReflectType`（`FieldType::U8` + `enum_variants()`），与
  `CameraClearFlags` 同一套编码 —— 检查器**自动**渲染成下拉框，RSCN / `schema_hash`
  不受影响，也不需要额外的类型表条目。
- `correlated_color_temperature_to_rgb(kelvin)`：镜像 Unity
  `Mathf.CorrelatedColorTemperatureToRGB`（Unity 是 native 实现，这里用常用的
  黑体拟合），裁到 `[1000, 40000]` K、sRGB→线性、**不归一化**；非有限输入退化为
  6500 K。
- 测试 7 个：Unity 数值/往返/未知值、`is_rendered` 边界、下拉变体列表、
  u8 反射往返、色温冷暖方向与 6500 K 近似白、裁剪与 NaN 回退、shadows/renderMode 数值。

**验证**：`cargo test -p prism-scene` → 43 passed；`cargo check --workspace --all-targets`；
`python3 scripts/pr_ready.py` 全绿。

**这一批刻意没做**：`Light` 组件本身（下一批 `20261025-light-unity-shape`）、
引擎接线、`ShadowCastingMode`、`BlendMode`/`CullMode`/`CompareFunction`/`RenderQueue`。

### P3b-1b — `Light` 组件（Unity 形状）✅

承接 P3b-1a 的枚举，这一批补上 Unity 形状的**组件本身**（纯加法）。

Unity 只有一个 `Light`，用 `Light.type` 区分方向光 / 点光 / 聚光，方向由
`transform.forward` 决定；PrismaRev 之前是三个独立组件，且方向光把欧拉角存在组件里。

**prism-scene（`components.rs`）**
- 新增 `Light`（`#[reflect(name = "prism_engine::scene::Light")]`）：
  `enabled`、`light_type`、`color`、`intensity`、`range`、`spot_angle`、
  `inner_spot_angle`、`shadows`、`shadow_strength`、`color_temperature`、
  `use_color_temperature`、`render_mode`，全部带默认值（`serde(default)`）。
- 单位刻意与旧组件一致（方向光 lux、点光/聚光 candela），**不是** Unity 的归一化
  `intensity = 1`：渲染器内部按 `LUX_TO_RADIANCE_SCALE` 折算，观感与旧组件一致。
  这是有意的偏差，写在组件文档里。
- `effective_color()` = `color × 色温`（仅 `use_color_temperature` 为真时），
  对应 Unity 的 `ExtractColorTemperature` + `ApplyColorTemperature`。
- `cone_half_angles_radians()`：Unity 的 `spotAngle` / `innerSpotAngle` 都是
  **全锥角（度）**，这里转成半角弧度，并把 `inner > outer` 的非法配置裁掉。
- `is_shadow_caster()` = `enabled && shadows != None`。
- `registry.rs` 注册 `Light`：schema / 默认值 / `.scene.json` / RSCN 共用同一条路径，
  因此检查器里能看到全部字段，`light_type`、`shadows`、`render_mode` 都是下拉框。
- 测试 5 个：默认值（Point / None / 30° / 21.8° / 6570 K）、JSON 往返 +
  缺字段回退、色温滤色只按开关生效、锥角换算与裁剪、阴影判定。

**旧组件不删、不迁移**：`DirectionalLight` / `PointLight` / `SpotLight` 继续可用，
`SCENE_DOC_VERSION` 仍为 2；引擎接线（两者都收）在下一批。

**这一批刻意没做**：引擎还没消费 `Light`（`collect_lights` 仍只查旧组件）；
`MeshRenderer.shadowCastingMode` 与 `BlendMode`/`CullMode`/`CompareFunction`/
`RenderQueue` 仍未开始。

### P3b-2 — 引擎接线（统一收集 + 聚光进 GPU）✅

`Light` 组件在上一批只是数据；这一批让引擎真正消费它，并把**聚光**接进前向光列表
（此前 `collect_spot_lights` 存在但从未被渲染器使用）。

**`prism-engine/src/scene/systems/lights.rs`（重写）**
- 新增 `LightSample` / `LightKind`（`Directional` / `Point` / `Spot`）：
  位置、朝向、线性颜色、强度、range、内外锥余弦、ambient、阴影字段都已解析。
- `collect_light_samples(world)`：
  - 先收 Unity 形状的 `Light`（跳过 `enabled = false`、`Active(false)` 和
    `light_type.is_rendered() == false` 的面光源/盒形/管状/金字塔）；
  - 再收旧三组件，**跳过已挂 `Light` 的实体**（避免同一实体点亮两次）；
  - 位置/朝向优先 `LocalToWorld`，缺省回退 `LocalTransform`；朝向 = 旋转 × `+Z`
    （Unity `transform.forward`，与相机看 `−Z` 不冲突：那是相机约定）；
  - 局部的点光+聚光只保留前 `LIGHT_MAX`（8）个，方向光不占额。
- `primary_directional_light(world)`：统一取方向光（`Light` 或旧组件）。
  旧的 `collect_directional_light` / `collect_point_lights` / `collect_spot_lights`
  保留（`pub`，老调用方与测试还在用）。

**`prism-engine/src/render_system.rs`**
- 方向光改用 `LightSample`：强度、颜色、ambient 分别进 frame UBO；
  旧的 `euler_xyz_deg_to_dir` 仍用于旧 `DirectionalLight`（在 `lights.rs` 内部）。
- 点光 + 聚光统一填 `GpuLight`：`const is_spot` 写进 `.color.w` 的类型位，
  聚光带朝向 + 内外锥余弦。
- 路径追踪的 `pt_lights` 改从 `LightSample` 取点光（PT 着色器仍无聚光分支，
  所以聚光只进前向路径）。

**`prism-render`（GPU 布局 32 → 64 字节）**
- `GpuLight` 增加 `direction`（xyz = 光源朝向，w = `cos(outer)`）与
  `params`（x = `cos(inner)`）；`.color.w` 表示类型（`GPU_LIGHT_KIND_POINT` /
  `GPU_LIGHT_KIND_SPOT`）。
- `GPU_LIGHT_SIZE` 常量取代 `forward_pass.rs` 里两处写死的 `32`（SSBO 分配与
  `update_lights` 的映射长度），`descriptor_tests` 同步断言 64 字节与偏移。
- `scene_frag.slang`：`GpuLight` 结构体补齐；点光循环里按类型乘聚光衰减
  `saturate((cos_angle − cos_outer) / (cos_inner − cos_outer))²`
  （`cos_angle = dot(-l, forward)`），锥外直接 `continue` 跳过 BRDF。

**行为变化**：聚光现在真的会照出锥形光斑（此前完全被忽略）；`Light` 实体
（点光/方向光/聚光）现在会被渲染。旧组件与旧场景不受影响。

**验证**
- ✅ `cargo test -p prism-engine -p prism-render` → 94 / 81 passed
  （新增 7 个光源采样测试：位置、朝向、锥角余弦、同实体去重、
  禁用/不支持类型跳过、旧聚光角换算、方向光不占局部预算）。
- ✅ `bash assets/shaders/compile.sh`（aarch64 slangc 2026.13.1）编译 scene_frag。
- ✅ `cargo check --workspace --all-targets`、`python3 scripts/pr_ready.py`。

**已知遗留**
- `Light.shadows` / `shadowStrength` 只是数据：阴影仍是全局方向光贴图
  （逐渲染器的 `ShadowCastingMode` 已在 P3b-3 补齐）。
- `GpuLight` 从 32 → 64 字节，**必须**重新编译着色器（`.spv` 是 gitignore 的，
  CI 会编）；旧 `.spv` 配新 Rust 结构体会读出垃圾数据。

### P3b-3 — 逐渲染器 `ShadowCastingMode` ✅

Unity 的 `Renderer.shadowCastingMode` 决定一个物体「是否投影」与「主通道是否可见」。
之前 PrismaRev 没有任何逐物体的阴影开关：所有 `DrawItem` 都既进主通道又进阴影贴图。

**prism-scene**
- `render_enums.rs` 新增 `ShadowCastingMode`（Unity 取值）：
  `Off=0` / `On=1` / `TwoSided=2` / `ShadowsOnly=3`；`Default` 是 `On`。
  `casts_shadows()`（除 `Off` 外都写阴影贴图）与 `visible_in_main_pass()`
  （`ShadowsOnly` 为 `false`）；反射仍是 `U8` + 变体名（检查器自动下拉框）。
- `MeshRenderer` 新增 `shadow_casting_mode`（`#[serde(default)]` → 老文档无需迁移）。
- `lib.rs` 重导出。

**prism-render**
- `DrawItem` 新增 `cast_shadow` / `visible_in_main_pass`（由引擎按组件填好，pass 只读）。
- `ShadowMapPass` 跳过 `!cast_shadow` 的绘制；`ForwardPass` 跳过
  `!visible_in_main_pass`（`ShadowsOnly`）。两个 pass 的 trace 现在分别统计
  **实际提交**的数量与绘制列表长度。

**prism-engine**
- `SceneReadView::draw_items()` 与 `scene_render_system()` 都从
  `MeshRenderer.shadow_casting_mode` 计算两个标志；没有 `MeshRenderer` 的运行时实体
  按默认 `On` 处理。
- 旧场景加载路径补 `shadow_casting_mode: Default::default()`。

**行为变化**：`ShadowsOnly` 物体不再出现在相机画面里、但会投下阴影；`Off` 物体不再
写阴影贴图。旧场景（默认 `On`）完全不变。

**验证**
- ✅ `cargo test -p prism-scene -p prism-engine -p prism-render` → 44 / 98 / 81 passed
  （新增：枚举数值/谓词/反射往返 1 个；默认、`Off`、`ShadowsOnly`、`TwoSided` 4 个）。
- ✅ `pnpm -C editor fixture`（`MeshRenderer.defaultJson` 多了 `shadow_casting_mode`）、
  `python3 scripts/pr_ready.py` 全绿。

**已知遗留**
- `TwoSided` 当时与 `On` 行为相同（P3b-6 已让阴影通道按它关闭背面剔除）。
- `Light.shadows` / `shadowStrength` 当时仍是数据（P3b-5 已接线）。

### P3b-4 — 编辑器光源预设切到 `Light` ✅

`Light` 组件虽然已经能被引擎渲染，但编辑器的「新建实体」下拉仍然只认识旧的
`DirectionalLight` / `PointLight`，等于新组件只能从检查器手动添加。根因是
`EntityPreset` 只带组件名，拿到的永远是 `Light` 的默认值（点光源）。

**prism-editor-api（`presets.rs`）**
- `EntityPreset` 新增两个字段：
  - `overrides: BTreeMap<String, Value>` —— 组件规范名 → **部分**作者 JSON，
    前端与 schema 的 `defaultJson` 浅合并；
  - `rotation: [f32; 4]` —— 初始四元数。光源朝向由 `transform.forward`（+Z）决定，
    没有旋转就没法表达方向光/聚光。
- 预设调整：
  - `sun`（平行光）→ `Light`，覆盖 `light_type = Directional`、`intensity = 100000`、
    `shadows = Soft`；旋转 = `from_rotation_arc(+Z, (−0.5, √½, 0.5))`，与旧
    `DirectionalLight.euler_xyz = (45, −45, 0)` 的光照方向一致。
  - `point_light` → `Light`，不覆盖（默认就是 `Point`）。
  - 新增 `spot_light`（聚光灯）→ `Light`，覆盖 `Spot` / 45° 外锥 / 30° 内锥 /
    `range 20` / `shadows Soft`；旋转绕 X +90°，让 +Z 朝下。

**editor（Vue）**
- `addEntity()` 把 `overrides[组件]` 浅合并进 `defaultJson`，并把 `preset.rotation`
  写进 `transform.rotation`（之前写死单位四元数）。
- `types.ts` 的 `EntityPreset` 补 `overrides` / `rotation`。

**验证**
- ✅ `cargo test -p prism-editor-api presets` → 4 passed（新增 3 个：四元数单位长度、
  三个光照预设都挂 `Light` 且类型正确、`overrides` 只覆盖已挂载组件的已存在字段）。
- ✅ `pnpm -C editor fixture`（`presets` 里带上 `overrides` / `rotation`）、
  `pnpm -C editor build`、`python3 scripts/pr_ready.py` 全绿。

**这一批没做**：`lit` 场景模板仍用旧组件（模板是另一条 `merge` 路径，留给后续）；
渲染枚举批次（`BlendMode`/`CullMode`/`CompareFunction`/`RenderQueue`）未开始。

### 清理 — `prism-cooker` 里重复的场景 schema ✅

`crates/prism-cooker/src/importer/scene.rs` 里还有一份 `SceneJson` / `TransformJson`
（连同 216 行测试），与 `prism_scene::json` 的权威定义重复，且没有任何调用方
（cooker 的实际路径走 `cooker/scene.rs` → `prism_scene::json::SceneJson`）。

删除这两个文件（356 行）并去掉 `importer/mod.rs` 的 `pub mod scene;`。cooker 现在只有
一份场景 schema，改字段不会再出现「改了一处、另一处还是旧的」。

验证：`cargo test -p prism-cooker`（95 + 6 + 3）、`cargo check -p prism-cooker --all-targets`、
`python3 scripts/pr_ready.py --quick`。

### P3b-5 — `Light.shadows` / `shadowStrength` 接线 ✅

`Light` 的阴影字段之前只是数据（PR #14 的「已知遗留」）。这一批让它们真正生效：

**prism-engine**
- `lights.rs` 新增 `directional_shadow_state(&[LightSample]) -> (bool, f32)`：
  `enabled = Light.shadows != None`、`strength = shadow_strength.clamp(0, 1)`；
  没有方向光时 `(false, 0.0)`。
- `SceneChanges` 新增 `shadows_enabled` / `shadow_strength`，
  `collect_scene_changes` 从方向光采样一次算好。
- `render_system` 把 `shadow_strength` 写进 `FrameUBOData`，把 `shadows_enabled`
  透传给 `FrameInput`。

**prism-render**
- `FrameInput` 新增 `shadows_enabled`；`GraphFrame.shadow_mode` 在
  `shadows_enabled == false` 时强制 `ShadowMode::None`（整帧跳过阴影贴图 pass，
  而不只是不采样）。
- `FrameUBOData` 用原来的 padding 槽位新增 `shadow_strength`（offset 256），
  结构体仍是 272 字节；`common.slang` 同步。
- `scene_frag.slang` 的 `sample_shadow` 末尾改成
  `lerp(1.0, sum / 9.0, saturate(frame.shadowStrength))` —— `shadows = None`
  （strength 0）时直接全亮，不会残留上一帧的脏阴影。

**prism-app**
- `render_runner` 的清屏色改用 `scene.clear_color`（顺手去掉写死的 `CLEAR_COLOR`），
  并传入新的 `shadow_strength` / `shadows_enabled`。

**行为变化**：`Light.shadows = None` 现在真的没有阴影（并省掉一次 shadow pass）；
`shadow_strength = 0.5` 是半强度阴影。旧 `DirectionalLight` 采样固定为
`Hard` / `1.0`，老场景不变；**没有方向光的场景**不再浪费一次阴影 pass。

**验证**
- ✅ `cargo test -p prism-engine -p prism-render` → 102 / 81 passed
  （新增 4 个：Soft/0.6、None/0.4、无方向光、strength clamp）。
- ✅ `bash assets/shaders/compile.sh`（slangc 2026.13.1，scene_frag）。
- ✅ `cargo check --workspace --all-targets`、`python3 scripts/pr_ready.py`。

**已知遗留**
- `TwoSided` 当时仍等同 `On`（P3b-6 已接线）。
- `Light.shadows` 只影响**方向光**：点光/聚光本来就不投影。

### P3b-6 — `ShadowCastingMode::TwoSided` 关闭阴影背面剔除 ✅

Unity `ShadowCastingMode.TwoSided` 的语义是：**阴影渲染时关闭背面剔除**
（即使材质本身开了背面剔除），因此单面几何体也会从两面投射阴影。
P3b-3 只把该变体接进 `cast_shadow` / `visible_in_main_pass`，但光栅阴影管道当时对
所有 caster 都用 `CullModeFlags::NONE`，所以 `TwoSided` 与 `On` 没有区别（也与 Unity
「`On` 单面」的语义不符）。

**prism-scene**
- `ShadowCastingMode::casts_two_sided_shadows()`：只有 `TwoSided` 为真；
  修正 `TwoSided` 的文档注释（原文案「与 On 相同」已过时）。

**prism-render**
- `DrawItem` 新增 `cast_shadow_two_sided`（引擎按组件填好，pass 只读）。
- `ShadowMapPass` 改为**两套 depth-only 管线**：背面剔除（`CullModeFlags::BACK`，
  `Off`/`On`/`ShadowsOnly`）与双面不剔除（`NONE`，`TwoSided`）；绘制时按
  `cast_shadow_two_sided` 选择、仅在模式切换时重绑；推送常量用当前绑定管线的布局。
  管线创建抽成 `create_pipeline(device, render_pass, cull_mode)`（`warmup`/`execute`
  共用），并新增 `shadow_cull_mode(two_sided)` 单测。
- 默认阴影管线从 `NONE` 改为 `BACK`：这是 Unity `On` 的语义，也与主通道默认剔除一致。

**prism-engine**
- `scene_render_system` / `SceneReadView::draw_items` 把
  `mode.casts_two_sided_shadows()` 写进 `cast_shadow_two_sided`。

**行为变化**：`ShadowCastingMode::On` 的阴影贴图现在做背面剔除（与主通道一致的
单面投影），`TwoSided` 才双面投射；两者不再等价。旧场景默认 `On`，因此此前依赖
「阴影也渲染背面」的薄面/开口几何体，阴影可能变薄——这是 Unity 的预期语义。

**验证**
- ✅ `cargo test -p prism-scene -p prism-render -p prism-engine` → 61 / 82+3 / 113 passed；
  新增/更新断言：`TwoSided` 关闭剔除，`Off`/`On`/`ShadowsOnly` 不关闭。
- ✅ `cargo check -p prism-scene -p prism-render -p prism-engine --all-targets`。

**已知遗留**
- 逐 caster 的阴影管线切换是状态式的（按绘制顺序），乱序时会有额外重绑，
  但正确性不受影响。
- 点光/聚光仍只有方向光的阴影贴图（与 `TwoSided` 无关）。

### P4a — `PropertyDrawer` schema 扩展点 ✅

Unity 的 `[Range]` / `[TextArea]` / `[ColorUsage]` 等**特性**由对应的 `PropertyDrawer`
渲染成滑动条 / 多行文本框 / 取色盘。这一批把它落地成 **声明式 DTO**：后端 schema 说明
「这个字段用哪个渲染器 + 哪些特性」，前端 Vue 负责真正的 UI（§3.2 的约定）。

**prism-editor-api**
- 新模块 `property_drawer`：
  - `PropertyRenderer` = `default` / `slider` / `textArea` / `color` / `custom`；
  - `PropertyAttribute` = `range` / `min` / `max` / `multiline` / `textArea` /
    `colorUsage` / `header` / `tooltip` / `delayed`（serde 标签对象，前端按 `type` 分发）；
  - `PropertyDrawer` = `renderer` + `attributes` + `custom`（自定义组件名），
    带 `range` / `min` / `max` / `text_area` / `multiline` / `color_usage` / `custom`
    构造器与 `range_bounds` / `line_range` / `tooltip` 等访问器；
  - `PropertyDrawerRegistry`：字段级声明**优先于**类型级（`[CustomPropertyDrawer(typeof(X))]`），
    后注册覆盖先注册。
- `FieldSchema` 新增 `drawer: Option<PropertyDrawer>`（`None` 不序列化，fixture 不膨胀）；
  `build_schema()` 应用 `PropertyDrawerRegistry::with_builtin()`；新增
  `build_schema_with(&ComponentRegistry, &PropertyDrawerRegistry)` 让宿主在内置声明之上追加。
- 内置声明（只覆盖有明确数值/展示语义的字段，不替所有字段猜控件）：
  `Camera.fov_y_degrees` / `exposure`（Range）、`background_color`（ColorUsage）、
  `near` / `far` / `orthographic_size`（Min）；`Light.intensity` / `range` /
  `color_temperature`（Min）、`spot_angle` / `inner_spot_angle` / `shadow_strength`（Range）；
  `Skybox.rotation_degrees`（Range）、`intensity`（Min）；`Text.content`（TextArea）。

**editor（Vue）**
- `types.ts`：新增 `PropertyRenderer` / `PropertyAttribute` / `PropertyDrawer`，
  `FieldSchema.drawer?`。
- `FieldEditor.vue`：按 `renderer` 渲染滑动条（Range = 滑动条 + 可编辑数值框）、
  多行文本框（TextArea / Multiline）、取色盘（Color 强制）；`[Tooltip]` → 标签 `title`、
  `[Header]` → 分组标题、`[Delayed]` → 只在 `change`（回车/失焦）提交；未知 `custom`
  渲染器退回默认控件并在标签后标注组件名。
- `ColorField.vue`：新增 `showAlpha` prop，`[ColorUsage(showAlpha: false)]` 时隐藏 alpha
  滑块与第 4 个通道。
- 重新生成 `editor/public/demo.json` 的 `schema` 段（与 `pnpm fixture` 同源，避免 fixture 漂移）。

**验证**
- ✅ `cargo test -p prism-editor-api` → **89 passed**（新增 9 个：range/非法 range/行数范围/
  重复特性/delayed/查找优先级/后注册覆盖/内置声明/JSON 形状）。
- ✅ `cargo clippy -p prism-editor-api --all-targets -- -D warnings`
- ✅ `cd editor && pnpm build`（`vue-tsc --noEmit` + Vite）
- ✅ `python3 scripts/pr_ready.py`

**已知遗留**
- `renderer == "custom"` 还没有前端插件注册机制：组件名只做标注，控件仍退回默认。
- 绘制器声明集中在 `prism-editor-api` 的内置表；`prism-scene` 的派生宏还没有
  `#[property(...)]` 语法糖——先让 schema 层集中声明，避免反射层依赖编辑器语义。

### P3b-6b — 渲染状态枚举（`CullMode` / `CompareFunction` / `BlendMode` / `RenderQueue`）✅

P3b-1a 建了光照枚举，这一批补上**渲染状态**的纯数据镜像（`prism-scene::render_enums`），
让材质/渲染器层面的 Unity 字段有一份共同定义，检查器与 `prism-render` 都能复用。
和 P3b-1a 一样：**只加数据，不改管线行为**。

- `CullMode`（`UnityEngine.Rendering.CullMode`）：`Off=0` / `Front=1` / `Back=2`，
  默认 `Back`（与现有图形管线的 `vk::CullModeFlags::BACK` 一致），谓词 `culls()`。
- `CompareFunction`：`Disabled=0` … `Always=8`，默认 `LessEqual`（Unity 着色器 `ZTest`
  的默认值），谓词 `is_enabled()`。[`Disabled`](Self::Disabled) 对应 Unity 的“关闭深度测试”，
  Vulkan 没有对应的 `vk::CompareOp`，由 `prism-render` 用深度测试开关（`depth_test_enable(false)`）表达。
- `BlendMode`：11 个 Unity 因子值（`Zero=0` … `OneMinusSrcAlpha=10`），默认 `One`
  （与 `Zero` 组成不透明混合）。只描述**因子**；Unity 的 `BlendOp`
  （Add/Sub/RevSub/Min/Max）是另一份数据，刻意留到下一批，避免半套状态。
- `RenderQueue`：**形状不同** —— Unity 里它是包裹 `int` 的 struct，不是枚举，
  具名队列只是 `static readonly int`。因此这里用 `RenderQueue(i32)` newtype +
  关联常量（`BACKGROUND=1000` / `GEOMETRY=2000` / `ALPHA_TEST=2450` /
  `GEOMETRY_LAST=2500` / `TRANSPARENT=3000` / `OVERLAY=4000`）、`value()`、
  `is_opaque()` / `is_transparent()`（不透明区间是 `[0, GEOMETRY_LAST]`，与 Unity
  一致；负数能无损保存但不算不透明）；默认 `GEOMETRY`。
  反射按 `FieldType::I32` 编码，任意值（含负数）都能无损往返。
- 三个 C 风格枚举与 `CameraClearFlags` 走同一套编码（`FieldType::U8` +
  `enum_variants()`），所以未来挂到组件字段上时检查器**自动**渲染成下拉框，
  且不参与 `schema_hash`；`RenderQueue` 是数值框。

**验证**
- ✅ `cargo test -p prism-scene` → 65 passed（新增 4 个：`CullMode` 数值/默认/变体、
  `CompareFunction` 全表 + 默认 `LessEqual`、`BlendMode` 全表 + 默认 `One`、
  `RenderQueue` 具名值/区间/反射负数往返）。
- ✅ `python scripts/pr_ready.py`（见 PR）。

**这一批刻意没做**：`BlendOp` / `RenderQueueRange`；把枚举挂到 `MeshRenderer`
（`cull_mode` / `render_queue`）或新建材质组件；`prism-render` 的管线状态翻译
（`vk::CullModeFlags` / `vk::CompareOp` / `vk::BlendFactor`）。都留给后续批次。

### P3b-7 — 逐渲染器 `CullMode` 接线 ✅

P3b-6 只加了枚举；这一批让 `CullMode` 真正变成 Vulkan 管线状态。Unity 把它放在
`Material`（`_Cull`）上，但 PrismaRev 的材质目前只有 PBR 参数、没有逐材质的管线变体，
所以先挂在 `MeshRenderer` 上——语义仍是「这一个绘制用哪一面」，等材质系统引入后再搬。

**prism-scene**
- `MeshRenderer.cull_mode: CullMode`（`#[serde(default)]` → 老文档不需要迁移，
  `SCENE_DOC_VERSION` 保持 3）。作者 JSON 取值是 `"Off"` / `"Front"` / `"Back"`。

**prism-render**
- `DrawItem.cull_mode: prism_gfx::CullMode`（共享枚举，见下方「抽出 `prism-gfx`」）。
  把枚举翻译成 Vulkan 位的是 `pipeline::cull_flags()`，保留在渲染器一侧。
- `ForwardPass` 的 `pipeline: Option<GraphicsPipeline>` 换成
  `pipelines: HashMap<CullMode, GraphicsPipeline>`：三种剔除模式在启动时各建一条
  （`ensure_pipelines`），因为剔除是**管线状态**而不是动态状态。三条变体的描述符集合
  布局与推送常量完全相同，所以描述符集 / 推送常量只在循环外绑一次，循环里只在
  `item.cull_mode` 变化时 `cmd_bind_pipeline`。
- 阴影贴图管线**不变**（固定 `CullModeFlags::NONE` 双面写深度），与 `ShadowCastingMode::TwoSided`
  的既有语义一致。

**prism-engine**
- `scene_render_system` 与 `SceneReadView::draw_items()` 都从 `MeshRenderer.cull_mode`
  取状态（缺组件按 `Back`），**直接**放进绘制项——枚举现在由 `prism-gfx` 共享，
  不再需要映射函数。
- `loader.rs` 的 bundle 插入补 `cull_mode: Default::default()`。

**行为变化**：`cull_mode = "Off"` 的物体现在真的双面可见（此前一律背面剔除）；
默认 `Back` 的旧场景完全不变。

**验证**
- ✅ `cargo test -p prism-scene -p prism-engine -p prism-render` → 66 / 108(+3) / 82 passed
  （新增：`MeshRenderer` 缺字段回退 + JSON 往返；`DrawCullMode::to_vk` 映射与唯一性；
  绘制项默认 `Back` + `CullMode::Off` 透传 + 映射函数）。
- ✅ `pnpm -C editor fixture` 后手动只保留 schema 相关差异（`defaultJson` / `fields` /
  模板场景各一处 `cull_mode`）——本机磁盘资产与提交版 fixture 本来就有一批无关漂移，
  没有一并写入。
- ✅ `python scripts/pr_ready.py`（见 PR）。

**这一批刻意没做**：混合状态（`BlendMode` / `BlendOp`）与 `RenderQueue` 的绘制排序、
透明度（`ZWrite` / 距离排序）——它们需要材质级管线变体 + 排序，单独一批；
`BlendOp` 枚举也留到那时一起加。点光/聚光阴影仍未实现。

### 重构 — 抽出 `prism-gfx`（共享渲染状态）✅

`CullMode` 要同时被场景 schema（`prism-scene`）和 Vulkan 管线（`prism-render`）使用，
而 `prism-render` 不依赖 `prism-scene`（也不该依赖：场景 schema 是上层概念）。上一批用
「渲染器侧镜像一份 `DrawCullMode` + 引擎映射函数」绕过去了，但每加一个共享枚举（`BlendOp`、
`RenderQueueRange`、以后的 `FilterMode`/`WrapMode`）都要重付一次，所以这次把数据抽出来。

- 新 crate `crates/prism-gfx`：**纯数据 + 反射编码**，依赖 `prism-reflect` / `serde` / `glam`，
  **不依赖 ash、不依赖 ECS**。内容就是原 `prism_scene::render_enums` 的全部枚举
  （光照 + 相机 + 渲染状态）与 `correlated_color_temperature_to_rgb`，连同它们的单测。
- `prism-scene`：`render_enums.rs` 变成 `pub use prism_gfx::*;` ——
  `prism_scene::render_enums::*`、`prism_scene::CullMode` 等**历史路径全部不变**，
  cooker / engine / editor-api 零改动。
- `prism-render`：依赖 `prism-gfx`，`DrawItem.cull_mode` 直接用共享 `CullMode`；
  删掉 `DrawCullMode` 与引擎里的 `draw_cull_mode()` 映射；Vulkan 翻译收敛到
  `pipeline::cull_flags()`（`prism-gfx` 保持 Vulkan-free）。
- 分层文档同步：`AGENTS.md` 的 crate 清单、`docs/DESIGN.md` §10.2 的表、
  CI 的 `cargo clippy/test -p ...` 列表都补上 `prism-gfx`。

**验证**：`cargo test -p prism-gfx -p prism-scene -p prism-engine -p prism-render`
→ 18 / 48 / 108(+3) / 82 passed（枚举单测随类型一起搬到 `prism-gfx`，scene 只剩 schema 测试）；
`python scripts/pr_ready.py` 全绿。

**顺带修正一条过时结论**：§9 P3b-3 的「`TwoSided` 与 `On` 相同，因为阴影管线只渲染
背面剔除后的三角形」描述有误 —— `ShadowMapPass` 的两条管线本来就用
`cull_mode: Some(vk::CullModeFlags::NONE)`（双面写深度），所以 `TwoSided` 的投影语义
**已经**被满足；两者视觉等价是既有行为，不是待修缺陷。

#### P3b-7b — 混合状态 + `RenderQueue` 排序（透明度）✅

上一批把剔除接进了管线，这一批补上 Unity 材质的另外三项状态：混合（`_SrcBlend` /
`_DstBlend` / `_BlendOp`）、`_ZWrite` 和决定绘制顺序的 `renderQueue`。

**prism-gfx**
- 新增 `BlendOp`（Add=0 / Subtract / ReverseSubtract / Min / Max，默认 Add），
  与 `BlendMode` 一样是 U8 枚举 + `enum_variants()`；`BlendMode` / `BlendOp` 加 `Hash`
  （管线缓存键需要）。

**prism-scene**
- `MeshRenderer` 增加 `render_queue: RenderQueue` / `z_write: bool` / `src_blend` /
  `dst_blend` / `blend_op`。`#[serde(default)]` + **手写 `Default`**，因为目标默认值
  （`One` / `Zero` / `Add` / `ZWrite=true` / 队列 2000）不是各字段类型自身的
  `Default::default()`（`BlendMode::default()` 是 `One`）。老文档零迁移。
- `MeshRenderer::blends()`：`One`/`Zero`/`Add` 即“不透明”（Unity 的约定），
  否则需要开混合。

**prism-render**
- `DrawItem.cull_mode` 升级为 `DrawItem.state: DrawState`（剔除 / 队列 / 深度写 /
  混合因子 / 混合运算）。`DrawState` 同时是 `ForwardPass` 的管线缓存键：
  `ensure_pipelines(device, &states)` 在**录制命令前**按本帧出现过的状态集建管线，
  循环里按需重绑；描述符集与推送常量布局在所有变体间相同，只绑一次。
- `pipeline::blend_factor()` / `blend_operation()` 做 Unity→Vulkan 映射
  （**两边数值顺序不同**，Unity 把 `DstColor` 排在 `SrcColor` 前，不能 `as` 转换）。
- 不透明绘制现在**真正关闭**颜色混合（此前管线恒定开着 `SRC_ALPHA`/`ONE_MINUS_SRC_ALPHA`
  —— 不透明片元 alpha=1，结果一致，但语义是错的）。
- `sort_draw_items(items, eye)`：队列升序；队列 > `GEOMETRY_LAST`（2500）的透明段
  按与相机距离**从远到近**（`sort_by` 稳定，同队列的不透明物体保持提交顺序）。
- 阴影贴图管线不变（双面、无混合、写深度）。

**prism-engine**
- `draw_state()` 把 `MeshRenderer` 的五个状态字段装进 `DrawState`（缺组件用默认值），
  `extract_frame_packet` 用 `scene.eye` 调用 `sort_draw_items` —— 排序必须在提交前完成，
  `ForwardPass` 只按列表顺序绘制。

**用法**：标准 alpha 混合 = `render_queue: 3000`、`z_write: false`、
`src_blend: "SrcAlpha"`、`dst_blend: "OneMinusSrcAlpha"`；加色 = `src_blend: "SrcAlpha"`、
`dst_blend: "One"`。

**验证**
- ✅ `cargo test -p prism-gfx -p prism-scene -p prism-render -p prism-engine` →
  19 / 48 / 86 / 108(+3) passed。新增：`BlendOp` 取值/反射；`MeshRenderer` 老文档默认值 +
  透明状态往返（`render_queue` 是 JSON 数字、混合因子是变体名）；`DrawState::blends`；
  `sort_draw_items`（不透明段按队列、透明段远→近、同队列稳定）；
  `blend_factor` / `blend_operation` 的 Unity→Vulkan 映射。

**这一批刻意没做**：`BlendOp` 之外的 Unity 状态（`_ColorMask`、`ZTest` / `CompareFunction`
接线、`Stencil`）、按材质资产的混合（目前仍在渲染器上，等材质系统带 `Material` 组件时再搬）、
`RenderQueueRange`。点光/聚光阴影仍未实现。

### P3c-1 — `CameraClearFlags` 的 Skybox 语义接进渲染 ✅

`Camera.clear_flags`（Unity `CameraClearFlags`）此前只是数据。这一批先把**单相机下唯一
可观察**的部分接上：**只有 `clearFlags == Skybox` 才绘制天空盒背景**，其余清屏方式即使
场景里有启用的 `Skybox` 实体也不画。

**prism-gfx / prism-scene**
- `prism-gfx` 新增 `CameraClearFlags::draws_skybox()`（仅 `Skybox` 为真），
  并在文档里写明与 `clears_color()` / `clears_depth()` 的分工；扩展
  `clear_predicates` 断言四个取值的天空盒行为。
- `prism-scene::render_enums` 维持 `pub use prism_gfx::*;`。

**prism-engine**
- `scene/systems/camera.rs` 新增 `clear_flags(world, entity)` 访问器：与
  `clear_color` 同源（都取 `CameraOutput::entity`），缺省 `Skybox`。
- `SceneChanges` 新增 `clear_flags`，`collect_scene_changes` 从活动相机取；
  `extract_frame_packet` 用 `draws_skybox()` 决定 `FramePacket.skybox` 是
  `Some(..)` 还是 `None` —— `ForwardPass` 对 `None` 直接跳过天空盒绘制，
  所以这就是它的开关，无需改 render pass。

**行为变化**：把相机的 `clearFlags` 从默认 `Skybox` 改成 `SolidColor` / `Depth` /
`Nothing` 后，天空盒背景不再绘制（改为相机 `background_color` 或保留已有颜色）。
默认值不变，老场景（`Skybox`）完全不变。

**验证**
- ✅ `cargo test -p prism-gfx -p prism-scene -p prism-engine` → passed。

**已知遗留（下一批）**
- `Depth` / `Nothing` 的**颜色/深度 load-op** 还没接：Vulkan 的 load op 写在 render pass
  里（静态），需要按 `(clears_color, clears_depth)` 组合缓存 render pass + framebuffer，
  或在 pass 外显式 `vkCmdClear*`；两者都属于多相机（叠加渲染）的前提，留到 P3c-2。
- 多相机（`Camera.allCameras` 按 `depth` 排序 + 叠加渲染）还没接。

### 待办（下一批候选）
- **P3b 光照/渲染（剩余）**：`CompareFunction`（`ZTest`）接线、`_ColorMask`、
  `RenderQueueRange`、把渲染状态搬进真正的 `Material` 组件（材质资产级）；
  点光/聚光还没有自己的阴影（只有方向光贴图）。
- **P3c 相机行为**：用测试钉住 yaw↔quaternion 的 `−π/2` 偏移；`clear_flags` 的
  `Depth`/`Nothing` 语义接进 `ForwardPass`；多相机（`depth` 排序 + 叠加）。
- **P7 剩余**：`UIElements`（编辑器是 Vue，507 类型）/ `UI`（已有 ECS 布局，
  UGUI 形状需先设计）/ `Audio`（运行时 `AudioSource` 已有，场景组件与
  `AudioListener` 需接线）/ `Animation` / `Physics`（暂无运行时后端，需先选型）。

### P7a — `UnityEngine.Input` 门面 ✅

**目标**：`UnityEngine.Input` 在差距矩阵里还是 ✗，但底层状态其实已经齐了——
[`InputManager`] 每帧收键盘/鼠标/触摸，只是方法名是 `key_held` 这类 Rust 风格。
这一批叠一层只读门面，让 Unity 代码可以按 `Input.GetKey` 的名字查询。

**新增 `crates/prism-engine/src/input_facade.rs`**
- `Input<'a>` 借用当帧的 `InputManager`（`Input::new(&manager)` / `From<&InputManager>`），
  零分配、`Copy`。
- 键盘：`get_key` / `get_key_down` / `get_key_up` / `any_key` / `any_key_down`。
- 鼠标：`get_mouse_button` / `get_mouse_button_down` / `get_mouse_button_up` /
  `mouse_position` / `mouse_scroll_delta`。
- 轴：`get_axis("Mouse X" | "Mouse Y" | "Mouse ScrollWheel")`，其它名字返回 `0.0`；
  `get_axis_raw` 与 `get_axis` 等价（没有平滑层）。
- 触摸：`touches` / `touch_count`（按事件，不按触点列表）。
- `lib.rs`：`pub mod input_facade;` + 根级 `pub use input_facade::Input;`。

**`InputManager` 补齐的状态**
- 新增 `mouse_just_released: Vec<MouseButton>`（`begin_frame` 清空，
  `handle_mouse_button(Released)` 填充）——之前只有 `keys_just_released`，
  没有鼠标键的 “up” 瞬态，`GetMouseButtonUp` 无从实现。
- 新增只读查询 `any_key_held` / `any_mouse_button_held` / `buttons_just_pressed` /
  `mouse_just_released`；不把内部的 `FxHashSet` 类型暴露到公开 API。

**与 Unity 的差异（刻意）**
- 没有 Input Manager 命名轴（`GetButton` / `GetAxis("Horizontal")`）：本项目没有
  `InputManager.asset`，键位直接查 `KeyCode`；`GetAxis` 只实现三条鼠标内置轴。
- 没有传感器与文本输入（`Input.location` / `compass` / `acceleration` /
  `inputString` / IME），已记入 §5.3。
- 触摸是**本帧事件列表**，不是 Unity 的 per-finger `Touch[]` 快照。

**验证**
- ✅ `cargo test -p prism-engine input_facade` → 6 passed。
- ✅ `cargo test -p prism-engine` → 119 passed / 1 ignored。
- ✅ `cargo clippy -p prism-engine --all-targets -- -D warnings`。
- ✅ `python3 scripts/pr_ready.py`（见 PR 内自检块）。

### P4b — `EditorWindow` / `[MenuItem]` 面板注册表 DTO ✅

Unity 的 `EditorWindow` 是「可停靠的编辑器窗口」，`[MenuItem("窗口/…")]` 是「菜单命令」。
这一批把两者投影成**后端声明 + 前端动作**：后端只说「有哪些窗口、标题、菜单路径、快捷键、
命令 id」，真正打开/关闭/停靠仍由前端 docking 负责（§3.2 的约定）。

**prism-editor-api**
- 新模块 `editor_window`：
  - `EditorWindowDescriptor { id, title, menu_path, shortcut, command }`（serde camelCase）；
    构造时 `command` 自动为 `window/toggle/<id>`，可用 `with_shortcut` 补快捷键；
    `menu_segments()` / `menu_label()` 负责拆 `窗口/分析/实时渲染图` 这类路径。
  - `EditorWindowRegistry`：`builtin()`（13 个窗口）/ `register()`（同 id 覆盖）/
    `get()` / `ids()` / `len()`；`build_window_registry()` 是命令层的入口。
  - 内置表与编辑器现有面板一一对应：`文档`（Ctrl+O）、`层级`、`资产`、`检查器`、`日志`、
    `渲染图`（`窗口/分析/实时渲染图`）、`项目设置`、`首选项`、`主题`、`构建项目`、
    `离线工具`、`快捷键`（`帮助/键盘快捷键`）、`关于`（`帮助/关于 PrismaRev Editor`）。
- `dump_fixture` 增加 `windows` 键，浏览器预览与 Tauri 用同一份声明。

**editor**
- `types.ts` / `api.ts` / `mock.ts`：`EditorWindowDescriptor` + `fetchEditorWindows()`
  （Tauri 命令 `editor_windows`；fixture 走 `windows` 键）。
- `App.vue`：`PANELS` 改为由注册表派生的 computed（`PANEL_IDS` 仍是前端类型的唯一来源）；
  `FALLBACK_WINDOWS` 是浏览器预览/旧后端的内置回退，启动时按 id 覆盖而非整体替换，
  少发一个窗口不会让标签页消失。
- `MENUS` 的「窗口」「帮助」两棵树由 `menuPath` 折叠生成
  （`addMenuPath` 复用同名子菜单），因此 `窗口 → 分析 → 实时渲染图` 与
  `帮助 → 键盘快捷键 / 关于…` 不再手写。`command` 通过前端 `PANEL_COMMANDS` 表执行，
  未知命令忽略（向前兼容）；`openGraph` 被删掉——显示面板时
  `watch(visiblePanels)` 已经负责刷新渲染图并开始轮询。
- 文件/编辑/构建菜单保持前端静态闭包（它们操作的是文档状态，不是窗口）。

**验证**
- ✅ `cargo test -p prism-editor-api`（新增 5 个：内置 13 项、按 id 查找/覆盖、
  菜单路径覆盖「分析」「帮助」、`menu_label` 回退、camelCase 序列化）
- ✅ `cargo clippy -p prism-editor-api --all-targets -- -D warnings`
- ✅ `cargo check --manifest-path editor/src-tauri/Cargo.toml`
- ✅ `cd editor && pnpm build`（`vue-tsc --noEmit` + Vite）
- ✅ `python3 scripts/pr_ready.py`

**已知遗留**
- 菜单动作仍是前端闭包：后端只给 `command` id，不执行命令（保持 editor-api 不链接 UI/状态）。
- 默认停靠布局仍写死在 `dock.ts` 的 `defaultDock()`；注册表还没有「默认 dock 位置」字段。
- 快捷键只在菜单里展示，尚未由 `shortcut` 字段自动注册到 `onKey`。

### P5a — `AssetImporter` 形状 + 资产浏览器显示导入器 ✅

Unity 里每个资产都由一个 `AssetImporter` 子类导入（`TextureImporter` / `ModelImporter` /
`AudioImporter` / `DefaultImporter`…），`AssetImporter.GetAtPath(path)` 拿到实例，
`importSettingsMissing` 表示还没有 `.meta` 里的导入设置。这一批把**只读的导入器描述**
投影出来，并显示在编辑器的资产详情里。

**prism-editor-api（新模块 `asset_importer`）**
- `ImporterClass` = `texture` / `model` / `audio` / `environment` / `scene` / `prefab` /
  `package` / `text` / `default`，`unity_class_name()` 映射到 Unity 类名：
  纹理与环境 → `TextureImporter`、模型 → `ModelImporter`、音频 → `AudioImporter`、
  文本 → `TextScriptImporter`、其余 → `DefaultImporter`。
- `ImporterClass::from_path()` 按扩展名判定（`.scene.json` / `.prefab.json` 后缀优先于
  最后的 `.json`）：`gltf/glb/obj/fbx/bin/mesh` → 模型；`png/jpg/ktx2/…` → 纹理；
  `hdr/exr` → 环境；`wav/mp3/ogg/flac/…` → 音频；`pak` → 包；`json/toml/ron/md/slang/…` → 文本。
- `AssetImporter { asset_path, importer, main_asset_type, user_data, import_settings_missing }`：
  `get_at_path(project_root, path)` 校验路径存在且是文件（目录 / 缺失分别报错），
  `main_asset_type` 复用 `assets::asset_kind`；`list_importers(project_root)` 批量解析。
- `AssetInfo` 新增 `importer`（导入器类名），由 `list_assets_info` 填充 —— 因此
  **不需要新的 Tauri 命令**：资产浏览器拿到的 `AssetInfo` 就带上了导入器。

**editor（Vue）**
- `types.ts` 的 `AssetInfo` 增加 `importer: string`。
- 资产浏览器详情面板显示 `类别 · 导入器 <类名>`（`title` 提示对应
  `AssetImporter.GetAtPath(...).GetType().Name`）。

**与 Unity 的差异（有意为之）**
- 引擎没有 `.meta` 文件、也不在运行时保存导入参数：`user_data` 恒为空、
  `import_settings_missing` 恒为 `true`。真正的导入参数在 cooker 的 `CookProfile` 里
  （平台相关的压缩 / 采样设置），不塞进编辑器的资产 DTO。
- 没有 `AssetImporter.SaveAndReimport()`：资产导入是 cooker/build 的职责，
  `prism-editor-api` 不链接 cooker。

**验证**
- ✅ `cargo test -p prism-editor-api` → **84 passed**（新增 6 个：扩展名 → 类名、
  `.scene.json`/`.prefab.json` 后缀优先、`get_at_path` 主资产类型、目录/缺失报错、
  批量覆盖每个资产、camelCase 序列化）。
- ✅ `cargo clippy -p prism-editor-api --all-targets -- -D warnings`
- ✅ `cd editor && pnpm build`
- ✅ `python3 scripts/pr_ready.py`

**已知遗留**
- 还没有逐资产的导入设置 UI（`TextureImporter` 的 sRGB / filter、`ModelImporter` 的
  缩放 / 切线等）；这些目前由 cooker 的 `CookProfile` 统一决定。
- `Build` / `Build.Profile` / `Compilation` 形状仍待后续批次。

### P5b — `Build` / `Build.Profile` 形状 ✅

Unity 的构建入口是 `BuildPipeline.BuildPlayer(BuildPlayerOptions)`，其中
`BuildPlayerOptions` 携带目标平台、场景列表（顺序即 build index）与 `BuildOptions`
（`Development` / `AllowDebugging` / `ConnectWithProfiler`）。这一批把这些概念投影成
`prism-editor-api` 的纯数据 DTO，并把「场景列表为空」变成真正的构建前校验。

**prism-editor-api（新模块 `build_profile`）**
- `BuildScene { name, path, enabled, buildIndex: Option<usize> }` —— Unity 的
  `EditorBuildSettingsScene`；启用项按顺序拿到连续的 `build_index`（0..n），禁用项为 `None`。
- `BuildOptions { development, allowDebugging, connectWithProfiler }` —— 由 `[build].profile`
  派生：`debug` → `development` + `allowDebugging`，`release` → 全关；
  `connectWithProfiler` 目前没有宿主支持，恒为 `false`。
- `BuildProfile { name, target, targetGroup, options, outputDir, runAfter, scenes }`，
  `name` 即 `[build].profile`，`target` 即 `[build].target`。
- `target_group(target)`：`windows|linux|macos` → `Standalone`、`android` → `Android`、
  `ios` → `iOS`、`web|webgl|wasm` → `WebGL`、其它 → `Unknown`。
- `BuildProfile::validate()`：目标为空 / 输出目录为空 / **0 个启用场景**都返回可读错误。
- `read_build_profile(root)` = `read_project_settings` + `read_scene_manifest`，
  不引入任何 cooker/build-pipeline 依赖。

**刻意不做的事**
- 不发明多目标枚举：`settings.rs` 的校验目前只允许 `[build].target == "windows"` 与
  `[build].profile ∈ {debug, release}`，所以这一层只做**字符串 → 目标组**的投影，
  等设置层真正支持多目标时再加强类型枚举（避免两套真相）。

**editor（Vue + Tauri）**
- Tauri 命令 `editor_build_profile`；`build_project_blocking` 在启动 `cargo build` 之前调用
  `read_build_profile(&root)?.validate()?` —— 没有启用场景时立刻失败并给出明确错误
  （此前会白跑一次 cargo build）。
- `BuildPanel`：标题显示目标组 / 目标 / profile；场景行的序号改用**有效 build index**
  （跳过禁用项，与 Unity 的 `SceneManager.GetSceneByBuildIndex` 一致）；底部增加一行
  `Unity Build.Profile` 摘要（profile、目标组、启用场景数、开发/发布、是否允许调试）。

**验证**
- ✅ `cargo test -p prism-editor-api` → 85 passed（新增 7 个：目标组映射 / `BuildOptions`
  派生 / 连续 build index / profile 字段 / `validate` 三类错误 / camelCase JSON /
  临时目录读取 settings+manifest）。
- ✅ `cd editor && pnpm build`、`python3 scripts/pr_ready.py`。

**已知遗留**
- 还没有 `BuildReport`（构建结果 / 体积 / 耗时）；`Compilation`（`CompilationPipeline`）
  形状也未做。
- fixture 不固化构建 profile：`assets/scenes.toml` 可能含本机绝对路径，浏览器预览的
  `buildProfile` 由 `mock.ts` 用 fixture 内场景即时合成。

## 附录

- `docs/unity_api_inventory.md` — Unity public 类型全清单（按模块/命名空间，XR 排除）。
- `docs/unity_gap_matrix.md` — 类型级机械差距矩阵（✅/◐/✗）。
- 参考源码快照：`/tmp/opencode/unitysrc/UnityCsReference-7000.0.0a12`（未纳入本仓库）。
