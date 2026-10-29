#!/usr/bin/env bash
# 让官方 linux-x86_64 版 Android NDK 在 aarch64 主机上可用（Ubuntu glibc / Termux 均可）。
#
#   bash scripts/make-ndk-host-native.sh [NDK 路径]
#
# 背景：Google 只发布 x86_64 主机的 NDK prebuilt 二进制（linux-x86_64 /
# darwin-x86_64 / windows-x86_64）。aarch64 主机上 `toolchains/llvm/prebuilt/
# linux-x86_64/bin/*` 全部是 `Exec format error`。但 NDK 里真正与主机架构无关的
# 部分——sysroot（bionic 头文件 + aarch64 stub 库 + crtbegin/crtend）和 lib/clang/<ver>
# 下的目标架构 runtime（libclang_rt.builtins-aarch64-android.a、aarch64/libunwind.a）
# ——完全可以直接用，只要换一个能在本机跑的 clang 驱动。
#
# 本脚本做两件事（原地改 NDK，可重跑；想恢复就重新解压 NDK zip）：
#   1. prebuilt/<host>/bin/*clang* 全部替换成 shim：读 argv[0] 里的 API level
#      （aarch64-linux-android31-clang），再用宿主 clang 指定 --sysroot / --target /
#      compiler-rt builtins；
#   2. llvm-{ar,ranlib,strip,objcopy,nm,readelf,...} 指向宿主 LLVM（同架构，能处理
#      aarch64 ELF），这样 AGP 的 strip 任务和 cmake 也能跑。
#
# 注意：这里刻意**不**使用 NDK 自带的 clang 头文件（lib/clang/<ver>/include）——它们
# 必须和同一版本的 clang 绑定；宿主 clang 用自己 resource dir 的 arm_neon.h 等，
# 否则会报 "incompatible constant for this __builtin_neon function"。
set -euo pipefail

NDK="${1:-${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-}}}"
if [ -z "$NDK" ] || [ ! -d "$NDK/toolchains/llvm/prebuilt" ]; then
    echo "用法: bash scripts/make-ndk-host-native.sh <NDK 路径>" >&2
    echo "（或设置 ANDROID_NDK_HOME）" >&2
    exit 1
fi
NDK=$(cd "$NDK" && pwd)

case "$(uname -m)" in
    aarch64|arm64) ;;
    *)
        echo "本脚本只用于 aarch64 主机（当前 $(uname -m)）。" >&2
        echo "x86_64 主机直接用官方 NDK 即可，不需要 shim。" >&2
        exit 1
        ;;
esac

HOST_PRE=$(ls -d "$NDK"/toolchains/llvm/prebuilt/*/ 2>/dev/null | head -n1)
BIN="${HOST_PRE%/}/bin"
[ -d "$BIN" ] || { echo "找不到 $BIN" >&2; exit 1; }
sysroot="${HOST_PRE%/}/sysroot"
[ -d "$sysroot/usr/lib/aarch64-linux-android" ] || {
    echo "找不到 aarch64 sysroot：$sysroot/usr/lib/aarch64-linux-android" >&2
    exit 1
}

CLANG=$(command -v clang || true)
[ -n "$CLANG" ] || { echo "缺少宿主 clang：apt install clang（或 pkg install clang）" >&2; exit 1; }
CXX=$(command -v clang++ || echo "$CLANG")
CLANG_MAJOR=$("$CLANG" --version | sed -nE '1s/.*clang version ([0-9]+)\..*/\1/p')
[ -n "$CLANG_MAJOR" ] || { echo "无法识别 clang 版本" >&2; exit 1; }

# NDK 侧只借它的目标架构 runtime（builtins / libunwind），版本号可能与本机 clang 不同。
NDK_RT_DIR=$(ls -d "${HOST_PRE%/}"/lib/clang/*/lib/linux 2>/dev/null | tail -n1)
[ -n "$NDK_RT_DIR" ] || { echo "NDK 里找不到 lib/clang/*/lib/linux" >&2; exit 1; }

SHIM="$BIN/ndk-clang-shim.sh"
cat > "$SHIM" <<EOF
#!/bin/sh
# 由 scripts/make-ndk-host-native.sh 生成。改写 NDK prebuilt 目录后重跑该脚本即可。
self=\$(readlink -f "\$0")
PRE=\$(dirname "\$(dirname "\$self")")
name=\$(basename "\$0")
case "\$name" in
    *clang++*) cc="$CXX";;
    *)         cc="$CLANG";;
esac
case "\$name" in
    aarch64-linux-android[0-9]*-clang*) ;;
    clang|clang++|*-clang|*-clang++) ;;
    *)
        echo "make-ndk-host-native.sh: 只支持 aarch64 目标（调用名：\$name）" >&2
        exit 1
        ;;
esac
api=\$(printf '%s' "\$name" | sed -nE 's/^aarch64-linux-android([0-9]+)-.*/\1/p')
[ -n "\$api" ] || api=31
exec "\$cc" "\$@" \\
  --target=aarch64-linux-android\$api \\
  --sysroot="\$PRE/sysroot" \\
  --rtlib=compiler-rt \\
  -L"$NDK_RT_DIR" \\
  -L"$NDK_RT_DIR/aarch64" \\
  -fuse-ld=lld
EOF
chmod +x "$SHIM"

cd "$BIN"
for f in *clang*; do
    [ "$f" = "ndk-clang-shim.sh" ] && continue
    rm -rf "$f"
    ln -s ndk-clang-shim.sh "$f"
done

# 宿主 LLVM 工具（同目标架构，能处理 aarch64 ELF）。
LLVM_BIN=""
for d in "/usr/lib/llvm-$CLANG_MAJOR/bin" "/usr/local/opt/llvm/bin" "$(dirname "$CLANG")"; do
    [ -x "$d/llvm-strip" ] && { LLVM_BIN="$d"; break; }
done
if [ -z "$LLVM_BIN" ]; then
    echo "警告：找不到宿主 llvm-strip（apt install llvm-$CLANG_MAJOR）。" >&2
    echo "      AGP 的 strip 任务会失败；debug 构建通常不受影响。" >&2
else
    for t in llvm-ar llvm-ranlib llvm-strip llvm-objcopy llvm-nm llvm-readelf \
             llvm-objdump llvm-size llvm-addr2line llvm-strings llvm-mc llvm-cxxfilt; do
        [ -x "$LLVM_BIN/$t" ] || continue
        rm -f "$t"
        ln -s "$LLVM_BIN/$t" "$t"
    done
fi

echo "OK: $BIN 已改成本机可执行的 shim（宿主 clang $CLANG_MAJOR，NDK runtime $NDK_RT_DIR）"
echo "提示：如果之前 Gradle 报 aapt2 'Exec format error'，还需要 scripts/build-android-game.sh 里的 aarch64 aapt2 说明。"
