# PrismaRev 一键启动脚本 (Windows)
# 用法: 在 PowerShell 中 cd 到本目录后执行  .\run.ps1
# 需要: 已安装 Vulkan SDK (默认 D:\VulkanSDK\1.4.350.0，自带 slangc) 和 Rust
$ErrorActionPreference = "Stop"

$sdk = "D:\VulkanSDK\1.4.350.0"
if (-not (Test-Path "$sdk\Bin\glslc.exe")) {
    # 回退到常见安装路径
    $sdk = "C:\VulkanSDK\1.4.350.0"
}
if (-not (Test-Path "$sdk\Bin\glslc.exe")) {
    Write-Warning "未找到 Vulkan SDK，请从 https://vulkan.lunarg.com/sdk/home 安装，并确认路径。"
}

$env:PATH = "$env:USERPROFILE\.cargo\bin;$sdk\Bin;" + $env:PATH
$env:VK_SDK = $sdk
$env:VK_LAYER_PATH = "$sdk\Bin"
$env:VULKAN_SDK = $sdk
$env:RUST_LOG = "info,tracy_client=off"

Set-Location $PSScriptRoot

# 重新编译 Slang 着色器 (slangc 保留 vertexMain/fragmentMain 入口名)。
# slangc 随 Vulkan SDK 提供；若无 slangc，回退到旧的 GLSL compile.bat
# (但注意 GLSL 产物入口名为 main，与当前 Rust 代码不兼容，仅作无 Slang 时的参考)。
if (Get-Command slangc -ErrorAction SilentlyContinue) {
    Write-Host "重新编译 Slang 着色器..."
    & bash shaders/compile.sh
    if ($LASTEXITCODE -ne 0) { Write-Error "Slang 着色器编译失败"; exit 1 }
} else {
    Write-Warning "未找到 slangc。请确认 Vulkan SDK 已含 slangc，或单独安装 Slang。"
    Write-Warning "跳过着色器编译 (将使用已提交的 .spv)。"
}

# 构建并运行 debug 版本
Write-Host "构建 PrismaRev (debug)..."
cargo build
if ($LASTEXITCODE -ne 0) { Write-Error "构建失败"; exit 1 }

Write-Host "启动 PrismaRev，关闭窗口即可退出..."
cargo run
