#!/usr/bin/env bash
# 发布档构建：nightly `-Z build-std` + immediate-abort，比 stable `dist` 再小 9~10%。
#
#   bash scripts/build-dist.sh                        # 默认 projects/prisma，宿主 target
#   bash scripts/build-dist.sh projects/fishing       # 指定工程
#   bash scripts/build-dist.sh projects/prisma aarch64-linux-android
#   bash scripts/build-dist.sh --size [project] [target]   # 极限体积（tiny profile + 禁内联）
#   PROFILE=dist bash scripts/build-dist.sh           # 换 profile
#
# 为什么要它：官方 libstd 是预编译的，链接时只能整块接受；`-Z build-std` 让 std 和你的
# 代码一起走 fat LTO，`-Cpanic=immediate-abort` 再删掉 panic 格式化（core::fmt）那一大片。
# 实测（aarch64 Linux，dist）：3D 全量 4.47 MB → 4.01 MB（−459 KB），
# 2D 极简 + 仅 X11 2.89 MB → 2.63 MB（−262 KB）。
#
# 代价：只在**发布构建**需要 nightly，日常 `cargo run` 仍是 stable——因为参数全在
# 命令行，没有写进任何 Cargo.toml（写了会让 stable cargo 直接拒绝解析清单）。
#
# 注意：immediate-abort 下 panic 只剩 abort，没有消息/回溯；只用于发布包，别拿它调试。
set -euo pipefail

readonly ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# --size：极限体积档（tiny profile + 禁止内联）。位置参数保持向后兼容。
SIZE_MODE=0
ARGS=()
for arg in "$@"; do
    case "$arg" in
        --size|--tiny) SIZE_MODE=1 ;;
        *) ARGS+=("$arg") ;;
    esac
done
PROJECT="${ARGS[0]:-projects/prisma}"
TARGET="${ARGS[1]:-}"
if [ "${SIZE_MODE}" = "1" ]; then
    PROFILE="${PROFILE:-tiny}"
else
    PROFILE="${PROFILE:-dist}"
fi
NIGHTLY="${NIGHTLY:-nightly}"

# ---------------------------------------------------------------- 环境检查
if ! command -v rustup >/dev/null 2>&1; then
    echo "build-dist: 需要 rustup（用它装 nightly + rust-src）" >&2
    exit 1
fi

if ! rustup toolchain list | grep -q "^${NIGHTLY}"; then
    echo "build-dist: 安装 ${NIGHTLY}（首次约 100 MB）"
    rustup toolchain install "${NIGHTLY}" --profile minimal
fi

if ! rustup component list --toolchain "${NIGHTLY}" | grep -q '^rust-src (installed)'; then
    echo "build-dist: 安装 rust-src"
    rustup component add rust-src --toolchain "${NIGHTLY}"
fi

# build-std 要求显式 --target，即使目标是宿主。
if [ -z "${TARGET}" ]; then
    TARGET="$("${NIGHTLY_TRIPLE:-rustc}" -vV 2>/dev/null | sed -n 's/^host: //p' || true)"
    if [ -z "${TARGET}" ]; then
        TARGET="$(rustc -vV | sed -n 's/^host: //p')"
    fi
fi

MANIFEST="${ROOT}/${PROJECT}/Cargo.toml"
if [ ! -f "${MANIFEST}" ]; then
    echo "build-dist: 找不到 ${MANIFEST}" >&2
    exit 1
fi

# 工程自己的 workspace 根（profile 从那里读）。
PKG="$(sed -n 's/^name = "\(.*\)"/\1/p' "${MANIFEST}" | head -1)"

echo "build-dist: ${PROJECT} (${PKG}) → ${TARGET} / profile=${PROFILE}"

# ---------------------------------------------------------------- 构建
# RUSTFLAGS 同时作用于 std 与工程，保证 panic 策略一致；
# 不写进 Cargo.toml，stable 构建因此完全不受影响。
# --size 再叠一刀：禁内联（纯体积手段）。热路径会慢 10–30%，只建议竞赛/极限场景用；
# 想两者兼得就在工程 Cargo.toml 里给关键 crate 单独写 [profile.tiny.package.<crate>] opt-level = 3。
EXTRA_RUSTFLAGS=""
if [ "${SIZE_MODE}" = "1" ]; then
    EXTRA_RUSTFLAGS=" -C llvm-args=-inline-threshold=0 -C llvm-args=-inlinehint-threshold=0"
fi

RUSTFLAGS="-Zunstable-options -Cpanic=immediate-abort${EXTRA_RUSTFLAGS}" \
    cargo "+${NIGHTLY}" build \
    --manifest-path "${MANIFEST}" \
    --profile "${PROFILE}" \
    --package "${PKG}" \
    --target "${TARGET}" \
    -Z build-std=std,panic_abort

# ---------------------------------------------------------------- 报告
BIN="${ROOT}/${PROJECT}/target/${TARGET}/${PROFILE}/${PKG}"
# Windows 目标的可执行文件带 `.exe` 后缀。
if [ ! -f "${BIN}" ] && [ -f "${BIN}.exe" ]; then
    BIN="${BIN}.exe"
fi
if [ -f "${BIN}" ]; then
    SIZE="$(stat -c%s "${BIN}")"
    printf 'build-dist: %s  %s bytes (%.2f MB)\n' "${BIN}" "${SIZE}" "$(echo "${SIZE}" | awk '{print $1/1048576}')"
else
    echo "build-dist: 构建成功但没找到 ${BIN}（检查包名与 bin 名是否一致）" >&2
fi
