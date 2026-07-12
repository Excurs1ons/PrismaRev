#!/usr/bin/env pwsh
<#
.SYNOPSIS
    Build PrismaRev for Android arm64-v8a: compile Rust cdylib via cargo-ndk,
    then assemble the APK via Gradle.
#>
$ErrorActionPreference = "Stop"

$ProjectRoot = Split-Path -LiteralPath $PSScriptRoot -Parent
$AndroidDir  = Join-Path -LiteralPath $ProjectRoot "android"
$JniLibsDir  = Join-Path -LiteralPath $AndroidDir "app\src\main\jniLibs"

# ---- Prerequisites ---------------------------------------------------------

# 1. Rust Android target
$target = "aarch64-linux-android"
$installed = rustup target list --installed 2>$null
if ($installed -notcontains $target) {
    Write-Host "Adding Rust target $target..."
    rustup target add $target
}

# 2. cargo-ndk
$ndkVer = cargo ndk --version 2>$null
if ($LASTEXITCODE -ne 0) {
    Write-Host "Installing cargo-ndk..."
    cargo install cargo-ndk
}

# 3. ANDROID_NDK_HOME or ANDROID_HOME
if (-not $env:ANDROID_NDK_HOME -and -not $env:ANDROID_HOME) {
    Write-Warning "Neither ANDROID_NDK_HOME nor ANDROID_HOME is set."
    Write-Warning "Set one of them to your NDK installation path."
    exit 1
}

# ---- Build Rust .so --------------------------------------------------------

Write-Host "Building prism-android for $target..."

cargo ndk `
    -t arm64-v8a `
    -o $JniLibsDir `
    build --release -p prism-android

if ($LASTEXITCODE -ne 0) {
    Write-Error "cargo ndk failed"
    exit 1
}

Write-Host "Rust .so built successfully."

# ---- Assemble APK via Gradle -----------------------------------------------

$Gradlew = Join-Path -LiteralPath $AndroidDir "gradlew"
if (-not (Test-Path -LiteralPath $Gradlew)) {
    Write-Host ""
    Write-Host "Gradle wrapper not found. Generate it with one of these methods:"
    Write-Host "  1) Open $AndroidDir in Android Studio (auto-generates wrapper)"
    Write-Host "  2) Run: gradle wrapper --gradle-version 8.11.1"
    Write-Host "  3) Run: cd android && gradle wrapper"
    Write-Host ""
    Write-Host "After the wrapper exists, run this script again."
    Write-Host "The .so file is ready at: $JniLibsDir"
    exit 0
}

Write-Host "Running Gradle assembleDebug..."
Push-Location -LiteralPath $AndroidDir
try {
    & $Gradlew assembleDebug
    if ($LASTEXITCODE -ne 0) {
        Write-Error "Gradle build failed"
        exit 1
    }
    Write-Host "APK: $AndroidDir\app\build\outputs\apk\debug\app-debug.apk"
} finally {
    Pop-Location
}

Write-Host "Build complete."
