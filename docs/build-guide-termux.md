# PrismaRev 从拉取到编译（Termux / Android 实测流程）

适用于**没有 slangc** 的主机（Termux/Android）。桌面机流程更简单：装 slangc 后
`bash assets/shaders/compile.sh` 再 `cargo build` 即可，跳过第 3 节的 CI 步骤。

## 1. 环境准备

```bash
pkg install rust git gh        # Termux；rust 需 ≥ 1.85（is_multiple_of 等新 API）
gh auth login                  # GitHub CLI 认证（HTTPS token 即可）
```

git 网络坑（Termux IPv6 到 github.com 会超时，IPv4 正常）：

```bash
git config --global http.version HTTP/1.1
```

可选（Android .so 目标，见第 5 节）：

```bash
rustup target add aarch64-linux-android
pkg install cargo-ndk          # 或 cargo install cargo-ndk
```

## 2. 拉取

```bash
git clone https://github.com/Excurs1ons/PrismaRev.git
cd PrismaRev
```

注意：`assets/shaders/*.spv` 是 gitignored 的（不入库），克隆下来**没有** `.spv`，
但 `prism-render` 用 `include_bytes!` 引用它们——直接 `cargo build` 会失败，
必须先走第 3 节生成/获取 `.spv`。

## 3. Shader 流程（无 slangc 主机）

### 3a. 首次：从 CI 取预编译产物

```bash
# 触发一次 CI（推送任意提交，或手动跑 workflow）
git push   # 或 gh workflow run ci.yml

# 等 CI 的 "slang compile" job 完成（约 2-3 分钟）
gh run list --limit 1           # 拿到 run id
gh run watch <run_id>           # 等绿
gh run download <run_id> -n spirv -D ~/spirv

# 部署到项目
cp ~/spirv/*.spv assets/shaders/
cp ~/spirv/reflection/*.json assets/shaders/reflection/
```

> `/tmp` 在 Termux 不可写，下载目录用 `~/` 下路径。
> `spirv` artifact 每次 CI 都会上传（含 .spv + reflection JSON）。

### 3b. 生成 Rust 绑定（bindgen）

```bash
cd crates/xtask
cargo run --bin shader-bindgen -- \
  ../../assets/shaders/reflection ../../crates/prism-render/src/shader_bindings
cd ../..
```

规则：**push constant 结构体一律用生成的 `shader_bindings::模块::Struct`**，
禁止手写。改过 `.slang` 后必须重跑此命令并提交 binding 差异
（CI 的 binding drift job 会校验：不重跑就失败）。

### 3c. 改了 shader 后的更新流程

```bash
git add -A && git commit -m "..." && git push   # 触发 CI 编译新 shader
gh run download <新run_id> -n spirv -D ~/spirv
cp ~/spirv/*.spv assets/shaders/
cp ~/spirv/reflection/*.json assets/shaders/reflection/
cd crates/xtask && cargo run --bin shader-bindgen -- ../../assets/shaders/reflection ../../crates/prism-render/src/shader_bindings && cd ../..
cargo check --workspace
```

## 4. 编译 / 检查 / 测试

```bash
cargo check -p prism-render            # 快速验证渲染层
cargo check --workspace                # 全部 crate
cargo clippy --workspace --all-targets # 必须 0 警告（#![deny(warnings)]）
cargo test --workspace                 # ~400 测试（含 CRT 状态机单测，无 GPU 可跑）
```

注意：

- 根 workspace **没有可运行的 bin**。桌面入口在 `game/`（`prismarev` 二进制，
  独立 workspace：`cd game && cargo run`）；编辑器 demo 是 `cargo run --bin editor_demo`；
  Tauri 壳在 `launcher/`（独立 workspace）。
- `game/`、`launcher/`、`crates/xtask` 不在根 workspace 的默认覆盖内——
  根 workspace 构建不包含它们，需各自 `cd` 进去单独 build/clippy。
- 首次全量构建较久（依赖多，磁盘约 2GB+）；`target/` 被外部清理后需重新构建。
- 本地 Cargo 镜像（可选，加速）：`~/.cargo/config.toml` 用 rsproxy：
  `sparse+https://rsproxy.cn/index/`。

## 5. Android .so（可选）

```bash
# 在 game/ 或根 workspace 编译 aarch64 .so（记忆：cargo-ndk 即可，无需 Gradle 工程）
cargo ndk -t arm64-v8a -o <输出目录> build
```

Tauri launcher 的 `gen/android/` 负责 APK 打包；Termux 上可直接把新 `.so`
替换进已有 APK 并用 `apksigner` 重签（Android 构建详见 README §Android）。

## 常见坑速查

| 症状 | 原因 | 解决 |
|---|---|---|
| git pull/push 卡死超时 | Termux IPv6 连不上 github.com | `git config --global http.version HTTP/1.1` |
| 编译报 include_bytes 找不到 .spv | .spv 不入库，本地没有 | 走第 3 节 CI 下载 |
| CI binding drift job 红 | 改了 shader 没重跑 bindgen | 3b 重生成并提交 |
| `mem::zeroed` 编译/运行报 invalid_value | ash::Device 含函数指针 | 测试构造用 `MaybeUninit` + `#[allow(invalid_value)]` |
| clippy 报 manual is_multiple_of | Rust 新版 API | 用 `.is_multiple_of(n)` |
| 修改 shader 后"什么都没变" | 跑的是旧 .spv | 重新编译/下载 shader 产物 |
