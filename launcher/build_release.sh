#!/usr/bin/env bash
# 打 arm64-v8a 的 release APK（产物在 launcher/src-tauri/gen/android/app/build/outputs/）。
#
# 用法（Git Bash / WSL / Linux）：bash launcher/build_release.sh
# 仓库按 core.autocrlf 检出，Windows 上直接 ./build_release.sh 会因 shebang 带 CR 失败。
#
# 依赖环境变量（见 launcher/SETUP.md），脚本本身不指向任何一台机器的路径：
#   JAVA_HOME          JDK 17（Tauri/Gradle 要求）
#   ANDROID_HOME       Android SDK
#   ANDROID_NDK_HOME   或 NDK_HOME，NDK 安装目录
set -uo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

: "${JAVA_HOME:?set JAVA_HOME to a JDK 17 install}"
: "${ANDROID_HOME:?set ANDROID_HOME to the Android SDK}"

ndk="${ANDROID_NDK_HOME:-${NDK_HOME:-}}"
if [ -z "$ndk" ]; then
    echo "set ANDROID_NDK_HOME (or NDK_HOME) to an NDK install" >&2
    exit 1
fi
export ANDROID_HOME
export ANDROID_NDK_HOME="$ndk"
export NDK_HOME="$ndk"

# Windows 上 Gradle 默认读不到 Windows 证书库；需要时用 JAVA_TOOL_OPTIONS 覆盖。
export JAVA_TOOL_OPTIONS="${JAVA_TOOL_OPTIONS:--Djavax.net.ssl.trustStoreType=Windows-ROOT -Dcom.sun.net.ssl.checkRevocation=false -Dcom.sun.security.disableRevocation=true}"

cd "$repo_root/launcher"
log="$repo_root/launcher/tauri_build_release.log"

status=0
pnpm tauri android build --target aarch64 --apk --split-per-abi >"$log" 2>&1 || status=$?
echo "BUILD_EXIT=$status"
echo "log: $log"
exit "$status"
