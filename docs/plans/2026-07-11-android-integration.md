# Android Integration Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build and run PrismaRev on Android via winit's `android-game-activity` feature + GameActivity + `cargo-ndk`.

**Architecture:**
- Existing `prism-engine` crate stays a pure Rust library; a new thin `prism-android` crate (`crate-type = ["cdylib"]`) provides the `android_main(app: AndroidApp)` entry point.
- `App::run()` is refactored into `App::run()` → `App::run_on_event_loop(EventLoop)` so Android can pass a pre-configured event loop (with AndroidApp).
- Desktop binary (`src/main.rs`) untouched; Android goes through a separate Gradle project that loads the `.so`.

**Tech Stack:** winit 0.30 (w/ `android-game-activity`), GameActivity 4.4.0, `cargo-ndk`, Android SDK 35 / API 31+, Vulkan 1.1+

---

### Task 1: Split `App::run()` into `run()` + `run_on_event_loop()`

**Files:**
- Modify: `crates/prism-engine/src/app.rs`
- Modify: `crates/prism-engine/src/lib.rs`

**Step 1: Refactor `App::run()`**

Current:
```rust
pub fn run() -> anyhow::Result<()> {
    let event_loop = EventLoop::new()?;
    let mut app = App::new();
    event_loop.run_app(&mut app)?;
    Ok(())
}
```

Change to:
```rust
pub fn run() -> anyhow::Result<()> {
    Self::run_on_event_loop(EventLoop::new()?)
}

pub fn run_on_event_loop(event_loop: EventLoop<()>) -> anyhow::Result<()> {
    let mut app = App::new();
    event_loop.run_app(&mut app)?;
    Ok(())
}
```

**Step 2: Update exports in `lib.rs`**

`App::run_on_event_loop` is already public (method on public `App`), no change needed.

**Step 3: Verify desktop still builds**

Run: `cargo build` — expected 0 warnings, success.

---

### Task 2: Create `prism-android` cdylib crate

**Files:**
- Create: `crates/prism-android/Cargo.toml`
- Create: `crates/prism-android/src/lib.rs`
- Modify: `Cargo.toml` (workspace root — add workspace member)

**Step 1: Create `crates/prism-android/Cargo.toml`**

```toml
[package]
name = "prism-android"
version.workspace = true
edition.workspace = true
license.workspace = true
description = "Android cdylib entry point for PrismaRev"

[lib]
crate-type = ["cdylib"]

[dependencies]
prism-engine = { path = "../prism-engine" }
winit = { workspace = true, features = ["android-game-activity"] }
log = { workspace = true }
anyhow = { workspace = true }
```

**Step 2: Create `crates/prism-android/src/lib.rs`**

```rust
//! Android entry point for PrismaRev.
//!
//! This crate produces a cdylib (.so) loaded by GameActivity. The
//! `android_main` function is the native entry point called by the
//! Android system. It creates a winit event loop configured with the
//! AndroidApp and delegates to `prism_engine::App::run_on_event_loop`.

use winit::event_loop::EventLoopBuilder;

#[cfg(target_os = "android")]
use winit::platform::android::{AndroidApp, EventLoopBuilderExtAndroid};

#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(app: AndroidApp) {
    log::info!("PrismaRev Android starting");
    let event_loop = EventLoopBuilder::new()
        .with_android_app(app)
        .build()
        .expect("failed to build Android event loop");
    prism_engine::App::run_on_event_loop(event_loop).expect("App::run_on_event_loop failed");
}
```

`#[cfg(target_os = "android")]` on the `use` ensures the import only exists on Android targets (avoids unused import warnings on desktop). The `#[no_mangle]` function also uses `#[cfg(target_os = "android")]` because `android_main` is meaningless on other targets.

Note: The function is `#[cfg(target_os = "android")]`, so this crate compiles fine on desktop (it just produces an empty .so with no entry point — which is harmless).

**Step 3: Add to workspace**

In root `Cargo.toml`, add `"crates/prism-android"` to `[workspace] members`.

**Step 4: Verify workspace resolves**

Run: `cargo check -p prism-android` — expected success on desktop (empty .so).

---

### Task 3: Create Android Gradle project

**Files:**
- Create: `android/settings.gradle.kts`
- Create: `android/build.gradle.kts`
- Create: `android/gradle.properties`
- Create: `android/gradle/wrapper/gradle-wrapper.properties`
- Create: `android/app/build.gradle.kts`
- Create: `android/app/src/main/AndroidManifest.xml`
- Create: `android/app/src/main/java/com/prismarev/MainActivity.kt`
- Create: `android/app/src/main/res/values/strings.xml`
- Create: `android/app/src/main/res/values/themes.xml`

**Step 1: Root Gradle files**

`android/settings.gradle.kts`:
```kotlin
pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}
dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
    }
}
rootProject.name = "PrismaRev"
include(":app")
```

`android/build.gradle.kts`:
```kotlin
plugins {
    id("com.android.application") version "8.7.3" apply false
}
```

`android/gradle.properties`:
```
org.gradle.jvmargs=-Xmx2048m
android.useAndroidX=true
```

`android/gradle/wrapper/gradle-wrapper.properties`:
```
distributionBase=GRADLE_USER_HOME
distributionPath=wrapper/dists
distributionUrl=https\://services.gradle.org/distributions/gradle-8.11.1-bin.zip
zipStoreBase=GRADLE_USER_HOME
zipStorePath=wrapper/dists
```

**Step 2: App module**

`android/app/build.gradle.kts`:
```kotlin
plugins {
    id("com.android.application")
}

android {
    namespace = "com.prismarev"
    compileSdk = 35

    defaultConfig {
        applicationId = "com.prismarev"
        minSdk = 31
        targetSdk = 35
        ndk {
            abiFilters += listOf("arm64-v8a")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    sourceSets {
        getByName("main") {
            // The .so files from cargo-ndk go into jniLibs/<abi>/
            jniLibs.srcDirs("src/main/jniLibs")
        }
    }
}

dependencies {
    implementation("androidx.games:games-activity:4.4.0")
}
```

**Step 3: AndroidManifest.xml**

`android/app/src/main/AndroidManifest.xml`:
```xml
<?xml version="1.0" encoding="utf-8"?>
<manifest xmlns:android="http://schemas.android.com/apk/res/android">
    <uses-feature android:name="android.hardware.vulkan.level" android:required="true" android:version="1" />
    <uses-sdk android:minSdkVersion="31" />

    <application
        android:allowBackup="false"
        android:label="PrismaRev"
        android:hasCode="true">
        <activity
            android:name="com.prismarev.MainActivity"
            android:exported="true"
            android:configChanges="orientation|keyboardHidden|screenSize"
            android:screenOrientation="landscape">
            <meta-data android:name="android.app.lib_name" android:value="prism_android" />
            <intent-filter>
                <action android:name="android.intent.action.MAIN" />
                <category android:name="android.intent.category.LAUNCHER" />
            </intent-filter>
        </activity>
    </application>
</manifest>
```

Note: `android.app.lib_name` must match the Rust library name without the leading `lib` prefix. With `crate-type = ["cdylib"]`, Rust produces `libprism_android.so`. The `_` vs `-` mapping: Rust replaces `-` with `_` for the library name. The Cargo.toml `package.name = "prism-android"` becomes `libprism_android.so`.

**Step 4: Kotlin Activity**

`android/app/src/main/java/com/prismarev/MainActivity.kt`:
```kotlin
package com.prismarev

import androidx.games.activity.GameActivity

class MainActivity : GameActivity()
```

**Step 5: Resources**

`android/app/src/main/res/values/strings.xml`:
```xml
<?xml version="1.0" encoding="utf-8"?>
<resources>
    <string name="app_name">PrismaRev</string>
</resources>
```

`android/app/src/main/res/values/themes.xml`:
```xml
<?xml version="1.0" encoding="utf-8"?>
<resources>
    <style name="AppTheme" parent="android:Theme.Material.NoActionBar.Fullscreen">
        <item name="android:windowFullscreen">true</item>
        <item name="android:windowNoTitle">true</item>
    </style>
</resources>
```

---

### Task 4: Build integration & Makefile

**Files:**
- Create: `Makefile.toml` (cargo-make) or `build-android.sh` / `build-android.ps1`
- Modify: `.cargo/config.toml` (optional NDK paths)

**Step 1: Verify NDK installation**

Check `ANDROID_NDK_HOME` or `ANDROID_HOME` environment variables.

**Step 2: Create build script**

`scripts/build-android.ps1`:
```powershell
#!/usr/bin/env pwsh
# Build Rust .so for Android arm64-v8a

$ErrorActionPreference = "Stop"

# Ensure cargo-ndk is installed
cargo ndk --version 2>$null
if ($LASTEXITCODE -ne 0) {
    cargo install cargo-ndk
}

# Build the cdylib
cargo ndk -t arm64-v8a -o android/app/src/main/jniLibs build --release -p prism-android

Write-Host "Build complete. .so placed in android/app/src/main/jniLibs/"
Write-Host "Run: cd android && ./gradlew installDebug"
```

**Step 3: Test complete build pipeline**

Run: `scripts/build-android.ps1` (or equivalent) — expected:
1. `cargo ndk` compiles and copies `.so` to `android/app/src/main/jniLibs/arm64-v8a/libprism_android.so`
2. Gradle `installDebug` installs APK to connected device/emulator

---

### Verification

| Check | Command | Expected |
|-------|---------|----------|
| Desktop build | `cargo build` | 0 warnings, all crates compile |
| Workspace check | `cargo check -p prism-android` | Success (desktop .so empty) |
| Tests | `cargo test` | 54 tests pass |
| NDK build | `cargo ndk -t arm64-v8a build --release -p prism-android` | `.so` produced |
| Gradle build | `cd android && ./gradlew assembleDebug` | APK produced |

---

### Files Changed Summary

| File | Action |
|------|--------|
| `crates/prism-engine/src/app.rs` | MODIFY: add `run_on_event_loop()` |
| `Cargo.toml` | MODIFY: add `crates/prism-android` to workspace members |
| `crates/prism-android/Cargo.toml` | CREATE: cdylib crate |
| `crates/prism-android/src/lib.rs` | CREATE: `android_main` entry point |
| `android/settings.gradle.kts` | CREATE: Gradle settings |
| `android/build.gradle.kts` | CREATE: root Gradle |
| `android/gradle.properties` | CREATE: Gradle properties |
| `android/gradle/wrapper/gradle-wrapper.properties` | CREATE: wrapper config |
| `android/app/build.gradle.kts` | CREATE: app module |
| `android/app/src/main/AndroidManifest.xml` | CREATE: manifest |
| `android/app/src/main/java/com/prismarev/MainActivity.kt` | CREATE: GameActivity |
| `android/app/src/main/res/values/strings.xml` | CREATE: strings |
| `android/app/src/main/res/values/themes.xml` | CREATE: theme |
| `scripts/build-android.ps1` | CREATE: build script |
