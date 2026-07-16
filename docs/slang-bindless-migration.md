# Slang 工具链 + Bindless 迁移指南

本文档记录 PrismaRev 引入 **Slang shader 工具链** 和 **Bindless（descriptor-indexing）纹理表** 的落地情况，以及需要在**有 slangc 的桌面/CI 机器**上完成的收尾步骤。

> 背景：改动在 Termux/aarch64 上开发，那里 `cargo check`/`cargo test -p prism-render` 全绿，但官方 `slangc` 是 glibc 二进制、无法在 Termux 原生运行。因此 shader 编译（`.slang -> .spv` + 反射 JSON）必须在桌面/CI 完成，`.spv` 与生成的 Rust 绑定 checked-in 后手机端照常构建。

---

## 一、Slang 工具链

### 新增文件
```
shaders/slang/common.slang     # 共享类型 (FrameUBO / 顶点流 / varyings)
shaders/slang/mesh.slang       # = mesh.vert + mesh.frag (Blinn-Phong)
shaders/slang/pbr.slang        # = pbr.frag (Cook-Torrance + IBL)
shaders/slang/gizmo.slang      # = gizmo.vert + gizmo.frag
shaders/slang/overlay.slang    # = overlay.vert + overlay.frag
shaders/slang/bindless.slang   # PBR 的 bindless 变体 (见第二节)
shaders/compile.sh             # slangc 编译脚本 (.spv + reflection JSON)
shaders/reflection/*.json      # 反射 JSON (桌面 slangc 生成；仓库内是占位样例)
xtask/                         # 反射 JSON -> Rust 绑定 codegen 工具
crates/prism-render/src/shader_bindings.rs  # @generated 绑定常量
```

原有 GLSL（`shaders/*.vert/*.frag`）与其 `.spv` **保留共存**，现有渲染不受影响。

### 绑定约定（`[[vk::binding(binding, set)]]`）
| shader | set | binding | 资源 | 类型 |
|--------|-----|---------|------|------|
| mesh/pbr | 0 | 0 | FrameUBO | UNIFORM_BUFFER |
| pbr | 1 | 0 | envCube | COMBINED_IMAGE_SAMPLER (samplerCube) |
| overlay | 0 | 0 | fontTex | COMBINED_IMAGE_SAMPLER (sampler2D) |
| bindless | 0 | 0 | FrameUBO | UNIFORM_BUFFER |
| bindless | 2 | 0 | bindlessCubes[] | 运行时数组 COMBINED_IMAGE_SAMPLER |

Push constant 大小：mesh=64, pbr=92, gizmo=64, bindless=96。**与 Rust `#[repr(C)]` 镜像字节对齐**（有单测校验 offset）。

> ⚠️ combined image sampler：Slang 用 `Sampler2D`/`SamplerCube`（不是分离的 `Texture2D + SamplerState`），以匹配 Rust 侧的 `COMBINED_IMAGE_SAMPLER` 描述符和原 GLSL。

### 桌面/CI 收尾步骤
```bash
# 1) 装 slang (对应你的平台)：
#    https://github.com/shader-slang/slang/releases
#    Termux/aarch64 无法用官方 glibc 版；请在桌面/CI 做这一步。

# 2) 编译 shader -> .spv + 反射 JSON
export SLANGC=/path/to/slangc      # 或让 slangc 在 PATH 上
bash shaders/compile.sh
#   产出 shaders/*.spv 和 shaders/reflection/*.json（覆盖仓库里的占位样例）

# 3) 从真实反射重新生成 Rust 绑定
cd xtask
cargo run --bin shader-bindgen -- ../shaders/reflection ../crates/prism-render/src/shader_bindings.rs

# 4) 校验
cd .. && cargo test -p prism-render
```

> `shaders/reflection/*.json` 目前是**手写占位样例**（结构与 slangc `-reflection-json` 输出一致），让 xtask 和 CI 在没有 slangc 时也能跑通端到端。真实 slangc 产出会覆盖它们；若字段名有出入，调整 `xtask/src/shader_bindgen.rs` 的 serde 结构即可。

### xtask 为什么是独立 crate（不是 build.rs）
- `build.rs` 每次 `cargo build` 都跑 → 会强制依赖 slangc → **手机端构建直接崩**。
- xtask 在 workspace `exclude` 列表里，默认 `cargo check`/`build` **完全不碰它**，生成的 `shader_bindings.rs` checked-in。
- 运行方式：`cd xtask && cargo run --bin shader-bindgen -- <反射目录> <输出.rs>`（不能用 `cargo run -p xtask`，因为它被 exclude 了）。

---

## 二、Bindless（descriptor-indexing）

### 设计
新增 `crates/prism-render/src/bindless.rs`：一个 `BindlessTextureTable` —— 单个大描述符集，含一个**运行时大小的 `COMBINED_IMAGE_SAMPLER` 数组**（默认容量 1024）。Shader 用 `u32` handle 索引（`bindless.slang`），材质/纹理切换不再需要重绑描述符集。

Flags：`PARTIALLY_BOUND | UPDATE_AFTER_BIND | VARIABLE_DESCRIPTOR_COUNT`，配合 `runtime_descriptor_array` + `shader_sampled_image_array_non_uniform_indexing`。

### 改动清单
- `context.rs`：`rt_caps.descriptor_indexing` 为真时，额外开启 5 个 descriptor-indexing 子特性（runtime array / partially bound / update-after-bind / variable count / non-uniform indexing）。
- `renderer.rs`：构造时创建 `BindlessTextureTable`（容量 1024），把 IBL cubemap 注册进去得到 `ibl_bindless_handle`；新增 `bindless()` / `bindless_mut()` / `ibl_bindless_handle()` 访问器。
- `ibl.rs`：新增 `image_view()` / `sampler()` 访问器，供注册使用。
- `pbr_push.rs`：新增 `PbrBindlessPushConstants`（96 字节，多一个 `env_handle: u32`），带 offset 单测。
- `bindless.slang`：PBR 的 bindless 变体，从 set 2 的运行时数组按 `env_handle` 用 `NonUniformResourceIndex` 采样。

### 关键：这是**增量**改造，不是替换
现有每资源独立描述符集（`descriptor.rs` / `ibl.rs` set-1 cubemap / `overlay.rs`）**全部保留可用**。bindless 表是并行新增的基础设施。这样做的原因：renderer.rs 有 995 行，在无法看到渲染结果的环境下把绑定全推倒重写风险极高。

### 桌面/真机上完成 bindless PBR 迁移（需你验证渲染结果）
当前 bindless 表已创建、IBL cubemap 已注册，但 **PBR pipeline 仍走老的 set-1 路径**。要切到 bindless：

1. **编 bindless shader**：`compile.sh` 已包含 `bindless.slang`，产出 `bindless_pbr.frag.spv`（确认脚本里的 entry/stage）。
2. **建 bindless PBR pipeline**：pipeline layout 用 `[descriptor_layout.layout(set0), <未用>(set1 占位或跳过), bindless.layout(set2)]`。
   - 注意：Vulkan set 索引必须连续或用空 layout 填。若想让 bindless 表在 set 2，set 1 需要一个空 descriptor set layout 占位；或把 bindless 表改到 set 1（同时改 `bindless.slang` 的 `[[vk::binding(0,1)]]`）。**推荐后者**（少一个空集）。
3. **push constant** 换成 `PbrBindlessPushConstants`（range size 96），填入 `env_handle = renderer.ibl_bindless_handle().0`。
4. **每帧绑定** bindless set：`cmd_bind_descriptor_sets(..., set_index, &[table.set], &[])`，替代原来绑 `ibl.descriptor_set`。
5. **验证**：桌面跑起来对比 IBL 反射/高光是否与老路径一致；真机（Android Vulkan）确认设备支持这些 descriptor-indexing 子特性（多数新 GPU 支持，老 Mali/Adreno 可能缺 update-after-bind，需要 fallback 到老路径）。

### 移动端注意
- 并非所有 Android GPU 都支持全套 descriptor-indexing。`context.rs` 只在 `descriptor_indexing` cap 为真时开启；建议再加一层 runtime 检查：不支持时 fallback 到现有 set-1 cubemap 路径（老路径已保留，天然可 fallback）。
- `UPDATE_AFTER_BIND` 在部分移动驱动上支持较弱，真机务必验证。

---

## 验证状态（Termux/aarch64）
- `cargo check -p prism-render` ✅
- `cargo test -p prism-render --lib` ✅ 25 passed
- `xtask` 端到端生成 `shader_bindings.rs` ✅（5 个 shader 模块）
- `prism-android` 在非 Android target 上编不过是**既有问题**（android-activity 需 NDK target），与本次改动无关。

## CI（.github/workflows/ci.yml）
5 个 job，全并行（除依赖）：
1. **lint** — fmt + clippy（report-only，因既有代码未格式化；可在仓库统一 `cargo fmt --all` 后收紧为 `-D warnings`）
2. **desktop** — `cargo build` + `cargo test`（prism-ecs/render/engine/prismarev）
3. **shaders** — slangc 编 `.spv` + 反射，xtask 重生成绑定，**drift guard**（spv/绑定与源码不符则 fail）
4. **android-rust** — cargo-ndk 交叉编 `prism-android` arm64 cdylib（CI 上 `aarch64-linux-android` target）
5. **android-apk** — Gradle assembleDebug，产出 debug APK（仅 push 到 master/dev 时跑，避免 PR 烧构建时长）

> 注意：`.spv` 是 `include_bytes!` 编进 Rust 的（见 renderer.rs），所以 shaders job 的 drift guard 是硬保证——谁改了 `.slang` 忘了重编 spv，CI 直接挂。
> prism-engine 经 winit 间接依赖 android-activity，但只在 `aarch64-linux-android` target 才启用；desktop job 在 CI 的 x86_64 runner 上正常。

## 资产
`assets/fbx/` 已拷入小资产：`cornell-box.fbx`、`cube-coord.fbx`、`cornell-box.zip`（含贴图 + coord.fbx）。
`sponza.zip`(110MB)/`sponza-1.zip`(99MB) 因网络对 GitHub 大文件的限制未拷；需要时在桌面/CI 用 `just fetch-res` 等价流程拉取（见 TruvisRenderer `resources.toml`）。
