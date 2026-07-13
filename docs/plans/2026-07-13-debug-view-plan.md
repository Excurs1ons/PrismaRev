# 实现计划：PBR 调试视图（Debug View）

> 设计文档：`docs/plans/2026-07-13-debug-view-design.md`（已提交 `5a7bb4a`）
> 目标：在 PBR 渲染上叠加可切换的调试视图（Final / Albedo / Specular / Reflection / Ambient / Normal[World|View|Tangent]），通过桌面键盘 + 应用内 Vulkan 2D 覆盖层按钮切换，无需 JNI。

## 关键约束（来自代码探查）
- **Push constant 预算**：Vulkan 保证最小 128 字节。`draw_mesh_pbr` 当前 push constant = 84 字节（model 64 + albedoMetallic 16 + roughness 4）。扩展为 **92 字节**（追加 `debug_mode: u32` + `normal_space: u32`）仍 < 128，安全。
- **View 空间法线**需要 view 矩阵。当前 `FrameUBOData` 仅含 `view_proj`（112 字节）。新增 `view: [[f32;4];4]`（offset 112，+64 = 176 字节）。UBO buffer 大小由 `size_of::<FrameUBOData>()` 自动推导，无需改 buffer 创建；只需在 `pbr.frag` 的 `FrameUBO` 声明里加 `mat4 view;`（Blinn-Phong 着色器可不改，因其只读前 112 字节且偏移不变）。
- 覆盖层（overlay）复用同一 `render_pass`（color+depth），但 pipeline 关闭深度测试/写入。
- 帧流程：`render_system`（`render_system.rs:158`）调用 `begin_frame` → 遍历实体 `draw_mesh_pbr`（`render_system.rs:201`）→ `end_frame`（`render_system.rs:209`）。覆盖层绘制须插在最后一次 draw 与 `end_frame` 之间。

## 任务拆分（TDD，每步可独立验证 + 提交）

### T1 — Push constant 结构体 + 单元测试
- 新增 `crates/prism-render/src/pbr_push.rs`：
  - `pub enum DebugMode { Final, Albedo, Specular, Reflection, Ambient, Normal }`（repr u32，索引 0..5）。
  - `pub enum NormalSpace { World, View, Tangent }`（repr u32）。
  - `#[repr(C)] pub struct PbrPushConstants { pub model: [[f32;4];4], pub albedo_metallic: [f32;4], pub roughness: f32, pub debug_mode: u32, pub normal_space: u32 }`。
- 测试 `pbr_push` 模块：
  - `size_of::<PbrPushConstants>() == 92`
  - `offset_of!(model)==0`, `albedo_metallic==64`, `roughness==80`, `debug_mode==84`, `normal_space==88`
- 提交。

### T2 — 把 debug 状态贯穿到 draw_mesh_pbr + render_system
- `draw_mesh_pbr`（`renderer.rs:743`）签名追加 `debug_mode: u32, normal_space: u32`；用 `PbrPushConstants` 构造并 `cmd_push_constants`（替换原 `[f32;21]` 数组）。
- `render_system`（`render_system.rs:158`）签名追加 `debug_mode: u32, normal_space: u32, show_ui: bool`；透传给 `draw_mesh_pbr`。
- `App::render_one_frame` 传入 `self.debug_mode as u32` / `self.normal_space as u32` / `self.show_ui`。
- 更新所有调用点（含测试/示例）。
- 提交。

### T3 — FrameUBO 增加 view 矩阵（供 View 空间法线）
- `descriptor.rs:100` `FrameUBOData` 追加 `pub view: [[f32;4];4]`（offset 112）。
- `OrbitCamera` 暴露 `view()`（内部已算，复用）；`render_system` 在 `frame_data` 里填 `view`。
- `pbr.frag` 的 `FrameUBO` 声明追加 `mat4 view;`（Blinn-Phong 着色器可选同步，不改偏移）。
- 测试：在 `descriptor` 模块加 `size_of::<FrameUBOData>() == 176` 断言（或保留注释 112→176）。
- 提交。

### T4 — pbr.frag 调试分支 + 重新编译
- `pbr.frag` push constant 块追加 `uint debug_mode; uint normal_space;`。
- `main()` 末尾按 `debug_mode` 分支输出：
  - Final：原 PBR 结果。
  - Albedo：`albedo`。
  - Specular：`specular`（含 F0 项，可乘个常数便于观察）。
  - Reflection：`sample_radiance(R, roughness*MAX_REFLECTION_LOD)`。
  - Ambient：`ambient`（diffuse IBL + specular IBL 近似）。
  - Normal：按 `normal_space` 取 `N`（World）/ `(view*vec4(N,0)).xyz`（View）/ `T`（Tangent），`*0.5+0.5` 输出。
- `shaders/compile.bat` 已含 pbr.frag；重新编译生成 `pbr.frag.spv`。
- 提交。

### T5 — 几何：Vertex 增加 uv + tangent
- `mesh.rs`：`Vertex` 增加 `uv: [f32;2]`, `tangent: [f32;3]`（5 属性）；更新 `binding_description` / `attribute_descriptions` 偏移；`create_mesh` stride 同步。
- `app.rs` `cube_vertices`：每顶点补 uv（面平面映射）+ tangent（面法线对应切向）。
- `app.rs` `sphere_mesh`：uv = 经纬映射；tangent = 解析切向（d/du 方向）。
- 测试：Vertex stride == 44 字节（11 floats）；attribute 偏移正确（pos0, normal12, color24, uv36, tangent44）。
- 提交。

### T6 — 覆盖层基础设施：位图字体 + pipeline + 着色器
- 新增 `crates/prism-render/src/overlay.rs`：`pub struct Overlay { pipeline, layout, font_image, font_view, sampler, desc_set, desc_layout, desc_pool, vertex_buffer, ... }`。
- 内置点阵字体：const 表（覆盖 `Final Albedo Specular Reflection Ambient Normal World View Tangent 0-9` 等所需字形），初始化时烘焙到一张 `R8`/`RGBA8` atlas 纹理（白字透明底）。
- 新增 `shaders/overlay.vert` / `shaders/overlay.frag`：顶点格式 `vec2 pos(clip); vec2 uv; vec4 color;`；frag 输出 `vColor * texture(tex, vUV).a`。加入 `compile.bat`。
- `Renderer::new` 中创建 overlay pipeline（复用 `self.render_pass`，depthTest/Write = false），并建字体 descriptor set。
- 测试（纯逻辑，不依赖 GPU）：
  - `glyph_cell(char) -> Option<(u,v,w,h)>` 映射正确。
  - `button_rects(extent) -> Vec<Rect>` 6 个按钮矩形（像素，左上原点）互不重叠且落在屏内。
  - `hit_test(point, rects) -> Option<usize>` 命中判定正确（含边界外返回 None）。
- 提交。

### T7 — 覆盖层布局 + 绘制 + 命中接线
- `overlay.rs`：
  - `layout(extent, debug_mode, normal_space) -> (Vec<Quad>, Vec<Rect>)`：生成 6 个按钮背景 quad + 文字 quad；返回按钮矩形供命中。
  - `draw(&self, cmd, extent, debug_mode, normal_space, show_ui)`：若 `!show_ui` 跳过；把 quad 写入 host-visible 顶点缓冲；bind overlay pipeline + 字体 set；`cmd_draw`（6 顶点/quad）。
  - `hit_test(px, py, extent) -> Option<OverlayAction>`（`OverlayAction::{SetMode(DebugMode), CycleNormalSpace}`）。
- `Renderer` 增加 `overlay: Overlay` 字段；`render_system` 在 `end_frame` 前调用 `renderer.draw_overlay(...)`（新增 `Renderer::draw_overlay` 包装，内部取 `self.current.command_buffer`）。
- 提交。

### T8 — 输入接线（键盘 + 指针命中）
- `app.rs` `KeyboardInput`：`Digit1..=Digit6` → `debug_mode`；`KeyN` → `normal_space` 循环；`KeyH` → `show_ui` 取反。
- `MouseInput`/`Touch` pressed：用 `input.mouse_position`（像素，左上原点）调用 `overlay.hit_test` → 应用 `SetMode`/`CycleNormalSpace`。
- 提交。

### T9 — 全量验证
- `cargo clippy --workspace --all-targets -- -D warnings` 干净。
- `cargo test --workspace` 全绿（prism-render 含 T1/T5/T6 新测试）。
- 桌面运行确认 6 模式 + 3 法线空间切换正常、覆盖层按钮可点。
- APK 构建 + 安装 + 启动 + `logcat` 无报错；请用户在设备上目视确认 6 模式与 3 法线空间。
- 提交（如有剩余改动）。

## 风险与回退
- 若某设备 push constant 上限 < 128（极罕见），T1 的 92 字节仍安全；view 矩阵走 UBO 而非 push constant，已规避。
- 覆盖层若影响性能，可后续改为静态 atlas + 预建顶点缓冲；当前每帧重建顶点缓冲仅数十 quad，开销可忽略。
- 字体仅覆盖调试所需字形；如需更多文字再扩展 const 表。
