# PrismaRev

从零实现的 Rust 游戏引擎：**Vulkan** 渲染，**Android 优先**（TBDR），一套渲染管线同时覆盖桌面与移动端。架构采用**数据导向的 ECS** 而非传统 OOP——实体是整数句柄，组件是纯数据，系统是查询数据切片的函数，与 Rust 的所有权模型天然契合。

> **设计目标与架构原则见 [`docs/DESIGN.md`](docs/DESIGN.md)** —— 移动端 TBDR 优先、全平台统一的模块化渲染管线、运行时自动探测 Vulkan 能力与扩展。新增特性前请先对照。

## 当前能力

- **Vulkan 渲染管线**：RenderGraph + pass 节点架构（`RenderPassNode`），`GraphRenderer` 驱动；GBuffer / Shadow / Lighting / Post 等 pass 组合，能力探测自动升降级。
- **PBR + IBL**：HDR 环境光、漫反射辐照度 + 预滤波镜面反射 cubemap。
- **Bindless 纹理表**：set 0 = frame UBO/materials/lights，set 1 = bindless 纹理，set 2 = IBL，set 3 = 阴影贴图。
- **编辑器**：Tauri 2 + Vue 3 的独立 Web 编辑器（场景/层级/组件/预制体/资产/Cook/**运行预览**/项目渲染设置/**实时渲染图**），完全独立于用户项目依赖链（见 §编辑器与用户项目分离）。
- **调试**：运行时诊断 HUD（F3：帧率 / 渲染模式 / PBR 通道，无 UI 框架依赖）、屏幕空间调试视图模式（bitmap 字体 HUD + 命中测试）、世界空间 XYZ gizmo。
- **全链路**：acquire → record → submit → present 在桌面与 Android 端到端跑通，验证层在 debug 构建下启用。
- **后台线程架构**：IO / 音频解码 / 物理三线程框架（flume 消息传递）。
- **资源管线**：运行时 `prism-asset`（`.pak` / `ResourceManager` / 热重载）与构建期 `prism-cooker`（glTF 导入、纹理/mesh 烘焙、CookProfile、`prism-cook` CLI）分层；场景 schema 与 RSCN v4 在 `prism-scene`。
- **离线构建**：`prism-build-pipeline` 提供 GI 烘焙与**参数化超高落差高度图生成器**（热力 + 水力侵蚀，−11 000 m ~ +8 850 m，CPU 多线程 / 可扩展 GPU compute）。

## 架构

```
PrismaRev/
├── crates/
│   ├── prism-ecs/             # ECS 核心（Entity / Component / World / Query）
│   ├── prism-reflect/         # 运行时反射 + RTTIBin（UI / render-free）
│   ├── prism-scene/           # 场景 schema：组件、ComponentRegistry、.scene.json、RSCN v4
│   ├── prism-render/          # Vulkan 后端（context、swapchain、RenderGraph passes、IBL、bindless）
│   ├── prism-engine/          # 运行时引擎（场景加载 / 渲染系统 / 资产解析；无 UI 框架）
│   ├── prism-platform/        # 平台抽象（窗口系统、Vulkan surface、输入路由）
│   ├── prism-app/             # 平台应用层（事件循环 / 窗口 / 帧编排；运行时诊断 HUD）
│   ├── prism-editor-api/      # Web 编辑器后端（schema / 文档 IO / 项目设置；Vulkan-free）
│   ├── prism-editor-tool/     # 编辑器工具（高度图生成器、地形工具等）
│   ├── prism-build-pipeline/  # 离线构建管线（GI 烘焙、资产烹饪、高度图生成器 CLI）
│   ├── prism-audio/           # 音频子系统（Firewheel）
│   ├── prism-asset/           # 运行时资产（AssetId/Handle、.pak、ResourceManager、streaming）
│   ├── prism-cooker/          # 构建期资产管线（importer / cooker / db / CookProfile + prism-cook CLI）
│   └── xtask/                 # 构建工具：Slang reflection → Rust 绑定代码生成（桌面/CI 专用，排除在默认 workspace 外）
├── assets/
│   ├── shaders/               # Slang 源码（slang/）、编译产物 .spv（不提交）+ reflection/*.json
│   ├── scenes/                # 场景资产（glTF 等）
│   └── ...
├── launcher/                  # Tauri 桌面壳 + Android APK 打包（独立 workspace）
├── editor/                    # Tauri 2 + Vue 3 Web 编辑器（场景/预制体/资产/Cook/运行预览，独立 workspace）
├── projects/                  # 用户项目（各自独立 workspace）
│   ├── prisma/                # 用户游戏项目（`prisma` + Android `libprisma.so`）
│   └── prisma-sponza/         # Sponza 路径追踪样例项目
├── docs/DESIGN.md             # 权威设计蓝图
├── scripts/                   # run.ps1 / build-android.ps1（高度图分析脚本在 scripts/scratch/）
└── Cargo.toml                 # workspace（crates/xtask 排除在外；launcher/ 与 projects/ 独立）
```

### 编辑器与用户项目分离

用户项目的依赖链**不含任何 UI 框架、编辑器或构建工具**：`prisma → prism-app → prism-engine → {prism-scene, prism-ecs, prism-render, prism-asset}`。

唯一的编辑器是 `editor/`（Tauri 2 + Vue 3），它**不链接引擎**：Rust 侧只依赖
`prism-editor-api`（schema / 文档读写 / 项目设置），Cook 与场景包构建通过子进程调用
`prism-build-pipeline`，运行预览通过 `PRISMREV_LAUNCH_CONFIG` 拉起游戏二进制。

`prism-app` 仍保留中性的 `FrameHook` 与 `prism_render::external_overlay::SwapchainOverlay`
扩展点（供调试宿主 / 未来工具使用）；两者之间的 UI 数据以类型擦除的 `OverlayMessage`
（`Box<dyn FnOnce(&mut dyn SwapchainOverlay) + Send>`）穿过 `RenderShared`。运行时诊断 HUD
（F3）直接复用 ECS UI 文本叠加，不引入任何 UI 框架。

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
- `GraphRenderer` —— RenderGraph 驱动（`forward_pass` 渲染到 HDR 中间目标，`post_pass` 色调映射到 swapchain，pass 按 `RenderPassNode` 的 setup/execute 声明资源与记录命令）。

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

当前根 workspace 无可执行 bin（渲染、ECS、平台层均作为库链接），可执行入口有三个：

- `launcher/`（Tauri 桌面壳，游戏启动器）：`cd launcher && pnpm tauri dev`，点 Play 启动 `prisma`。
- `projects/prisma/`（用户游戏项目）：`cargo run --manifest-path projects/prisma/Cargo.toml`。
- Web 编辑器：`cd editor && pnpm install && pnpm tauri dev`（Tauri 2 + Vue 3；Rust 侧只依赖
  `prism-editor-api`，不链接引擎 / Vulkan）。工具栏的「构建」重新 cook 并打包 `scenes.pak`，
  「运行」以 `LaunchConfig.scene` 启动游戏预览；「渲染设置」写入 `assets/settings.toml` 的 `[render]` 段；
  「渲染图」读取游戏进程写出的 `target/prism-render-graph.json`，用 SVG 实时展示 pass / 资源 DAG（渲染线程
  每 60 帧刷新一次，路径由 `PRISMREV_RENDER_GRAPH_REPORT` 指定）。

启动器通过 `PRISMREV_LAUNCH_CONFIG` 环境变量（桌面）或 `filesDir/launch_config.json`（Android）向游戏
传 JSON 启动参数（`{ "scene": "...", "log_level": "..." }`，见 `crates/prism-engine/src/launch_config.rs`；`scene` 缺省时加载 `assets/scenes.toml` 的第一个场景）。
验证层在 debug 构建下启用，`RUST_LOG=info`（或 `debug`）查看诊断。`scripts/run.ps1` 提供 Windows 一键
脚本（自动重新编译 Slang 着色器后 `cargo build`）。

### Android

- 用户项目 `projects/prisma/` 以 `cdylib` 输出 `libprisma.so` 并暴露 `android_main` JNI 入口
  （内部调用 `prism_app::run_on_android`）；`.cargo/config.toml` 将链接器指向 NDK 的 clang wrapper。
- `run_on_android` 在构造 App 前把 APK `game/` assets 复制到 `<filesDir>/assets` 并切换 cwd，
  因此 `assets/scenes.pak`、`assets/settings.toml` 等路径与桌面完全一致；启动配置从
  `filesDir/launch_config.json` 注入同一个 `PRISMREV_LAUNCH_CONFIG` 环境变量。
- APK 内有两个 Activity：Tauri 启动器 WebView（launcher）与 `com.prisma.MainActivity`
  （`GameActivity`，独立 `:game` 进程，`android.app.lib_name = prisma`）。
- 打包：`scripts/build-android.ps1`（自动探测 NDK 与 API level，先生成 `scenes.pak` 并把运行时
  资源复制进 APK 的 `assets/game/`，再用 `cargo ndk -P <api> -t arm64-v8a` 构建 game 到 `jniLibs/`，
  最后 `pnpm tauri android build`）；也可在 `launcher/` 下单独执行 Tauri 构建。
- 无 slangc 环境：Android 携带预编译 `.spv`（`include_bytes!` 编译期嵌入，从 CI `spirv` artifact 获取），
  着色器编译只在桌面/CI 进行。

## 着色器管线

- 着色器用 **Slang** 编写（`assets/shaders/slang/*.slang`），入口 `vertexMain` / `fragmentMain`。
- `bash assets/shaders/compile.sh` 调用 slangc 编译为 `.spv` + reflection JSON。
- `.spv` **不提交仓库**（gitignore），由 `assets/shaders/compile.sh` 在桌面/CI 生成并被 `include_bytes!` 使用——编辑 `.slang` 后必须重编译，否则运行的是陈旧 SPIR-V。无 slangc 的主机（Termux/Android）从 CI `spirv` artifact 获取预编译 `.spv`。
- reflection JSON 驱动 `crates/xtask` 的 `shader-bindgen`，生成 `crates/prism-render/src/shader_bindings.rs`：入口名常量、descriptor set/binding 常量、`#[repr(C)]` push-constant 结构。
- **CI 漂移守卫**：重新生成的绑定/SPIR-V 若与提交版本不一致则 CI 失败（`shaders` job）。

## 资源管线（prism-asset / prism-scene / prism-cooker）

资产管线按职责分三层（不是旧的 7-crate 拆分）：

```
[Source] → Importer → [Intermediate] → Cooker → [Cooked] → Package → [.pak] → Runtime
             prism-cooker (build host)                         prism-asset (runtime)
             prism-scene: components + schema + RSCN v4
```

- **prism-scene**：场景组件、`ComponentRegistry`、`.scene.json` 作者格式、RSCN v4
  （`type_id + schema_hash` + 按 `TypeDesc` 顺序的位置编码载荷）。cooker 与运行时共用同一 schema。
- **prism-cooker**：glTF/场景/纹理等 importer、平台化 cooker（CookProfile 继承 + CLI 覆盖）、
  资产数据库与 `prism-cook` CLI。
- **prism-asset**：`AssetId`/`Handle`、`.pak` 读写、`ResourceManager`、streaming/hot-reload。
  不包含 importer/cooker/db。
- **CLI**：`prism-cook` 子命令驱动导入/烘焙/打包。场景包的发布入口是
  `prism-build-pipeline build-scenes`：读取 `assets/scenes.toml`，把场景 cook 到
  `target/scene_pack/`，再生成运行时读取的 `assets/scenes.pak` + `.pak.meta.json`。
  单场景可用 `cook`，通用目录打包可用 `pack`。

## 高度图生成器（prism-build-pipeline）

参数化超高落差拟真侵蚀生成器（Spec v1.0）：

- 动态范围 −11 000 m ~ +8 850 m（落差 ≈ 20 km），`f64` 存储。
- **热力侵蚀**：休止角材料滑动，双缓冲 + rayon 并行，削平极端陡坡。
- **水力侵蚀**：粒子法（`Particle` 携带水量/泥沙/速度），rayon 分块并行 + 每线程局部缓冲合并；速度钳制、沉积容量上限、海平面以下侵蚀倍率控制稳定性。
- 全参数化 `ErosionParams`，支持 JSON/TOML 预设。
- 初始地形：seed 化值噪声 + FBM（域扭曲），输出 [0,1] 相对高度。
- **CLI 一键生成**：`prism-build-pipeline heightmap` 子命令，输出 raw f32 高度数据：

```sh
# 512×512、默认高程范围（−11 km ~ +8.85 km）、70% 海洋
cargo run -p prism-build-pipeline -- heightmap --width 512 --height 512 --seed 42

# 自定义：全陆地 + 更高侵蚀强度
cargo run -p prism-build-pipeline -- heightmap --width 1024 --height 1024 \
  --min-elevation 0 --max-elevation 4000 --sea-level 0.1 \
  --iterations 200 --particles 500000 --talus-angle 38

# 常用参数
#   --output      输出 .raw 路径（f32 小端，width×height 采样）
#   --seed        随机种子（同 seed → 同地形）
#   --sea-level   海平面分位 0.0~1.0（0.7 ≈ 地球 71% 海洋，默认）
#   --min/max-elevation  高程范围（米）
#   --iterations / --particles / --talus-angle  侵蚀强度
```

raw 文件可用 Python/Blender/Unity 等直接读取：`np.fromfile(path, dtype='<f4').reshape(h, w)`。

## 测试

```sh
cargo test -p prism-ecs      # ECS 单元测试（spawn/despawn/query/generation）
cargo test -p prism-asset -p prism-scene -p prism-cooker  # 运行时格式 / RSCN / 构建工具
cargo test -p prism-build-pipeline  # 侵蚀算法测试（陡坡变平、平坦地形不变）
cargo clippy --all-targets   # 零警告门禁
python scripts/pr_ready.py   # 提交前自检：改动文件 fmt、受影响 crate 的 clippy/test、bindings 漂移
```

## CI

`.github/workflows/ci.yml`，推送 `master` / `dev` 或 PR 触发：

| Job | 内容 |
|-----|------|
| `lint` | `cargo fmt --check` + clippy（桌面 crates + xtask，零警告） |
| `desktop` | 桌面 crates 的 `cargo build --locked` + `cargo test --locked` |
| `shaders` | slangc 编译 `.slang` → `.spv` + reflection，重新生成 Rust 绑定，**漂移守卫**（提交的绑定与新鲜编译不一致即失败；`.spv` 不提交，每次由编译步骤新鲜生成） |

## 贡献

先读 [`CONTRIBUTING.md`](CONTRIBUTING.md) 和 [`AGENTS.md`](AGENTS.md)。分支 / 提交 / PR / AI 协作的完整
规范在 [`docs/ai-workflow.md`](docs/ai-workflow.md)；提交前跑 `python scripts/pr_ready.py`，把输出贴进 PR。

## License

MIT OR Apache-2.0
