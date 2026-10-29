# PrismaRev Hub

PrismaRev 的桌面启动器，Unity Hub 形态：管理工程列表、新建 / 添加工程，
并用 `editor/` 的编辑器打开它们。

Android 上同一个 Tauri 工程仍是 APK 的启动页：`hub_environment` 返回
`platform: "mobile"`，前端切回「开始游戏」，通过 Kotlin plugin 启动
`GameActivity`。

## 功能

- **工程列表**：路径与最近打开时间保存在应用数据目录的 `hub.json`；
  场景数 / 游戏 crate / 是否丢失每次从磁盘重新读取。
- **新建工程**：在选定父目录下生成 `assets/` 骨架（默认场景 + `scenes.toml`
  + `settings.toml`）。能找到仓库源码时同时生成最小游戏 crate（依赖使用
  绝对路径，因此工程可以放在任意位置）。
- **添加工程**：选择包含 `assets/scenes` 的目录；重复添加是幂等的。
- **打开工程**：以工程目录为 `current_dir` 启动编辑器。编辑器
  （`prism_editor_api::find_project_root`）据此向上查找 `assets/scenes`。
  同一工程不会重复启动，已退出的编辑器会自动从「运行中」状态回收。
- **编辑器页**：显示解析到的编辑器二进制、版本与候选路径；可重新检测。

## 编辑器二进制定位

不查 `PATH`，顺序如下：

1. `PRISMREV_EDITOR_BIN`（显式指定；文件不存在直接报错，不回退）；
2. Hub 可执行文件同目录的 `prisma-editor[.exe]` / `prisma-editor-app[.exe]`
   （Tauri bundle 与 cargo 构建两种命名）；
3. 开发布局 `editor/src-tauri/target/{debug,release}/` 下的同名二进制。

开发时先生成编辑器：

```sh
cd editor
pnpm install
pnpm tauri build        # 或者只 cargo build 生成 target/debug/prisma-editor-app
```

## 开发

```sh
cd launcher
pnpm install
pnpm tauri dev
```

前端检查：

```sh
cd launcher
pnpm build
```

后端检查 / 测试（独立 workspace，不挂根 PrismaRev workspace）：

```sh
cd launcher/src-tauri
cargo test
```
