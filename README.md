# PrismaRev

从零实现的 Rust 游戏引擎：**Vulkan** 渲染，**Android 优先**（TBDR），一套渲染管线同时覆盖桌面与移动端。架构采用**数据导向的 ECS** 而非传统 OOP——实体是整数句柄，组件是纯数据，系统是查询数据切片的函数，与 Rust 的所有权模型天然契合。

> **设计目标与架构原则见 [`docs/DESIGN.md`](docs/DESIGN.md)** —— 移动端 TBDR 优先、全平台统一的模块化渲染管线、运行时自动探测 Vulkan 能力与扩展。新增特性前请先对照。

## 当前能力

- **Vulkan 渲染管线**：RenderGraph + pass 节点架构（`RenderPassNode`），`GraphRenderer` 驱动；GBuffer / Shadow / Lighting / Post 等 pass 组合，能力探测自动升降级。
- **PBR + IBL**：HDR 环境光、漫反射辐照度 + 预滤波镜面反射 cubemap。
- **Bindless 纹理表**：set 0 = frame UBO/materials/lights，set 1 = bindless 纹理，set 2 = IBL，set 3 = 阴影贴图。
- **调试与编辑器**：egui inspector（实体/渲染设置/烘焙控制），屏幕空间调试视图模式（bitmap 字体 HUD + 命中测试），世界空间 XYZ gizmo。
- **全链路**：acquire → record → submit → present 在桌面与 Android 端到端跑通，验证层在 debug 构建下启用。
- **后台线程架构**：IO / 音频解码 / 物理三线程框架（flume 消息传递）。
- **资源管线**：`prism-asset` 统一资产管线（Import → Cook → Package → Runtime），glTF 2.0 加载、纹理/mesh 烘焙、CookProfile 平台配置、`.pak` 打包与热重载。
- **离线构建**：`prism-build-pipeline` 提供 GI 烘焙与**参数化超高落差高度图生成器**（热力 + 水力侵蚀，−11 000 m ~ +8 850 m，CPU 多线程 / 可扩展 GPU compute）。

## 架构

```
PrismaRev/
├── crates/
│   ├── prism-ecs/             # ECS 核心（Entity / Component / World / Query）
│   ├── prism-render/          # Vulkan 后端（context、swapchain、RenderGraph passes、IBL、bindless）
│   ├── prism-engine/          # 应用层（AppDriver、winit 事件循环、主循环编排）
│   ├── prism-platform/        # 平台抽象（窗口系统、Vulkan surface、输入路由）
│   ├── prism-app/             # 平台应用层（事件循环 / 窗口 / 帧编排；Android `android_main` 入口）
│   ├── prism-editor/          # egui 编辑器 UI（inspector、调试、渲染设置、烘焙）
│   ├── prism-editor-tool/     # 编辑器工具（高度图生成器、地形工具等）
│   ├── prism-build-pipeline/  # 离线构建管线（GI 烘焙、资产烹饪、高度图生成器 CLI）
│   ├── prism-audio/           # 音频子系统（Firewheel）
│   ├── prism-asset/           # 统一资产管线（core / runtime / cooker / package / db / importer / CLI）
│   ├── prism-launcher/        # Tauri launcher（桌面壳 + Android APK 打包，独立 workspace）
│   └── xtask/                 # 构建工具：Slang reflection → Rust 绑定代码生成（桌面/CI 专用，排除在默认 workspace 外）
├── assets/
│   ├── shaders/               # Slang 源码（slang/）、编译产物 .spv + reflection/*.json
│   ├── scenes/                # 场景资产（glTF 等）
│   └── ...
├── docs/DESIGN.md             # 权威设计蓝图
├── scripts/                   # run.ps1 / build-android.ps1
└── Cargo.toml                 # workspace（xtask 与 prism-launcher 排除在外）
```

## 坐标约定

所有渲染数学遵循一套严格约定。**偏离即 bug**——本项目绝大多数朝向/手性问题都源于混用。

### 世界 / 视图空间（右手系）
- 原点：场景原点 `(0, 0, 0)`；轨道相机绕 `OrbitCamera::target` 旋转。
- 轴向：**+X = 右，+Y = 上，+Z = 朝向观察者**（相机看向 −Z）。
- `OrbitCamera::view()` 构建右手系视图矩阵（`right = forward × up`，`up = +Y`）。

### 裁剪空间
- 列主序 `mat4` 对齐 GLSL（`m[col][row]`；Rust `[[f32; 4]; 4]` 以 `[col][row]` 索引）。
- 变换链：`clip = projection * view * model`。
- 透视投影应用 **Vulkan y-flip**：`p[1][1] = -inv_tan(fovy/2)`。这是 Vulkan 的正确写法（OpenGL 用 `+inv_tan`）。深度映射到 Vulkan 范围 **[0, 1]**（而非 [−1, 1]）。

### NDC（透视除法 `xyz / w` 之后）
- **x ∈ [−1, 1]**：−1 = 左，+1 = 右。
- **y ∈ [−1, 1]**：**−1 = 上，+1 = 下**。Vulkan 相对 OpenGL 翻转 y（OpenGL 中 +1 为上）。
- **z ∈ [0, 1]**：0 = 近平面，1 = 远平面（Vulkan 深度范围）。

### 帧缓冲
- **左上角原点**；x 向右递增，**y 向下递增**。
- NDC `(−1, −1)` → 左上角；NDC `(+1, +1)` → 右下角。

### 屏幕 / 指针（winit、Android `MotionEvent`）
- **左上角原点**；x 向右递增，**y 向下递增**——与帧缓冲内存布局一致。
- 指针/触摸坐标以该空间报告（用户所见，合成器之后）。
- 合成器可能对整个帧缓冲施加 `pre_transform`（如横屏 Android 应用的 `ROTATE_90`）。为保持正向，3D 内容**和** 2D 覆盖层在裁剪空间按 `surface_rotation = pre_transform⁻¹` 预旋转（`Renderer::orientation()`）。HUD 矩形直接定义在此左上角/y 向下屏幕空间，命中测试无需额外旋转。

### 参考：gizmo 轴向
- `Gizmo` 绘制的世界轴：**X = 红，Y = 绿，Z = 蓝**——+Y 指向上方的右手三轴。

### prism-ecs
- `Entity { id, generation }` —— 轻量句柄；回收时 generation 递增，过期句柄可区分。
- `Component` —— 对任意 `'static` 数据 blanket 实现；无需 derive 样板。
- `World` —— 按 `TypeId` 键控的类型擦除稀疏组件池；`spawn`/`insert`/`get`/`get_mut`/`remove`/`query`/`query_mut`。

### prism-render（ash 0.38）
- `VulkanContext` —— instance（验证层 + debug messenger）、物理设备选择、逻辑设备、图形队列。
- `Swapchain` —— surface、swapchain + image views、帧同步：`MAX_FRAMES_IN_FLIGHT` acquire 信号量（旋转、fence 守护）、每 swapchain 镜像一个 render-finished 信号量（按 image index 索引）、`image_in_flight` fence 追踪防止复用镜像的命令缓冲被覆盖。
- `GraphRenderer` —— RenderGraph 驱动（`scene_pass` 直接渲染到 swapchain，pass 按 `RenderPassNode` 的 setup/execute 声明资源与记录命令）。

### prism-engine / prism-app
- `AppDriver` trait + `Platform::run()`；winit `ApplicationHandler` 经 `WinitBridge` 翻译为 `AppDriver` 事件；`WindowSubsystem` 持有 `Arc<Window>` + `InputManager`。
- 后台线程：IO / 音频解码 / 物理三线程，flume 通道通信。

## 构建与运行

要求：Rust stable（仓库经 `rust-toolchain.toml` 固定）、支持 Vulkan 的 GPU、Vulkan loader。

### 桌面

```sh
cargo check --workspace   # 全 workspace 编译检查
cargo build -p prism-ecs -p prism-render -p prism-engine -p prism-platform
cargo test  -p prism-ecs -p prism-render -p prism-engine -p prism-platform
```

当前桌面以库形式构建/测试（渲染、ECS、平台层均作为库链接）。可执行入口正随 `crates/prism-launcher`（Tauri）迁移；验证层在 debug 构建下启用，`RUST_LOG=info`（或 `debug`）查看诊断。`scripts/run.ps1` 提供 Windows 一键脚本（自动重新编译 Slang 着色器）。

### Android

- `prism-app` 以 `cdylib` 暴露 `android_main` JNI 入口；`.cargo/config.toml` 将链接器指向 NDK 的 clang wrapper。
- `crates/prism-launcher`（Tauri）负责 APK 打包：`pnpm tauri android build`（`build_debug.sh` / `build_release.sh`）。
- 无 slangc 环境：Android 直接携带预编译 `.spv`，着色器编译只在桌面/CI 进行。

## 着色器管线

- 着色器用 **Slang** 编写（`assets/shaders/slang/*.slang`），入口 `vertexMain` / `fragmentMain`。
- `bash assets/shaders/compile.sh` 调用 slangc 编译为 `.spv` + reflection JSON。
- `.spv` **提交进仓库**并被 `include_bytes!` 使用——编辑 `.slang` 后必须重编译并提交，否则运行的是陈旧 SPIR-V。
- reflection JSON 驱动 `xtask` 的 `shader-bindgen`，生成 `crates/prism-render/src/shader_bindings.rs`：入口名常量、descriptor set/binding 常量、`#[repr(C)]` push-constant 结构。
- **CI 漂移守卫**：重新生成的绑定/SPIR-V 若与提交版本不一致则 CI 失败（`shaders` job）。

## 资源管线（prism-asset）

统一资产管线，单 crate 多模块：

```
[Source] → Importer → [Intermediate] → Cooker → [Cooked] → Package → [.pak] → Runtime
```

- **core**：`AssetData` / `AssetHandle` / `AssetGuid` / `AssetId`，宏定义资产类型。
- **importer**：glTF 2.0 场景导入等。
- **cooker**：平台化烘焙（CookProfile 继承 + CLI 覆盖，内置 base/desktop/android/ios/embedded）。
- **package / db**：`.pak` 打包与资产数据库。
- **runtime**：`ResourceManager` 读取 `.pak` 的消费侧 API（零依赖编辑器 crate），支持热重载。
- **CLI**：`prism-asset-cli` 子命令驱动导入/烘焙/打包。

## 高度图生成器（prism-build-pipeline）

参数化超高落差拟真侵蚀生成器（Spec v1.0）：

- 动态范围 −11 000 m ~ +8 850 m（落差 ≈ 20 km），`f64` 存储。
- **热力侵蚀**：休止角材料滑动，双缓冲 + rayon 并行，削平极端陡坡。
- **水力侵蚀**：粒子法（`Particle` 携带水量/泥沙/速度），rayon 分块并行 + 每线程局部缓冲合并；速度钳制、沉积容量上限、海平面以下侵蚀倍率控制稳定性。
- 全参数化 `ErosionParams`，支持 JSON/TOML 预设。
- 以库 API 形式暴露（`prism_build_pipeline::heightmap`），供 CLI / 编辑器工具调用。

## 测试

```sh
cargo test -p prism-ecs      # ECS 单元测试（spawn/despawn/query/generation）
cargo test -p prism-asset    # 资产管线测试（导入/烘焙/打包）
cargo test -p prism-build-pipeline  # 侵蚀算法测试（陡坡变平、平坦地形不变）
cargo clippy --all-targets   # 零警告门禁
```

## CI

`.github/workflows/ci.yml`，推送 `master` / `dev` 或 PR 触发：

| Job | 内容 |
|-----|------|
| `lint` | `cargo fmt --check` + clippy（桌面 crates + xtask，零警告） |
| `desktop` | 桌面 crates 的 `cargo build --locked` + `cargo test --locked` |
| `shaders` | slangc 编译 `.slang` → `.spv` + reflection，重新生成 Rust 绑定，**漂移守卫**（提交产物与新鲜编译不一致即失败） |

## License

MIT OR Apache-2.0
