# PrismaRev 架构审计与改进 Plan

> 生成日期：2026-07-12
> 背景：对全仓库（prism-ecs / prism-render / prism-engine / prism-android / shaders）做的现代化引擎/游戏架构审计。
> 已排除项：**`Renderer::color_format` 字段未初始化导致无法编译** 已在本次会话修复（初始化时存格式、`render_pass_color_format()` 直接返回，安卓 init/resume 格式判断现已一致）。本 plan 不含该项。
> 执行方式：新会话按「优先级 + 执行顺序」逐条处理。每条都自带上下文，可直接交给实现 agent。

---

## 0. 横切项：补最小 CI（最高优先级，防止再次回归）

当前没有 CI，`cargo check` 失败（缺字段）长期未被发现。

- **改动**：在仓库加 `.github/workflows/ci.yml`，步骤为 `cargo check --workspace` → `cargo clippy --all-targets -- -D warnings` → `cargo test --workspace`。
- **验收**：push 后 CI 全绿；之后任何破坏编译/带 warning 的提交会被拦下。

---

## 1. 高优先级（核心架构债）

### 1.1 ECS 存储不是 data-oriented
- **问题**：`prism-ecs/src/lib.rs:353` `ComponentPool<T>` 内部是 `HashMap<u32, Box<dyn Any>>`。每个组件单独堆分配 + 类型擦除，无缓存局部性、每次访问都要 `downcast`。README 自称 "data-oriented"，实现却是装箱 trait-object 稀疏 map——DOD 的反面。
- **现代做法**：archetype（Bevy/legion 风格）或真正的 sparse-set，组件以 `Vec<T>` 连续列（SoA）存储。
- **建议改动**：把 `ComponentPool` 改为「稀疏索引 + 稠密 `Vec<T>` 列」的 sparse-set（`dense: Vec<T>` + `sparse: Vec<Option<usize>>` 或 `indices`）。`ErasedPool` 的 `remove` 仍需类型擦除 drop，可用 `dyn Any` 仅保留在 drop 路径，查询路径走具体 `Vec<T>`。
- **验收**：`cargo test -p prism-ecs` 全过；组件在内存中连续；`query` 不再做 `downcast` 热路径转换。

### 1.2 没有真正的查询系统
- **问题**：`query2` / `query3` / `query2_mut`（`lib.rs:200-299`）是写死元数的 join，且每次调用都 `collect()` 成 `Vec`（渲染每帧分配一次）。无泛型 `Query<(&A, &mut B)>` builder、无编译期元数、无零分配迭代器。
- **现代做法**：泛型查询 builder，返回惰性迭代器，按 archetype/sparse-set 求交集，不分配。
- **建议改动**：提供 `world.query::<(Entity, &A, &mut B)>()` 形式的泛型查询（可用宏或 trait 实现多 arity），返回迭代器而非 `Vec`。`render_system`（`prism-engine/src/render_system.rs:132`）改为惰性迭代，去掉每帧 `Vec` 分配。
- **验收**：渲染路径无每帧堆分配（可用 perf/alloc 计数或人工确认）；多组件查询可任意组合。

### 1.3 `query`/`query_mut` 每次克隆 generation 向量
- **问题**：`lib.rs:173,186` `let generation_for = self.entities.clone();` 每次查询整份克隆 `Vec<u32>`。
- **建议改动**：随 1.1/1.2 的存储重构一并消除（generation 校验改为迭代时按 id 查 `entities[id]`，或把 entity 的 generation 直接编码进 dense 索引）。
- **验收**：查询热路径无 `Vec` 克隆。

---

## 2. 中优先级（设计异味 / 可维护性）

### 2.1 swapchain 重建逻辑复制 3 份
- **问题**：「重建 swapchain + 重建 depth images + 重建 framebuffers」在 `renderer.rs` 的 `begin_frame` out-of-date 分支（~584-606）、`end_frame` out-of-date 分支（~834-857）、`recreate_swapchain`（~399-428）各写一遍。
- **建议改动**：抽一个 `fn rebuild_swapchain_dependent_resources(&mut self)` 统一处理 depth + framebuffer 重建，三处调用它。
- **验收**：三处逻辑合并为单一实现；行为不变（可对比重建前后渲染结果）。

### 2.2 两个各自为政的 frames-in-flight 常量
- **问题**：`swapchain.rs:26` `MAX_FRAMES_IN_FLIGHT` 与 `renderer.rs:34` `FRAMES_IN_FLIGHT` 都是 2，需手动同步。
- **建议改动**：在 `prism-render` 内定义单一 `pub(crate) const FRAMES_IN_FLIGHT: usize = 2;`，两处共用。
- **验收**：常量只定义一次。

### 2.3 Mesh 注册表反模式
- **问题**：`MeshHandle(usize)` 只是 `App.meshes: Vec<Mesh>` 下标（`prism-engine/src/app.rs:127,221`），mesh 活在 ECS 之外，耦合渲染与 App、无法去重共享、生命周期不归 ECS 管。
- **现代做法**：`MeshManager` / `Assets<Mesh>` 资源，或把 GPU buffer 句柄直接放进组件。
- **建议改动**：引入 `MeshManager` 资源（或 `World` resource），`MeshHandle` 改为引用管理器内的句柄；`render_system` 从资源取 mesh 而非外部 `Vec`。
- **验收**：`App` 不再持有 `Vec<Mesh>`；渲染只依赖 ECS + 资源。

### 2.4 调试帧抓取写进生产热路径
- **问题**：`request_capture` / `take_capture_data` / `save_bgra_as_ppm` / `insert_capture_readback`（`renderer.rs:236-302, 795-1002`）在 `Renderer`/`App` 内；`App::render_one_frame`（`app.rs:391-397`）第 3 帧无条件请求抓帧。
- **建议改动**：把抓帧逻辑移到独立 `debug_capture` 模块或 `#[cfg(feature = "capture")]` 后；从主循环移除无条件抓帧。
- **验收**：默认构建不含抓帧热路径；需要时通过 feature/显式调用开启。

### 2.5 每帧 `info!` 刷屏
- **问题**：`render_system.rs:115` 每帧打 `display_aspect`；`renderer.rs:371-382` `orientation()` 每帧打整段。
- **建议改动**：降为 `debug!`/`trace!`，或只在格式/方向变化时打一次。
- **验收**：正常运行日志不再每帧刷 aspect/orientation。

### 2.6 `Renderer` 是 1045 行巨石
- **问题**：同时持有 context / swapchain / render pass / framebuffers / depth / pipeline / descriptors / UBO / shader modules / command pool / command buffers / capture。
- **建议改动**：按职责拆分（Device/Instance、Swapchain、PipelineCache、FrameGraph/ResourceManager）。M2 阶段可只做轻量拆分（如把 capture 拆出、把 swapchain 资源重建抽 helper），不必一步到位。
- **验收**：单文件显著缩短，职责边界清晰。

### 2.7 Vulkan 句柄无 RAII
- **问题**：每种资源都有手动 `unsafe fn destroy` + 只 `log::warn!("dropped without explicit destroy")` 的 `Drop`（`pipeline.rs:146`、`render_pass.rs:100`、`descriptor.rs:49/103/191` 等）。drop 漏调就静默泄漏。
- **现代做法**：句柄包真正的 `Drop`（或上 `vk-mem` / `gpu-alloc` / `raw-window-handle` 风格 RAII）。
- **建议改动**：为资源类型实现真实 `Drop`（调用对应 `vkDestroy*`），移除 warn-only Drop；或引入轻量 RAII 包装宏。
- **验收**：资源在 owner drop 时自动释放，无 warn 日志。

### 2.8 `upload_to_buffer` 用 `queue_wait_idle`
- **问题**：`buffer.rs:141` 每次建 mesh 整队列 stall。
- **建议改动**：改用 fence 等待该次提交完成，而非 `queue_wait_idle`（整队列）。
- **验收**：mesh 上传不再阻塞整队列（行为等价，延迟更低）。

---

## 3. 低优先级（代码质量）

- **3.1 死字段 `_enabled_layer_names`**（`context.rs:42,93`）永远为空，注释误导。→ 删除或真正填充。
- **3.2 `OrbitCamera::new(_aspect)` 忽略 aspect**（`camera.rs:13`）→ 存 `aspect` 字段，resize 时更新而非重建 camera。
- **3.3 `present_mode` 硬编码 `FIFO`**（`swapchain.rs:332`）→ 暴露选项（mailbox 等低延迟）。
- **3.4 测试场景硬编码在 `App::ensure_window`**（`app.rs:194-225`）→ 抽成数据驱动的场景描述（M2 可接受，记一笔）。
- **3.5 `query2_mut` 用 `unsafe` 裸指针双可变借用**（`lib.rs:282-284`）→ 随 1.1/1.2 存储重构自然消除。
- **3.6 ECS 缺 CommandBuffer / 延迟变更** → 迭代中无法安全 insert/despawn；补标准 ECS 能力。
- **3.7 `Component` 对一切 `'static` blanket impl**（`lib.rs:44-47`）→ 可接受，仅记录。

---

## 4. 执行顺序建议

1. **第 0 项 CI**（先挡回归）
2. **1.1 → 1.2 → 1.3**（ECS 核心，地基，其他项多依赖它）
3. **2.1 / 2.2 / 2.5 / 2.8**（低风险、高收益、可独立提交）
4. **2.3 / 2.4 / 2.6 / 2.7**（中等重构，单独 PR）
5. **第 3 项**（顺手清理）

每条改完跑 `cargo clippy --all-targets` 与 `cargo test --workspace` 确认无回归。
