# PrismaRev 一键构建脚本 (Windows)
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
$env:RUST_LOG = "warn,tracy_client=off"

# 脚本位于 scripts/ 下，切到仓库根目录
Set-Location (Split-Path -Parent $PSScriptRoot)

# 重新编译 Slang 着色器 (slangc 保留 vertexMain/fragmentMain 入口名)。
# slangc 随 Vulkan SDK 提供；.spv 不提交，必须编译后引擎才使用最新着色器。
if (Get-Command slangc -ErrorAction SilentlyContinue) {
    Write-Host "重新编译 Slang 着色器..."
    & bash assets/shaders/compile.sh
    if ($LASTEXITCODE -ne 0) { Write-Error "Slang 着色器编译失败"; exit 1 }
} else {
    Write-Warning "未找到 slangc。请确认 Vulkan SDK 已含 slangc，或单独安装 Slang。"
    Write-Warning "跳过着色器编译 (将使用上次编译产物或 CI 产出的 .spv)。"
}

# 构建整个 workspace（渲染 / ECS / 引擎 / 资产管线均以库形式构建）。
# 注意：桌面可执行入口已迁移到 launcher/（Tauri 独立 workspace），
# 根 workspace 没有可运行的 bin，`cargo run` 不再适用。
Write-Host "构建 PrismaRev (debug)..."
cargo build
if ($LASTEXITCODE -ne 0) { Write-Error "构建失败"; exit 1 }

Write-Host ""
Write-Host "构建完成。启动入口："
Write-Host "  launcher/ (Tauri 桌面壳 + Android APK)：cd launcher && pnpm tauri dev"
Write-Host "  Android：scripts/build-android.ps1 (Rust cdylib) + launcher/build_release.sh (APK)"
