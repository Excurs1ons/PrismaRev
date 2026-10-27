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
| 相机控制 | 左键轨道 / 中键或 Shift 平移 / 滚轮缩放，参数由前端持有并整体下发 | 转发鼠标、滚轮、少量按键给 `InputManager` |
| Play | 不受影响 | 工具栏「运行」= 重载场景后开始模拟 |

编辑器相机**不写进文档**（它只是视口元数据，不应该被 cook 进场景）。

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

- 没有拾取 / gizmo / 选中高亮：都是「在视口上做一点再回写文档」的编辑功能，
  需要文档 ↔ 实体的双向映射（当前实体 id 只在加载时分配）。
- 每次编辑都整场重载（debounce 450ms）。大场景可以考虑增量同步（只重建改动的实体）。
- 每个视口一个 Vulkan 实例 + 一个渲染线程；两个视口同时可见时显存/带宽翻倍。
- 输入映射只覆盖 `KeyCode` 里列出的键（`viewport.rs::key_code_from_code`）。
- `editor_play_scene`（spawn 真正的游戏二进制）保留给 MCP / 自动化，
  用来验证发布路径；编辑器 UI 不再使用它。
