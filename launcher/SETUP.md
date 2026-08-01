# 环境配置指南

本项目已用 **Tauri 2 + Vue 3 + Vite + TypeScript + pnpm** 脚手架生成完毕,前端依赖已安装并能正常 `pnpm build`。

但要在 **Android** 上运行/打包,还差以下工具链(本机尚未安装)。请按顺序完成。

---

## 当前已就绪 ✅

| 项 | 状态 |
|---|---|
| Node.js + npm | ✅ |
| pnpm 11.11.0 | ✅ 已全局安装 |
| Android SDK | ✅ `C:\Users\jasonngu\AppData\Local\Android\Sdk` |
| Android Platforms | ✅ android-36.1 |
| Android Build-tools | ✅ 36.1.0, 37.0.0 |
| `ANDROID_HOME` / `ANDROID_SDK_ROOT` 环境变量 | ✅ 已配置(用户级) |
| 前端依赖 + 前端构建 | ✅ `pnpm build` 通过 |

## 待安装 ❌

| 项 | 用途 |
|---|---|
| Rust toolchain | 编译 Rust 后端(必需) |
| JDK 17 | Gradle / Android 构建(必需) |
| Android NDK | Rust 交叉编译到 Android(必需) |
| Android cmdline-tools | 安装 NDK 的命令行工具(必需) |
| Rust Android targets | `aarch64-linux-android` 等(必需) |

---

## 步骤 1:安装 Rust

打开 https://rustup.rs/ 下载 `rustup-init.exe` 并运行,或用 winget:

```powershell
winget install Rustlang.Rustup
```

安装完成后**新开一个终端**(让 PATH 生效),验证:

```powershell
rustc --version
cargo --version
```

## 步骤 2:安装 Tauri CLI

```powershell
# 方式 A:用 cargo 安装(推荐,与 Rust 版本匹配)
cargo install tauri-cli --version "^2.0"

# 方式 B:项目已自带 @tauri-apps/cli,可直接用 pnpm tauri,无需单独装
```

本项目已通过 `devDependencies` 里的 `@tauri-apps/cli@^2` 提供 `pnpm tauri` 命令,通常无需单独安装 cargo 版本。

## 步骤 3:安装 JDK 17

推荐 Microsoft OpenJDK 17 或 Eclipse Temurin 17:

```powershell
winget install Microsoft.OpenJDK.17
```

安装后**设置环境变量**(假设装在默认路径,实际路径以安装结果为准):

```powershell
# 以管理员 PowerShell 运行,路径换成你实际的 JDK 安装目录
[Environment]::SetEnvironmentVariable("JAVA_HOME", "C:\Program Files\Microsoft\jdk-17.0.x.x-hotspot", "User")
$p = [Environment]::GetEnvironmentVariable("PATH", "User")
if ($p -notlike "*%JAVA_HOME%\bin*") {
    [Environment]::SetEnvironmentVariable("PATH", "$p;%JAVA_HOME%\bin", "User")
}
```

验证(**新终端**):

```powershell
java -version
# 应显示 openjdk version "17.x.x"
```

## 步骤 4:安装 Android cmdline-tools + NDK

### 4.1 装 cmdline-tools

你的 SDK 目前**没有** `cmdline-tools` 目录,需要先装。

打开 Android Studio → Settings → Languages & Frameworks → Android SDK → SDK Tools 勾选:
- ✅ Android SDK Command-line Tools (latest)

或者用已下载的 commandlinetools:https://developer.android.com/studio#command-line-tools-only

解压到 `C:\Users\jasonngu\AppData\Local\Android\Sdk\cmdline-tools\latest\`(注意目录名必须是 `latest`)。

### 4.2 装 NDK

Tauri 2 推荐 **NDK 27**(与最新 Rust android targets 兼容性最好)。

用 sdkmanager(装好 cmdline-tools 后):

```powershell
# 接受许可
sdkmanager --licenses

# 安装 NDK 27
sdkmanager "ndk;27.2.12479018"
```

或 Android Studio → SDK Tools → 勾选 "Show Package Details" → NDK (Side by side) → 勾选 `27.2.12479018`。

### 4.3 设置 NDK_HOME 环境变量

```powershell
# 管理员 PowerShell,版本号换成你实际装的
[Environment]::SetEnvironmentVariable("NDK_HOME", "C:\Users\jasonngu\AppData\Local\Android\Sdk\ndk\27.2.12479018", "User")
```

验证(**新终端**):

```powershell
echo $env:NDK_HOME
# 应指向 ndk\<版本> 目录
```

---

## 步骤 5:添加 Rust Android targets

```powershell
rustup target add aarch64-linux-android armv7-linux-androideabi i686-linux-android x86_64-linux-android
```

- `aarch64-linux-android`:64 位 ARM(绝大多数现代手机,必装)
- `armv7-linux-androideabi`:32 位 ARM(老设备)
- `i686-linux-android` / `x86_64-linux-android`:模拟器用

## 步骤 6:初始化 Android 项目

进入项目目录,首次运行会生成 `src-tauri/gen/android/` 目录:

```powershell
cd F:\learn\rust\tauri-android-app
pnpm tauri android init
```

如果报错,常见原因:
- `NDK_HOME` 未设或路径不对
- `JAVA_HOME` 未设或不是 JDK 17
- Rust targets 未装

## 步骤 7:运行 🚀

### 连接设备/模拟器

```powershell
adb devices
# 确认有设备或模拟器在线
```

### Android 开发模式

```powershell
pnpm tauri android dev
```

首次会编译 Rust 到 Android target,耗时较长(10-30 分钟)。

### Android 打包

```powershell
# Debug APK
pnpm tauri android build --debug

# Release AAB(上传 Google Play)
pnpm tauri android build
```

产物在 `src-tauri/gen/android/app/build/outputs/apk/` 或 `bundle/`。

---

## 常用命令速查

| 命令 | 作用 |
|---|---|
| `pnpm dev` | 仅启动前端(Vite dev server, http://localhost:1420) |
| `pnpm build` | 前端生产构建(已验证可跑) |
| `pnpm tauri dev` | 桌面端开发(需 Rust) |
| `pnpm tauri android dev` | Android 开发(需完整工具链) |
| `pnpm tauri android build` | Android 打包 |
| `pnpm tauri android init` | 生成 Android 工程(首次) |

## 参考文档

- Tauri Android 前置条件:https://tauri.app/start/prerequisites/
- Tauri Android 开发:https://tauri.app/start/run-android/
- Tauri 配置参考:https://tauri.app/reference/config/
