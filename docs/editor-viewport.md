# 编辑器内置视口（Scene / Game view）

> 2026-10-27 起，编辑器**在自身进程内**跑 engine 实例并渲染 Scene / Game 视图。
> 「运行」不再 spawn `projects/prisma` 游戏进程。

## 为什么是这个形状

需求是「editor 下运行项目不再需要其他进程，所有子系统由 editor 持有的 engine 实例」。
可选的三条路：

1. **子窗口嵌入**（native child HWND）——winit 0.30 没有 `with_parent_window`
   （只有 `with_owner_window`），且 z-order / DPI / 输入转发全是 Windows 私有行为，
   无法在 CI 或非 Windows 上验证。
2. **共享纹理 / 外部内存**（editor 与游戏进程跨进程共享 Vulkan 图像）——需要额外进程，
   直接违反需求。
3. **同进程离屏渲染 + 帧回读**（当前实现）——engine 真的跑在编辑器进程里，
   帧走一条普通的字节通道，平台无关、可测（`crates/prism-app/tests/hosted_viewport.rs`）。

代价是每帧一次 CPU 读回 + IPC（≈ 面板分辨率 × 4 字节）。对 30fps、≤1600×900 的编辑器
视口来说可以接受；真正的运行时（launcher / APK）仍然走交换链直出，没有这条路径。

## 分层

```
Tauri 主线程                  ViewportManager（editor/src-tauri/src/viewport.rs）
  │  editor_viewport_* 命令         │  每个视口一个线程 + 命令队列
  ▼                                 ▼
Vue ViewportPanel.vue  ◄──── Channel(ArrayBuffer) ──── 视口线程
  putImageData                   │        prism_app::App（hosted）
                                 │          ├─ 主逻辑：host_tick → tick_sim → FramePacket
                                 │          └─ 渲染线程：GraphRenderer::offscreen_new
                                 │                PostPass → 离屏图像 → readback_pixels
```

- `prism-app` 新增宿主模式（`App::hosted` / `start_hosted` / `host_tick` /
  `host_resize` / `take_hosted_frame` / `shutdown_hosted`），不创建 winit 窗口，
  也不读 `LaunchConfig` / manifest。
- `prism-render` 把原本固定 256×256 的 `OffscreenTarget` 参数化
  （`GraphRenderer::offscreen_new(w, h)` + `resize_offscreen`），并给离屏图像加上
  `COLOR_ATTACHMENT` 用途 —— PostPass 直接把 tonemap 结果画进去。
- 渲染线程只比游戏模式多一件事：宿主请求时 `copy_offscreen_to_readback` +
  `readback_pixels`，把 RGBA8 放进 `RenderShared::frame`。

## 场景来源

前端把**当前编辑中的文档**（可能未保存）发给后端，后端写
`<project>/target/editor-preview/<id>.scene.json`，再 `Engine::load_scene_json_file`
按 `SceneSource::JsonFile` 加载。因此：

- 不需要先 cook / 打包 `scenes.pak`；
- 改字段立刻能在视口里看到（前端 450ms debounce 后重建）；
- 视口加载失败不会影响编辑（错误显示在面板浮层，状态经 `editor_viewport_stats` 轮询）。

重载会重建 `Engine` 与资源解析器。`App::load_scene_json_file` 会先与渲染线程同步
（等已排队的资产上传落地）再丢弃结果，避免旧场景的网格挂到新世界同 id 的实体上。

## Scene view 与 Game view

| | Scene view | Game view |
|---|---|---|
| 模拟 | 不跑 `Engine::update`，只传播层级 | Play 时跑完整 `update` |
| 相机 | 叠加一台 `depth = 1000`、`clear_flags = SolidColor` 的编辑器相机 | 文档里的相机（无则用 runtime 回退相机） |
| 相机控制 | 左键轨道 / 中键或 Shift 平移 / 滚轮缩放 / `F` 聚焦选中 / `A`·`Home` 框住场景，参数由前端持有并整体下发 | 转发鼠标、滚轮、少量按键给 `InputManager` |
| Play | 不受影响 | 工具栏「运行」= 重载场景后开始模拟；运行中可暂停 / 单步 |

编辑器相机**不写进文档**（它只是视口元数据，不应该被 cook 进场景）。

### 取景

`editor_viewport_frame(id, "all" | "selection")` 返回一份新的 `CameraOrbit`
（`yaw` / `pitch` 保持不变，只换注视点与距离），前端套用后照常走 `editor_viewport_camera`。

- 包围盒是「实体世界矩阵 × 局部单位立方体」的 8 个角——和点选同一个代理，
  对编辑器模板基本体精确、对导入网格是近似。
- 距离 = 包围球半径 / `tan(fov/2)` × 1.2（垂直方向留 20% 余量），再夹到 `0.5..=500`。
- Scene view **第一次出帧时自动框住整个场景**（否则默认轨道可能让内容在相机背后，
  像"什么都没渲染"）；换文档后会重新自动取景一次。

## Play 的暂停与单步

- 暂停 = `HostState.paused`：`simulate_now()` 返回 false，但线程照常 `host_tick(false)`
  → 只渲染 + 传播层级，所以画面不动但仍然是最新的一帧（可以放心看某个瞬间）。
- 单步 = `step_pending`：下一次 `simulate_now()` 返回 true，消费后立刻回到暂停。
- 前端状态（`paused`）在 `App.vue` 里，和 `playing` 一起控制 Game 面板的工具栏按钮；
  后端还会把 `paused` 放进 `editor_viewport_stats` 的快照里作为真相来源。
- 开始 / 停止 Play 都会 `set_paused(false)`：Play 结束不该留下一个"暂停中"的幽灵状态。
- **Play 期间文档改动不会重载 Game view**（`scheduleReload` 在 `playing` 时直接丢弃）：
  否则 Scene view 里拖一下 gizmo 就会把正在跑的模拟重置掉。停止 Play 前先把当前文档
  `editor_viewport_reload` 一次，Game view 回到编辑态时看到的就是最新文档。

## 平移 gizmo（拖动改位置）

Scene view 里选中实体后，`Gizmo` 组件会用三色世界轴标出它的位置，箭头长度按
「屏幕高度的 `GIZMO_SCREEN_FRACTION`」换算（滚轮缩放时视觉大小不变）。

一次拖动经过四个命令：

| 命令 | 作用 |
|---|---|
| `editor_viewport_gizmo_hit` | 光标（canvas 位图坐标）落在哪根轴上 → `"x"` / `"y"` / `"z"` / `null` |
| `editor_viewport_drag_begin` | 记下实体世界位置与该射线在轴上的参数 |
| `editor_viewport_drag` | 新光标 → 新的**局部** `transform.position`（`[f32; 3]`） |
| `editor_viewport_drag_end` | 结束会话 |

约定：

- 拖动量 = 「光标射线与轴线的最近点参数」之差，因此拖动过程中转相机也不会让实体漂移。
- 世界位移经**父级世界矩阵的逆**换算回局部坐标再写文档；箭头与实体都直接改 ECS，
  所以视口跟手，不必等文档重载。
- 前端把每次移动写回文档（`editKey = drag-move:<下标>`，整段拖动合并成一次撤销），
  但**压住重载**直到松手：整场重载会重建实体、让进行中的拖动失效。
- 命中容差 10 像素（`GIZMO_PICK_SLOP`，见 `viewport.rs`），没有悬停高亮。

## 帧协议

```
[0..4)  width  (u32 LE)
[4..8)  height (u32 LE)
[8..)   RGBA8，紧凑打包，左上原点
```

`Channel` 的 payload 大于 1024 字节时 Tauri 走内部 fetch，JS 侧直接拿到 `ArrayBuffer`，
所以不需要 base64。

性能开关都在 `editor/src-tauri/src/viewport.rs` 顶部：`TARGET_FPS = 30`、
`MAX_WIDTH/MAX_HEIGHT = 1600×900`（超出按比例缩小，canvas 负责放大）。
面板被切到后台标签时（`v-show` → 0×0）前端发 `editor_viewport_set_active(false)`，
视口线程停止 tick 与出帧。

## 已知限制（后续可以单独做）

- 拾取是**代理盒子**（实体世界变换 × 局部单位立方体），不是逐三角形求交：大网格
  （地形、Sponza 这类）点边缘可能选不中或选中邻居。要做精确拾取得给网格加 CPU 包围盒
  （改 cooker + 资产格式）或者加 GPU id buffer 通道（改渲染图）。
- 平移 gizmo 只有三根**世界**轴（没有旋转 / 缩放手柄、没有平面手柄，也没有局部轴向）。
  拖动时不能多选（只作用在最后一个选中项）。
- 手柄的视觉就是引擎的 `Gizmo`（三色世界轴箭头）——没有悬停高亮的「当前轴」反馈，
  命中与否靠光标形状（拖动中 `grabbing`）。
- 拖动期间不重载场景（见「平移 gizmo」），所以拖动中做的别的编辑要等松手后才出现在视口里。
- 每次编辑都整场重载（debounce 450ms）。大场景可以考虑增量同步（只重建改动的实体）。
- 每个视口一个 Vulkan 实例 + 一个渲染线程；两个视口同时可见时显存/带宽翻倍。
- 输入映射只覆盖 `KeyCode` 里列出的键（`viewport.rs::key_code_from_code`）。
- `editor_play_scene`（spawn 真正的游戏二进制）保留给 MCP / 自动化，
  用来验证发布路径；编辑器 UI 不再使用它。

## 日志

进程内引擎不再有独立的终端：`editor/src-tauri` 里 `EditorLogger` 把 `log` 记录同时写到
stderr 和日志面板（`stream = "engine"`、`level` 直接给出，前端不用再从文本里猜）。
`RUST_LOG` 仍然生效（内层就是 `env_logger`），默认 `warn,prism_render=info`。
Rust panic 不经过 `log`，只会出现在终端。
