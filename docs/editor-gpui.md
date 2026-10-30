# GPUI 原生编辑器（`editor-gpui/`）— 已停止维护

> **状态：停止维护（legacy）。** 主线已切到 [`editor-egui/`](../editor-egui/README.md)
> （egui + ash，Scene/Game 视口零拷贝，见 [`docs/editor-egui.md`](editor-egui.md)）。
> GPUI 0.2.2 没有外部纹理导入入口（[#60572](https://github.com/zed-industries/zed/discussions/60572)
> 仍是 proposal），只能 CPU 回读或原生 overlay，达不到零拷贝目标。
>
> 本目录仍然保留：`crates/prism-editor-gpui-core` 是 UI 框架无关的数据层，
> `editor-egui` 直接依赖它。`editor/`（Tauri + Vue）仍是内容与发布的参考实现。
>
> 下面记录的是 GPUI 壳的实现与调研结论，供历史参考。
>
> 本文说明「为什么做、边界在哪、怎么接真实引擎」，功能清单见
> [`editor-gpui/README.md`](../editor-gpui/README.md)。

## 1. 定位

`editor-gpui/` 是 PrismaRev 编辑器的**第二个外壳**，和 `editor/`（Tauri + Vue）
并列：

```text
                     ┌──────────────────────────────┐
   .scene.json  ───▶ │ prism-editor-gpui-core       │ ◀── schema / 模板 / fixture
   .prefab.json      │ dock · doc · undo · theme    │
                     │ i18n · log · assets · keys   │
                     └────────────┬─────────────────┘
                                  │ Workspace
                     ┌────────────▼─────────────────┐
                     │ prism-editor-gpui (GPUI)     │
                     │ 菜单/工具栏/dock/浮动窗口      │
                     │ 层级 · 检查器 · 资产 · 日志    │
                     │ Scene / Game View (canvas)   │
                     └────────────┬─────────────────┘
                                  │ 下一步：离屏帧 or 原生 surface
                     ┌────────────▼─────────────────┐
                     │ prism-app / prism-engine     │
                     │ prism-render (Vulkan)        │
                     └──────────────────────────────┘
```

**硬约束**：GPUI 只出现在 `editor-gpui/` 里。
`prism-app` / `prism-engine` / `prism-render` 仍然 UI 框架无关，游戏运行时依赖链
（`prisma → prism-app → prism-engine → …`）完全不受影响。

## 2. 为什么另起一个 GPUI 外壳

前面比较过 egui / Dear ImGui / GPUI / Tauri WebView，结论是：

- **WebView 在编辑器里是嵌入成本最高的一类**：原生 Vulkan 视口要和
  WebView2 的合成层、输入转发、DPI、焦点搏斗（`editor/` 已经付出过这份成本）。
- **egui** 的 Vulkan/Ash 集成证据最足，但 docking、复杂属性面板、资产树需要大量自建，
  产品上限受 immediate-mode 约束。
- **GPUI** 是 Rust 原生、GPU 加速、hybrid retained 模型，适合做长期维护的桌面 UI；
  它的不确定项是 **「把现有 Vulkan 渲染嵌进来」的路径**，而不是 UI 能力本身。
- **Dear ImGui** Vulkan 接入最成熟，但定位是工程调试 UI，不适合做内容编辑器。

所以这次的做法是：**先用 GPUI 把「编辑器产品层」完整做出来**，把 Vulkan 视口
留成一个明确的接线点（`viewport.rs::paint_scene`）。这样既能验证 GPUI 是否撑得住
形状的编辑器，又不会把 `prism-render` 绑死在某套 UI 生命周期上。

## 3. 与 Web 编辑器的共享契约

| 契约 | 文件 | 说明 |
|---|---|---|
| dock 布局 v4 | `editor/src/dock.ts` ↔ `core/src/dock.rs` | JSON 形状一致：`{version,dock,floating}`，`type: leaf/split` |
| 主题 | `editor/src/theme.ts` ↔ `core/src/theme.rs` | 38 token × 6 预设，id 与文案一致（机械生成） |
| 文案 | `editor/src/locales/*.ts` ↔ `core/src/locales/*.json` | 331 条，key 集合有测试 |
| 组件 schema | `prism-editor-api::build_schema` ↔ `core/src/schema.rs` | `FieldSchema` / `PropertyDrawer` / `PropertyAttribute` |
| 文档 | `.scene.json` v3 | 稳定 `id`、`parent` 指向 id、Transform 是作者字段 |
| fixture | `editor/public/demo.json` | `scripts/sync-demo.sh` 同步；也 `include_str!` 内嵌 |

这样同一个项目可以在两个外壳之间切换，用户可以只装 GPUI 版（小体积、无 WebView），
也可以继续用 Web 版（Web 生态、CDP 自动化）。

## 4. Dock 模型（与 Web 一致）

- `DockNode::{Leaf, Split}`；`Split { dir, ratio, a, b }`。
- 拖 splitter 只改共享 `ratio` → 同侧对齐的面板一起缩放（/VS 的行为）。
- tab 拖拽：leaf 中央 = 合并成 tab（支持同 leaf 内重排）；上下左右 25% = 新开分屏；
  主窗口左/右/底边 12px = 新开整列/整行；窗口内其它位置 = 浮动。
- 浮动窗口：`{x,y,width,height,z}`，8 方向 resize、松手磁吸（视口边缘 + 其它面板
  的四边/中心线/28px 级联偏移），坐标与尺寸都有 clamp。
- 持久化用 `DockLayout::to_json` / `from_json`：未知 tab 丢弃、空 leaf 剪掉、
  单孩子 split 折叠。

渲染侧的实现要点：

- 递归时把**绝对 origin** 传下去，顺带把每个 leaf 的 `Bounds` 记进 `leaf_rects`，
  松手时用它做命中测试（窗口坐标先减去工具栏高度）。
- 浮动层在 dock 之后绘制，天然在 DOM 之上；8 个 resize 热区是绝对定位的透明 div。

## 5. 检查器：schema 驱动

- 控件由 `FieldSchema::widget()` 决定（`kind` / `structType` / `variants` / `drawer`）。
- 数值字段支持**拖动**（`DragState::NumberField`，位移 × step）；`Shift+点击`进入文本输入。
- 写回统一走 `Workspace::set_field`（等价于 Web 的 `editor_set_component_field`）：
  写 `components[component][path]`，失败就不改；步进进入撤销栈并按字段合并。
- `Transform` 不在 `components` 里（作者字段），走 `set_transform_axis`，撤销也单独合并。
- 没有 schema 的组件 → 直接显示 pretty JSON（保证旧文档/新组件不会「打不开」）。
- **删除 / 复制**：删除整棵子树；`duplicate_subtree` 复制根 + 后代并把内部 parent
  映射到新 id，根保留原父级（Ctrl+D 语义）。
- **重命名**：`preview_rename` 逐字改文档但不入栈，`finish_rename(base)` 在提交时补一个
  快照撤销步；Esc 直接回滚到 `base`。
- **多对象编辑**：只显示所有选中实体的公共组件（`common_components`），字段写入走
  `set_field_multi`（一次快照 = 一次撤销），组件移除走 `remove_component_multi`；
  Transform 卡在多选时也显示（以主选择的值作为显示），拖动/输入走
  `set_transform_axis_many`，把同一**局部**分量写到所有选中实体（一次撤销）。
- **内联输入**：所有文本目标（重命名 / 字段 / 搜索 / 另存为 / 资产重命名 / 预制体名）
  共用 `core/src/textinput.rs` 的 `TextInput`：光标 + 锚点（字节偏移、始终在 char 边界），
  支持选区替换、Home/End、Shift 扩选、Ctrl/Alt 按词左右移动、Ctrl+Backspace/Delete
  按词删除、Ctrl+A 全选。UI 只把按键映射成方法调用，提交/预览逻辑不变。
- **实体剪贴板**（Ctrl+C / X / V）与组件剪贴板分开（组件粘贴改成 Ctrl+Shift+V）：
  `copy_selection` 只取 `selection_roots()`（父级也被选中时去掉子级，避免重复），
  每棵树经 `extract_subtree_preserving_links` 重新编号；`paste_entities(parent)`
  逐棵 `instantiate_prefab`（空路径 = 不覆盖链接），根挂到 `parent` 或根级，
  名字按当前文档去重。右键「粘贴」= 挂到被点实体的父级，「粘贴为子级」= 挂到它下面。
  剪切 = 复制 + 删除；多选 Ctrl+D / 创建副本会复制每个顶层子树并选中全部新根。
- **资产选择器**：`assets::is_asset_path_field`（`*_path` + `env_map` / `volume` / `prefab`）
  决定字段右侧是否出现「…」；`Workspace::asset_picker_candidates` 按字段名推断类别
  （mesh→Model、sprite/texture→Texture、hdr/env→Env、audio、material、prefab、scene），
  弹层列出候选路径（当前值用 ● 标记），点击经 `apply_field` 写回（多选同样一次撤销），
  另有「清除（空路径）」。
- **资产拖拽**：资产列表行用 GPUI `on_drag`（`AssetDragPreview` 小卡片），检查器资产
  字段和层级实体行是 drop target：拖到字段直接 `apply_field(String)`，拖 `.prefab.json`
  到层级实体会 `instantiate_prefab_asset` 到该实体下并选中新根。
- **资产浏览器**：目录树 + 面包屑 + 前进/后退历史、列表/网格切换、新建文件夹（写盘后重建
  资产树，空目录也会保留）、目录行作为 drop target（`move_asset_to_folder` → `rename_path`）。
  搜索 / 分类过滤时列全项目资产，否则只列当前目录的直接子项。右键可创建副本
  （`unique_asset_path` 保留 `.scene.json` / `.prefab.json` 双后缀）、复制路径 / GUID、删除。
- **资产检查器**：选中资产时检查器优先显示 `AssetDetails` —— 稳定 GUID、体积、导入器、
  文本预览（UTF-8 边界截断）或图片尺寸（PNG/GIF/BMP/JPEG 文件头解析），以及跨打开 /
  磁盘文档的引用；点击引用会打开对应文档并选中实体。选中实体或视口拾取会清掉资产选择。
- **颜色选择器**：颜色字段的色块点开弹层，可输入 `#rrggbb` / `#rrggbbaa` 或点 8 个预设
  色块；`hex_to_rgba` / `rgba_to_hex` / HSV 互转在 `core/src/theme.rs`，写回仍走
  `apply_field`（多选一次撤销）。
- **多选拖拽层级**：拖到行中间时若被拖的实体在选中集合里，`reparent_selection` 把
  `selection_roots()` 整组挂到目标下（一次快照撤销）；行上/下 25% 仍是单实体排序。
- **递归展开**：`Alt`+点箭头走 `set_subtree_expanded`（根 + 全部后代一起展开 / 收起），
  普通点击只 `toggle_expanded` 当前项。
- **撤销历史面板**：`UndoStack::history()` 给出最近 → 最早的标签，点击第 `i` 条调用
  `Workspace::undo_to(i)`（撤销 `i + 1` 步，回到该操作之前）；清空按钮走
  `clear_history`。面板只反映当前标签页的撤销栈，与 `Ctrl+Z` 完全同源。

 ## 6. 项目文件

 `core/src/project.rs` 只做文件系统，不依赖 engine/render：

 - `find_project_root`：从起点向上找**确实包含 `assets/` 的目录**（与
   `prisma_engine::asset_paths` 同语义）。
- `scan_documents`：递归扫描 `assets/scenes` / `assets/prefabs`（最多 4 层）。
- `scan_assets`：递归扫描整个 `assets/`（最多 12 层，跳过 `.` / `target` / `node_modules`），
  按扩展名归类并填导入器名；`load_project` 用它重建资产面板与检查器候选。
- `read_document` / `write_document`：展示格式是 2 空格缩进 + 结尾换行，读入后
  `normalize()`（补 id、迁移旧 parent、清理悬空父级）。
- `default_save_path`：文件名非法字符替换、重名追加 `-2`；`normalize_save_path`
  把用户输入（可省略 `assets/`、可带反斜杠）规范化。
- 预制体实例：`revert_prefab_instance` 用预制体当前内容覆盖实例子树，保留根名字与
  父级；`sync_prefab_instances` 批量处理同源实例；磁盘优先、fixture 模式回退同名模板。
  `apply_prefab_instance` 反向把实例子树（`extract_subtree`，剥掉根上的实例链接）写回
  链接指向的 `.prefab.json`，只改预制体文件、不动场景。
- 存为预制体：`save_selection_as_prefab` = `extract_subtree`（剥掉实例链接）+
  `sanitize_stem` + 重名后缀 + `write_document`，不修改当前场景。
- 实例化：`instantiate_prefab_asset` 从磁盘（或 fixture 模板）读预制体，
  `instantiate_prefab` 追加到场景并挂到选中实体，自动选中新根、可撤销。
- 打开预制体：实例根检查器的 P 徽章与实体右键「打开预制体」都调用
  `open_project_file(prefab_link)`，已打开则直接切标签。
- 资产删除：`delete_asset` 只允许 `assets/` 内的普通文件，场景/预制体走文档面板；
  引用不做自动清理（检查器里保留悬空值，便于发现）。
- 文档删除：`delete_document` 删磁盘文件 + 打开标签 + 项目文件表，并移除
  `scenes.json` 里对应的条目（立即写盘）；预制体实例保留为悬空链接。菜单与资产面板
  都先弹确认层，确认后才执行。
- 关闭标签：`close_document` 删除标签并修正 `active` 下标（关闭前面的标签不会跳错文档）；
  UI 对 dirty 文档先弹「保存并关闭 / 不保存 / 取消」，保存走 `save_document(index)`，
  因此未激活的标签也能直接保存。
- 资产面板双击 `.scene.json` / `.prefab.json` 调 `open_project_file`：已打开的标签直接切过去，
  否则从磁盘读入并激活。
- 资产重命名 / 移动：`rename_path` 只允许 `assets/` 内、拒绝覆盖；随后
  `SceneDoc::rewrite_reference` 对所有组件的字符串值做**精确匹配**替换（不做子串，
  避免把 `a.png` 误伤成 `a.png.bak`），并把打开文档自己的 `path` 同步过去。

## 7. 本地状态与自动保存

- `core/src/state.rs` 把 Web 编辑器的三份 localStorage 合并成一个 JSON：
  `{version, dock, themePreset, themeOverrides, locale, preferences}`。
- `capture(workspace, prefs)` / `apply(workspace, prefs)` 是唯一入口；读取时对
  dock 走 `DockLayout::from_json`（未知面板丢弃），主题只接受「存在预设 + 合法
  `#rrggbb` token」，locale 非法回退中文。
- 文件默认落在 `<project>/target/editor-gpui/state.json`（fixture 模式落 cwd 的
  `target/`）；启动时恢复，运行中每 3s 仅在内容变化时重写。主题面板里的逐项
  RGBA 覆盖也走同一份状态。
- 自动保存由同一 33ms 定时器驱动：每 5s 检查一次当前文档是否 dirty，项目模式下
  写盘；Play 中跳过。

## 8. 项目设置写回

`core/src/settings.rs` 只读写 `assets/settings.ini` 的三段：

- 读：缺失键回退默认，数字做范围钳制（1..=16384），文件不存在 = 默认值；
- 写：先把现有 TOML 解析成 `toml::Value`，只替换 `[app]` / `[window]` / `[build]`
  里我们负责的键，`[render]` 与未知键原样保留；
- 面板上的「应用设置」调用它；没有项目根时只记录警告，不写文件。

## 8.1 场景清单（`assets/scenes.json`）

`core/src/manifest.rs` 与 Web 编辑器的 Build Settings 同语义：

- 数组顺序 = 构建顺序 = 启动顺序；第一个 `enabled = true` 是启动场景；
- 缺文件 = 空清单；旧条目没有 `enabled` 字段时按 `true` 处理；
- 写回前校验名字/路径非空、名字不重复；直接覆盖原文件（注释会丢，与 Web 编辑器一致）；
- `doc_relative_path` 在 `scenes/x.rscn` ↔ `scenes/x.scene.json` 之间映射，外部格式
  （glTF 等）返回 `None`；
- 构建面板可以勾选/上下移/移除条目，并从 `assets/scenes` 里把尚未入清单的文档加进来
  （名字自动去重为 `foo-2`，路径写成 `scenes/foo.rscn`）；每次改动立即写盘。

## 8.2 构建执行

`core/src/build.rs` 只产出纯数据的构建计划，UI 负责 spawn：

- `game_manifest_path`：仓库布局优先 `projects/prisma/Cargo.toml`，否则项目根
  `Cargo.toml`（用户项目自成 workspace 的情况）；
- 两步：`cargo build --manifest-path <game> [--release]`，然后
  `cargo run -p prism-build-pipeline -- build-scenes`（cwd = 项目根）；
- 产物：`projects/<game>/target/{debug,release}/prisma[.exe]`、`assets/scenes.pak`、
  `assets/settings.ini` → `[build].output_dir`（相对路径相对项目根）；
- `copy_artifacts` 跳过不存在的产物（例如还没 cook 过 `scenes.pak`），不算失败。
- UI 用后台线程 + `mpsc` 发 `BuildEvent::{Line,Step,Done}`；stdout/stderr 各一个读取线程
  （避免管道写满死锁），`background_tick` 每 tick 最多消费 40 行，日志同时进
  构建面板与日志面板。每个 step 开始时发 `Step { index, total, label }`，面板头部显示
  「构建中 i/n · 步骤名」，并用 n 段进度条标出已完成步骤。

## 9. Scene / Game View 的现状与接线

当前 `viewport.rs` 用 `gpui::canvas` 做**CPU 投影**：

- 轨道相机（target / yaw / pitch / distance）投影到屏幕，画 21×21 世界网格、
  未选中实体的线框、选中实体的线框 + 三色 gizmo 轴；
- 左键：先试方向 gizmo（右上角三轴端点，9px 内 → 相机转到该轴），再试 gizmo
  （移动/缩放拖轴 8px 内；旋转拖圆环 10px 内，Ctrl 吸附 0.25），
  否则射线拾取实体（单位立方体 × scale 的近似，双击同一实体 = 取景）；点击空白清空选择，
  在空白处拖拽 = **框选**（投影中心 / 8 个角任一落在矩形内即选中，Shift 追加）；
  `Alt`+左键或右键轨道旋转、中键平移、滚轮缩放；`Q` / `W` / `E` / `R` 切换选择 / 移动 /
  旋转 / 缩放工具，`X` 或标题栏按钮切换世界 / 局部坐标，`G` 开关网格，
  `End` 把选中实体贴到地面（`snap_selection_to_ground`，按世界 AABB 半高），
  `Ctrl+Shift+F` 视角对准选中、`Ctrl+Alt+F` 把选中移到轨道中心。多选时线框全画、
  gizmo 画在主选择上，三种变换都支持**批量变换**（整组一个撤销步）：平移共用世界位移，
  旋转绕各自位置，缩放保留各自起始比例。标题栏「统计」显示 FPS / 帧号 / 实体数 /
  选中数 / Play 时间 / 相机 target 与距离。
- 旋转在世界空间左乘 `q_axis * q_old`，局部空间右乘 `q_old * q_axis`；局部模式的
  三轴取实体沿父链累积的世界旋转（`world_rotation` / `gizmo_basis`）。平移把世界位移
  经**父级世界旋转的逆**换算成 `position` 增量（`translate_entity_world`），所以父级
  旋转后拖轴也不会漂。缩放夹在 0.01..=1000，避免退化矩阵。两者都走 `push_transform`，
  一次拖动合并成一步撤销。
  Game View 只读，点击按归一化视口坐标记日志（将来直接转成游戏输入）。
- **真实 Vulkan 帧（可选 feature `engine-viewport`）**：`engine_viewport.rs` 在后台线程
  创建 `prism_app::App::hosted`，`load_scene_json_file` 加载
  `<project>/target/editor-gpui/engine-viewport.scene.json`，`start_hosted` 后 30Hz
  `host_tick`；RGBA 帧经 channel 回到 UI，用 `image::Frame` + `RenderImage` 贴图。
  文档变化 300ms 节流重写快照并 `reload`，视口尺寸变化 `host_resize`，
  Play/Pause/Step 通过 `set_simulate` / `step` 同步。引擎初始化失败（无 Vulkan 设备）
  会回退 CPU 投影并在视口左上角显示错误条。选中线框 / gizmo / 方向 gizmo 仍由
  `canvas` 以 overlay-only 模式画在帧上。
- 没有 feature 时 `paint_scene` 是纯 CPU 投影；两条路共用同一套拾取 / gizmo 数学，
  所以交互不依赖引擎是否可用。
- **当前仍是 CPU 回读**：引擎离屏 image → `vkCmdCopyImageToBuffer` → CPU `Vec<u8>` →
  channel → GPUI `RenderImage`（再上传为 GPUI 纹理）。1280×720 RGBA 每帧 ~3.5 MiB，
  30 Hz 约 105 MB/s；静止时引擎线程不再 `host_tick`，只保留首帧/文档变更/Play 时的回读。
- **GPUI 0.2.2 没有通用 GPU interop**：`gpui::surface()` / `Window::paint_surface`
  只接受 macOS `CVPixelBuffer`（`#[cfg(target_os = "macos")]`），Windows/Linux 没有
  公开的外部纹理导入 API；`wgpu-graft` 的 GPUI demo 也因此故意走 CPU 回读。上游
  「external compositor」仍是 proposal（zd#60572），不在 0.2.2。
  要真正零拷贝只有三条路：
  1. 原生 surface / overlay 窗口（`window.window_handle()` 拿原生句柄，自己建 swapchain；
     Tauri 的 Win32 popup 已验证，但输入、层级、DPI、浮动面板遮挡都要自己处理）；
  2. fork / 升级 GPUI，实现 external compositor 或暴露 `wgpu::Device` 导入 Vulkan
     external memory（需要引擎侧导出 dmabuf / NT handle / IOSurface）；
  3. 只接受 macOS 的 `CVPixelBuffer` 路径（引擎需要 IOSurface external memory）。
  没有现成的 `gpui_interop` 依赖可以直接加。
- **同类问题的现状（2026-10 调研）**：
  | 方案 | 状态 |
  |---|---|
  | Zed 上游 `external compositor`（[#60572](https://github.com/zed-industries/zed/discussions/60572) / PR #60573） | 仅有 proposal + fork `0xCarbon/zed/external-compositor`，Wayland + RADV 验证过 zero-copy；未合入 0.2.2，且目标树是拆分后的 `gpui_wgpu` |
  | [wgpui](https://github.com/nestrilabs/wgpui)（GPUI-CE 的 wgpu + winit fork） | 已有 `WgpuSurface`：双缓冲 `wgpu::Texture`，外部渲染器写 `back_buffer_view()` 后 `present()`，合成器直接采样，无 CPU 拷贝 |
  | `mdeand/gpui-wgpu` | 同样提供 `WgpuSurface` / 外部渲染集成 |
  | gpui-native 的 fork | 有 leased external surface（WebGPU canvas），但明确不是 Windows 零拷贝；GPU 拷贝路径 |
  | longbridge/gpui-kit [#3168](https://github.com/longbridge/gpui-kit/issues/3168) | 仍在请求稳定 seam，下游靠 patch fork |
  | [wgpu-graft](https://github.com/merely-made/wgpu-graft) / `wgpu-native-texture-interop` | 提供 Vulkan image / dmabuf / IOSurface / DX12 shared texture → wgpu 的导入；其 GPUI demo 仍走 CPU 回读，因为公开 GPUI 不暴露设备 |
  OS 层原语本身都成熟：Windows 用 D3D12 shared NT handle ↔ `VK_KHR_external_memory_win32`（同适配器、共享 fence / keyed mutex）；Linux 用 dmabuf + `VK_KHR_external_memory_fd` / DRM format modifier + sync_file；macOS 用 IOSurface（MoltenVK `VK_EXT_metal_objects`）；Android 用 AHardwareBuffer。难点在 GPUI 侧没有导入入口，以及格式/tiling/同步/适配器兼容。
- 对 PrismaRev 来说，`prism-render` 是 ash Vulkan：
  1. 继续用 GPUI 0.2.2 → 只能 CPU 回读或原生 overlay surface；
  2. 迁到 wgpui / wgpu-based fork → 引擎需要 wgpu 设备，或在 wgpu 侧用 external memory 导入 Vulkan image（GPU→GPU 拷贝一次，仍无 CPU 回读）；
  3. 跟进 0xCarbon/zed 的 external compositor → 要先升级到拆分后的 GPUI 树，成本最大。
- 原生 surface（Windows owner popup + 输入转发）仍是可选的下一步，离屏图像路线已经覆盖
  跨平台需求。

选 1 还是 2 取决于：Scene View 是否需要 DOM/canvas 覆盖层（GPUI 没有 DOM，
覆盖层可以自己画，所以路线 1 的缺点比 WebView 小），以及是否接受一帧 CPU 拷贝。

## 10. 验证

本机（Termux/Android）没有显示服务器，GPUI 无法运行；可执行的是：

```sh
cd editor-gpui
cargo test -p prism-editor-gpui-core                                  # 192 passed
cargo check -p prism-editor-gpui --target x86_64-pc-windows-gnu       # 类型检查
cargo clippy -p prism-editor-gpui -p prism-editor-gpui-core \
    --target x86_64-pc-windows-gnu                                    # clean
```

CI 覆盖：

- `.github/workflows/ci.yml` 的 `editor-gpui` job（ubuntu）跑数据层单测；
- `scripts/pr_ready.py` 在改动 `editor-gpui/` 时跑数据层单测，并在本机装有
  `x86_64-pc-windows-gnu` 目标时追加 `cargo check -p prism-editor-gpui --target
  x86_64-pc-windows-gnu`（Termux 上的替代验证方式）。

GPUI 的**运行**仍然需要桌面环境（Windows/macOS/Linux + GPU），CI 只做类型检查。
