# 发布档构建（Windows）：nightly `-Z build-std` + immediate-abort。
# 与 `scripts/build-dist.sh` 等价，参数/理由见那边的注释。
#
#   powershell -File scripts/build-dist.ps1
#   powershell -File scripts/build-dist.ps1 projects/prisma
#   powershell -File scripts/build-dist.ps1 projects/prisma x86_64-pc-windows-gnu
param(
    [string]$Project = "projects/prisma",
    [string]$Target = "",
    [string]$Profile = "dist",
    [string]$Nightly = "nightly"
)

$ErrorActionPreference = "Stop"
# scripts/ 的上一级就是仓库根；参数一律用位置传，PowerShell 5.1 的 Join-Path 不吃 -LiteralPath。
$Root = Split-Path -Parent $PSScriptRoot
$Manifest = Join-Path $Root (Join-Path $Project "Cargo.toml")
if (-not (Test-Path $Manifest)) { throw "找不到 $Manifest" }

if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) {
    throw "需要 rustup（用它装 nightly + rust-src）"
}

$installed = rustup toolchain list
if ($installed -notmatch "^$Nightly") {
    Write-Host "build-dist: 安装 $Nightly（首次约 100 MB）"
    rustup toolchain install $Nightly --profile minimal
}
$components = rustup component list --toolchain $Nightly
if ($components -notmatch "rust-src \(installed\)") {
    Write-Host "build-dist: 安装 rust-src"
    rustup component add rust-src --toolchain $Nightly
}

if ([string]::IsNullOrEmpty($Target)) {
    $Target = ((rustc -vV | Select-String '^host: ').ToString() -replace '^host: ', '').Trim()
}

$name = (Select-String -Path $Manifest -Pattern '^name = "(.*)"' | Select-Object -First 1).Matches[0].Groups[1].Value
Write-Host "build-dist: $Project ($name) → $Target / profile=$Profile"

# 参数只走命令行，不写进 Cargo.toml：stable 构建不受影响。
$env:RUSTFLAGS = "-Zunstable-options -Cpanic=immediate-abort"
cargo "+$Nightly" build `
    --manifest-path $Manifest `
    --profile $Profile `
    --package $name `
    --target $Target `
    -Z build-std=std,panic_abort
if ($LASTEXITCODE -ne 0) { throw "cargo build 失败（exit $LASTEXITCODE）" }

$bin = Join-Path $Root (Join-Path $Project "target\${Target}\${Profile}\${name}.exe")
if (Test-Path $bin) {
    $size = (Get-Item $bin).Length
    Write-Host ("build-dist: {0}  {1} bytes ({2:N2} MB)" -f $bin, $size, ($size / 1MB))
} else {
    Write-Host "build-dist: 构建成功但没找到 $bin（检查包名与 bin 名是否一致）"
}
