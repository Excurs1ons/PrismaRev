# 用 CDP 调试端口启动 web 编辑器，供 prism-editor-mcp 的 ui_* 工具驱动。
#
# 用法：powershell -File scripts/editor-cdp.ps1 [-Port 9222]
param(
    [int]$Port = 9222
)

$ErrorActionPreference = "Stop"

$root = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
$editor = Join-Path $root "editor"
if (-not (Test-Path (Join-Path $editor "package.json"))) {
    Write-Error "找不到 editor/：$editor"
}

$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$Port"
Write-Host "WebView2 调试端口: $Port"
Write-Host "验证: curl http://127.0.0.1:$Port/json"

Push-Location $editor
try {
    pnpm tauri dev
} finally {
    Pop-Location
}
