# 在 aarch64 Linux 上打 Android APK（game-only，无 launcher）

本文记录 **没有 x86_64 机器** 时如何原生打包 PrismaRev 的 Android APK：
aarch64 宿主机（Ubuntu glibc 或 Termux）+ 官方 x86_64 版 NDK +
aarch64 版 build-tools。全流程已在一台 `Ubuntu 26.04 aarch64`（glibc 2.43、
clang 21、JDK 21）上跑通：`libprisma.so` 11.7 MB → APK 21 MB，可直接 `adb install`。

> 和 `launcher/` 那条路的区别：`launcher/src-tauri/gen/android` 打的是
> **Tauri 启动器 + 游戏** 的双 Activity APK（WebView UI + `:game` 进程），
> 构建期必须跑 `pnpm tauri android build`。本文用的是
> `projects/prisma/android/`，只有 `GameActivity` + `libprisma.so` + `assets/game/`，
> **不含 Tauri / WebView / Kotlin**，点图标直接进游戏。

## 1. 为什么不能照搬官方流程

| 组件 | 官方发布形态 | aarch64 主机上 |
|---|---|---|
| NDK `toolchains/llvm/prebuilt/*` | 只有 `linux-x86_64` / `darwin-x86_64` / `windows-x86_64` | `Exec format error` |
| `aapt2` / `aidl` / `zipalign` / `split-select` | 只有 x86_64 ELF | `Exec format error` |
| Gradle / AGP / D8 / R8 / apksigner | JVM | ✅ 原生可跑 |
| Rust `aarch64-linux-android` | rustup target | ✅ 原生可跑 |

关键点：NDK 里**真正与主机架构无关**的部分是 `sysroot/`（bionic 头文件 +
aarch64 stub 库 + `crtbegin_so.o`/`crtend_so.o`）和 `lib/clang/<ver>/lib/linux/`
下的目标 runtime（`libclang_rt.builtins-aarch64-android.a`、`aarch64/libunwind.a`）。
所以只需要换掉「clang 驱动」和四个 build-tools 二进制。

## 2. 一次性环境准备（Ubuntu / Debian）

```bash
sudo apt-get update
sudo apt-get install -y clang lld llvm-21 cmake ninja-build unzip \
                        openjdk-21-jdk-headless curl
```

Termux 对应：`pkg install clang lld llvm cmake ninja openjdk-21 unzip`。

### 2.1 Android SDK（jar 部分跨架构，直接装）

```bash
export ANDROID_HOME=$HOME/android-sdk
mkdir -p "$ANDROID_HOME/cmdline-tools"
curl -L -o /tmp/cmdline.zip \
  https://dl.google.com/android/repository/commandlinetools-linux-14742923_latest.zip
unzip -q /tmp/cmdline.zip -d /tmp/cmdtmp
mv /tmp/cmdtmp/cmdline-tools "$ANDROID_HOME/cmdline-tools/latest"

yes | "$ANDROID_HOME/cmdline-tools/latest/bin/sdkmanager" --licenses
"$ANDROID_HOME/cmdline-tools/latest/bin/sdkmanager" \
  "platforms;android-36" "build-tools;36.0.0"
```

`aapt2` 等四个二进制此时仍是 x86_64，下一步替换。

### 2.2 aarch64 版 build-tools

Google 不发，用社区从 AOSP 交叉编译的版本（只需替换四个原生二进制）：

```bash
curl -fsSL -o /tmp/arm-tools.sh \
  https://raw.githubusercontent.com/Commit451/android-arm-build-tools/main/install.sh
bash /tmp/arm-tools.sh --sdk "$ANDROID_HOME" --version 36.0.0
"$ANDROID_HOME/build-tools/36.0.0/aapt2" version    # 能打印版本即成功
```

> 要求 glibc ≥ 2.36 + GLIBCXX ≥ 3.4.30（Ubuntu 22.04+ / Debian 12+ 满足）。
> Termux 上对应的静态 aarch64 版本见 `Lzhiyong/sdk-tools` 或
> `hamza72x/android-sdk-linux-arm64`。

### 2.3 NDK + 本机可执行的 clang shim

```bash
curl -L -o /tmp/ndk.zip https://dl.google.com/android/repository/android-ndk-r29-linux.zip
unzip -q /tmp/ndk.zip -d "$ANDROID_HOME/ndk-tmp"        # 约 2.4 GB
mkdir -p "$ANDROID_HOME/ndk"
mv "$ANDROID_HOME/ndk-tmp/android-ndk-r29" "$ANDROID_HOME/ndk/29.0.14206865"
export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/29.0.14206865"

bash scripts/make-ndk-host-native.sh "$ANDROID_NDK_HOME"
```

`make-ndk-host-native.sh` 原地改写 NDK：把 `prebuilt/linux-x86_64/bin/*clang*`
换成「宿主 clang + NDK sysroot」的 shim（API level 从 `argv[0]` 的
`aarch64-linux-android31-clang` 推断），并把 `llvm-strip/ar/ranlib/...` 指向宿主 LLVM。
想恢复原状重新解压 NDK zip 即可。

### 2.4 让 AGP 用 SDK 里的 aapt2

AGP 8.x 会从 Maven 拉 `aapt2-*-linux`（x86_64）。`scripts/build-android-game.sh`
已经通过 `-Pandroid.aapt2FromMavenOverride=<SDK>/build-tools/36.0.0/aapt2` 处理；
手跑 `gradlew` 时二选一：

```bash
# 项目内
./gradlew -Pandroid.aapt2FromMavenOverride=$ANDROID_HOME/build-tools/36.0.0/aapt2 :app:assembleDebug
# 或全局（~/.gradle/gradle.properties，这条路径是本机的，不要提交）
echo "android.aapt2FromMavenOverride=$ANDROID_HOME/build-tools/36.0.0/aapt2" >> ~/.gradle/gradle.properties
```

AGP 会打一条 `option setting ... is experimental` 警告，属正常。

## 3. 打 APK

```bash
export ANDROID_HOME=$HOME/android-sdk
export ANDROID_NDK_HOME=$ANDROID_HOME/ndk/29.0.14206865

bash scripts/build-android-game.sh debug       # 日常调试（debug 证书，可直接装）
bash scripts/build-android-game.sh release     # 未签名 release
```

脚本做的事：`cargo build --lib --target aarch64-linux-android` →
`build-scenes` → 复制 `.so` 与 `assets/game/` → `gradlew clean assemble<变体>`。

产物：

```
projects/prisma/android/app/build/outputs/apk/debug/app-debug.apk
  AndroidManifest.xml   package=com.prisma, minSdk 31, targetSdk 36, arm64-v8a only
  lib/arm64-v8a/libprisma.so    11.7 MB（release；debug 带调试符号约 208 MB）
  classes*.dex                  ~6.8 MB（GameActivity + appcompat，无 Tauri）
  assets/game/                  scenes.pak + meta + settings.toml + HDR
```

安装：`adb install -r app-debug.apk`。

## 4. 发布签名

`release` 产物是未签名的（`app-release-unsigned.apk`），签名材料**不入库**：

```bash
$ANDROID_HOME/build-tools/36.0.0/apksigner sign \
  --ks ~/prisma-release.jks --ks-key-alias prisma \
  --out prisma-release.apk app-release-unsigned.apk
```

## 5. 目录与职责

```
projects/prisma/android/          game-only Android 宿主（独立 Gradle 工程，不属任何 cargo workspace）
  app/src/main/AndroidManifest.xml    GameActivity + android.app.lib_name=prisma
  app/src/main/java/com/prisma/MainActivity.java   3 行，extends GameActivity
  app/src/main/res/values/themes.xml  AppCompat 主题（GameActivity 继承 AppCompatActivity）
  app/src/main/jniLibs/               生成物：libprisma.so（gitignore）
  app/src/main/assets/game/           生成物：场景包与设置（gitignore）
scripts/build-android-game.sh     一条命令打包
scripts/make-ndk-host-native.sh   aarch64 主机的 NDK shim 安装器
```

引擎侧不需要任何改动：`prism_app::run_on_android` 把 APK 的 `assets/game/` 解到
`<filesDir>/assets`、切 cwd，再调 `projects/prisma/src/lib.rs` 的 `android_main`。

## 6. 坑速查（都实际踩过）

| 症状 | 原因 | 解决 |
|---|---|---|
| `clang: Exec format error` | NDK prebuilt 是 x86_64 | `make-ndk-host-native.sh` |
| `incompatible constant for this __builtin_neon function` | 用了 NDK 的 `lib/clang/*/include` 头文件配宿主 clang | shim 不传 `-resource-dir`，头文件用宿主 clang 自己的；只从 NDK 借 sysroot + `builtins`/`libunwind` |
| `ld.lld: unable to find library -lunwind` | `libunwind.a` 在 `lib/clang/<v>/lib/linux/aarch64/` | shim 追加该 `-L` |
| `ld.lld: unable to find library -lgcc` | 宿主 clang 默认 `--rtlib=libgcc` | shim 加 `--rtlib=compiler-rt` |
| cc-rs 传的 `--target=aarch64-linux-android` 覆盖 API level | 少 API 后缀 = 按最低 API 链接 | shim 把 `--target=...<api>` 放在最后 |
| `aapt2: cannot execute binary file` / `Daemon startup failed` | Maven 拉的 aapt2 是 x86_64 | `-Pandroid.aapt2FromMavenOverride=<aarch64 aapt2>` |
| `GameActivity` 编译报 `找不到 AppCompatActivity` | games-activity 4.4.0 继承 AppCompatActivity | `androidx.appcompat` + AppCompat 主题 |
| `找不到 style/Theme.AppCompat.NoActionBar.Fullscreen` | 该主题不存在 | 自定义主题 `parent="Theme.AppCompat.NoActionBar"` + `android:windowFullscreen` |
| `Duplicate class kotlin.collections.jdk8.*` | appcompat 带进 kotlin-stdlib-jdk7/jdk8 1.6.21 | `implementation(platform("org.jetbrains.kotlin:kotlin-bom:1.9.25"))` |
| APK 体积突然多出 ~200 MB | 换 `.so` 后增量打包在 zip 里留空洞 | 打包前 `clean`（脚本已默认 clean） |
| `-laaudio not found` | API level < 26 | API level 取 `max(minSdk, 26)`；本项目 minSdk 31 |
