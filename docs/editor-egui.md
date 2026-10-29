# egui 编辑器（`editor-egui/`）

PrismaRev 的第三个编辑器壳，也是**当前主线**：`editor-gpui/` 已停止维护，
Web 编辑器（`editor/`，Tauri + Vue）仍是内容与发布的参考实现。

设计目标只有两条：

1. **布局和功能对齐 Unity / Web 编辑器**：菜单 + 工具栏 + 状态栏、split-tree
   docking、层级 / 检查器 / 资产 / 文档 / 日志 / 历史、预制体、撤销、项目读写。
2. **Scene / Game 视图零拷贝**：引擎渲染的图像直接被 UI 采样，不经过 CPU。

## 1. 为什么 egui 能做到零拷贝

GPUI 0.2.2 的公开 API 只有 CPU 像素（`RenderImage`）和 macOS 专用的
`surface(CVPixelBuffer)`，也不暴露 `wgpu::Device`，所以只能回读或走原生 overlay
（详见 `docs/editor-gpui.md` §9）。

egui 不是渲染器：它只产出 UI 的 mesh + 纹理 id，Vulkan 设备、命令缓冲区、
swapchain 都在宿主手里。`egui-ash` 的 `AppCreator::create` 要求宿主自己创建
instance / device 并返回 `AshRenderState`，因此可以做到 Unity 的模型：

```text
同一个 VkDevice / VkQueue
├─ prism-render GraphRenderer（Shadow → Scene → GTAO → Post → 离屏 image）
├─ egui-ash（UI 顶点 / 字体纹理 / 采样视口 image）
└─ swapchain present
```

`ImageRegistry::register_user_texture(image_view, sampler)` 把引擎的离屏图像注册
成 `egui::TextureId`，`painter.image()` 在**同一命令缓冲区**里采样它。GPU→GPU，
没有读回。

## 2. 设备共享的接线

`prism-render` 新增两个能力（都在 `crates/prism-render/src/context.rs`）：

- `create_host_instance(entry, extensions)`：宿主（UI 框架）自己持有 instance；
- `VulkanContext::from_host(entry, instance, physical, family, extra_device_extensions)`：
  用**与窗口模式完全相同的特性集**（bindless、synchronization2、时间线信号量、
  设备地址、条件 RT）在宿主 instance 上创建设备，但**不拥有**它
  （`ContextOwnership::External`，析构时不销毁，避免 UI 侧句柄悬空）。

`pick_host_physical_device` / `pick_present_queue_family` 保证选中的物理设备与
队列族同时支持图形和呈现（egui-ash 每个窗口自己建 surface + swapchain）。

离屏图像侧（`crates/prism-render/src/offscreen.rs` + `graph_renderer.rs`）：

- `OffscreenTarget` 从 `pub(crate)` 变成 `pub`，暴露 `image_view()` / `extent()` /
  `format()`，图像 usage 增加 `SAMPLED`；
- `GraphRenderer::offscreen_new_with_context(Arc<VulkanContext>, w, h, env)`：
  在共享设备上建离屏渲染器；
- `GraphRenderer::offscreen_present_for_sampling()`：帧结束（PostPass 的 final
  layout 是 `PRESENT_SRC_KHR`）后插入 `PRESENT_SRC_KHR → SHADER_READ_ONLY_OPTIMAL`
  的 barrier 并等待完成，随后图像可以交给 egui 采样；
- `offscreen_image_view()`：注册纹理用的 `VkImageView`。

下一次帧的 PostPass 用 `initial_layout = UNDEFINED`（load op clear），所以从
`SHADER_READ_ONLY_OPTIMAL` 回到颜色附件是合法的，不需要额外的状态机。

## 3. 帧节奏

编辑器**不建渲染线程**（与 `prism-app` 的 hosted 模式不同）：`Engine` +
`GraphRenderer` 直接跑在 egui-ash 的事件循环线程上，`App::ui()` 里同步出帧。

- 资产上传也同步：`GpuAssetResolver::prepare_requests` → `GraphRenderer::
  apply_asset_requests` → `apply_responses` 写回 ECS（原本是两个线程各一半）；
- 视口只在「文档变化 / 相机变化 / 选中变化 / resize / Play / 首帧」时重渲染，
  静止时零 GPU 工作；
- Play 时 Game view 每帧 `tick()` + 重渲染；Scene view 仍是编辑态。

## 4. 交互

- 轨道：左键/右键拖拽 → `OrbitCamera::orbit`；滚轮 → `zoom`；
- 点选：屏幕坐标 → 射线 → 与文档实体的单位立方体代理求交（与 GPUI / Tauri
  编辑器同一套 `viewport_math`，索引即 `.scene.json` 的 `entities[i]`）；
- 平移 gizmo：`pick_gizmo_axis` 命中轴后，按「屏幕位移 · 轴屏幕方向 / 每世界
  单位像素」换算世界位移，交给 `Workspace::translate_entity_world`；
- 选中高亮复用引擎的 `Gizmo` 组件（世界三色轴箭头），不引入新的渲染概念。
- 工具：`W` / `E` / `R` 切换移动 / 旋转 / 缩放 gizmo，`X` 切世界/局部，`G` 开关网格；
  旋转走 `pick_rotation_axis` + `screen_angle`，缩放走 `set_scale_axis`。
- 空白拖拽 = 框选（`box_select`），`Alt`+拖 / 右键拖 = 轨道，双击 = 聚焦选中项。

## 4.1 日志

`logging.rs` 把 `env_logger` 包一层 tee：同一条 `log` 记录既进终端（`RUST_LOG`
语义不变），也进 `prism-editor-core` 的 `LogBuffer`，所以 Console 面板能看到
视口里 `prism_render` / `prism_engine` 的警告（与 Tauri 编辑器的 `EditorLogger`
是同一思路）。`Trace` 不转发，避免淹没窗口。

## 4.2 菜单（Unity 结构）

菜单按 Unity 的分组组织，二 / 三级菜单里都是真实命令：

| 菜单 | 二级 / 三级 |
|---|---|
| File | New Scene / New Scene From Template ▸ / New Prefab / Open / Recent Scenes ▸ / Save / Save As / Save Project / Reload / Build Settings / Build Scene Pack / Cook Active Scene / Build Player / Build And Run / Clean Build / Exit |
| Edit | Undo / Redo / Cut / Copy / Paste / Duplicate / Delete / Select All / Deselect All / Invert Selection / Select Children / Select Prefab Root / Next·Previous Selection / Copy·Paste Transform ▸ / Paste Component / Play / Pause / Step / Project Settings / Preferences / Theme |
| Assets | Create ▸ (Scene / Prefab / Folder…) / Open / Show in Explorer / Copy Path / Duplicate / Delete / Instantiate In Scene / Find References In Scene / Find References In Project / Refresh |
| GameObject | Create Empty / Create Empty Child / Create Empty Parent / Create Other ▸ (3D Object ▸ Cube… / Light ▸ / Camera ▸) / Move To View / Align View to Selected / Snap to Ground |
| Component | schema 分类 ▸ 组件列表 |
| Window | General ▸ 面板开关 / Layouts ▸ 预设 / Reset Layout |
| Help | Shortcuts / Editor Guide / About |

`Build*` 与 `Cook Active Scene` 通过子进程调用 `prism-build-pipeline`（编辑器仍然
不链接 cooker），输出进 Console。

## 4.3 面板能力

- **层级**：搜索、Unity 式拖拽（上/下 25% 插入、中间挂成子级）、就地重命名（双击）、
  显隐、复制 / 删除 / 改父级、Create 下拉；
- **检查器**：单选 + 多选（公共组件批量改字段，一次撤销）；预制体区块（Revert /
  Sync All / Save as Prefab…）；组件卡头的复制 / 移除；Add Component 分类菜单；
- **Project（资产）**：后退 / 前进 / 面包屑、目录浏览、类型过滤、搜索、重命名 /
  复制 / 删除 / 复制路径 / 实例化预制体；**资产可拖拽**：拖到目录 = 移动并改写引用，
  拖到层级实体 = 实例化预制体，拖到检查器的路径字段 = 直接写入路径（`AssetDrag` 载荷）。
- **Docking**：拖动标签页（Unity 行为）：拖到另一个 leaf 的左/右/上/下 25% = 分屏，
  中间 = 合并成标签，拖出面板区 = 浮动窗口，拖到本 leaf 边缘 = 原地分屏。
- **预制体**：实例区块有 Revert / Sync All / **Apply to Prefab**（当前子树覆盖预制体
  文件，再同步同源实例）/ Save as Prefab…（无链接时）。
- **视口拖放**：预制体拖进 Scene / Game 视口 → 实例化并放到光标下的地面点
  （射线 ∩ `y = 0`）；`.scene.json` 拖进视口 = 打开该场景文档。
- **分辨率倍率**：渲染图面板里的 `Viewport res`（0.5×–1.5×）只缩放离屏目标，
  不改变 UI 的 points 布局，给高 DPI 屏 / 弱 GPU 一个折中。
- **多选拖拽**：拖多选中的任一行且落在行的中间区 = `reparent_selection` 整组改父级。
- **资产选择器**：检查器的路径字段右侧有 `⋯` 按钮，按字段名过滤扩展名
  （`mesh_path` → 模型、`sprite_path` → 贴图、`hdr_path`/`env_map` → HDR…），
  选中的相对路径直接写入字段。

## 5. 与另外两个编辑器的关系

| | Web（`editor/`） | GPUI（`editor-gpui/`） | egui（`editor-egui/`） |
|---|---|---|---|
| 状态 | 内容参考 / 发布 | 停止维护 | 主线 |
| 数据层 | Rust + TS 各一份 | `prism-editor-gpui-core` | 复用 `prism-editor-gpui-core` |
| 视口 | 进程内引擎 + CPU 通道 | CPU 回读（可选原生 surface） | **零拷贝（同设备纹理）** |

`prism-editor-gpui-core` 只是历史命名，它不依赖 GPUI；egui 编辑器通过路径依赖
直接使用它。将来如果 GPUI 目录整体删除，把该 crate 改名 / 迁到
`crates/prism-editor-core` 即可（`editor-egui` 里只有一处 `package = ...` 需要改）。

## 6. 未完成 / 风险

- **未在真实设备上运行**：开发机是 Termux（aarch64、无显示），只做了
  `cargo check`（本机 + `x86_64-pc-windows-gnu`）。首帧呈现、resize、字体、
  docking 手感都需要桌面复核。
- **尚未实现**：材质 / 贴图拖到场景里自动挂 `MeshRenderer`（目前只有预制体和场景
  文件有落点语义）、层级多选拖到两个实体之间做整组排序（整组只支持改父级）、
  多窗口（egui viewport）的独立 DPI。
- **性能**：每次文档变化都会整场重载两个引擎实例（防抖 120ms），大场景下偏重；
  后续可以改成「只重建变化的实体」或引入 delta 上传。
- **两个引擎实例**意味着两份 GPU 资源（管线、IBL、网格）。如果以后只保留一个
  视口可见，可以按可见性懒加载 / 回收 Game view。
