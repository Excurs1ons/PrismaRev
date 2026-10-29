#!/usr/bin/env bash
# 把 Web 编辑器的 fixture（真实 schema / 模板 / 资产列表）同步到 GPUI 壳。
# Web 侧用 `cd editor && pnpm fixture` 生成 editor/public/demo.json。
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
cp "$root/editor/public/demo.json" "$root/editor-gpui/assets/demo.json"
echo "synced editor/public/demo.json -> editor-gpui/assets/demo.json"
