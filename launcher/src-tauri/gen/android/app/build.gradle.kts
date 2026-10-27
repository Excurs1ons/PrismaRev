import java.util.Properties

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("rust")
}

val tauriProperties = Properties().apply {
    val propFile = file("tauri.properties")
    if (propFile.exists()) {
        propFile.inputStream().use { load(it) }
    }
}

// ---- 发布签名材料 -----------------------------------------------------------
// release APK **不能**用公开的 debug keystore 签名：同一个 applicationId + 公开
// 密钥意味着任何人都能签一个可以覆盖安装的更新。材料只从本机文件或环境变量读取，
// 两个来源都不入库（`keystore.properties` 已 gitignore）。
//
//   launcher/src-tauri/gen/android/keystore.properties
//     storeFile=/abs/path/release.jks   # 相对路径按 gen/android/ 解析
//     storePassword=...
//     keyAlias=...
//     keyPassword=...
//
// 或环境变量 PRISMREV_ANDROID_KEYSTORE / _KEYSTORE_PASSWORD / _KEY_ALIAS / _KEY_PASSWORD。
// 缺材料时 debug 构建照常，release 构建会在任务图确定后直接失败并说明缺什么。
val keystoreProperties = Properties().apply {
    val propFile = rootProject.file("keystore.properties")
    if (propFile.exists()) {
        propFile.inputStream().use { load(it) }
    }
}

val signingValue: (String, String) -> String? = { property, env ->
    keystoreProperties.getProperty(property)?.takeIf { it.isNotBlank() }
        ?: System.getenv(env)?.takeIf { it.isNotBlank() }
}

val rawStorePath = signingValue("storeFile", "PRISMREV_ANDROID_KEYSTORE")
val releaseStorePassword = signingValue("storePassword", "PRISMREV_ANDROID_KEYSTORE_PASSWORD")
val releaseKeyAlias = signingValue("keyAlias", "PRISMREV_ANDROID_KEY_ALIAS")
val releaseKeyPassword = signingValue("keyPassword", "PRISMREV_ANDROID_KEY_PASSWORD")
val releaseStoreFile: File? = rawStorePath?.let {
    val candidate = File(it)
    if (candidate.isAbsolute) candidate else rootProject.file(it)
}
val hasReleaseSigning = releaseStoreFile != null && releaseStorePassword != null &&
    releaseKeyAlias != null && releaseKeyPassword != null

gradle.taskGraph.whenReady {
    // 只在真的要出 release 产物时拦：配置阶段就抛错会连 `assembleDebug` 一起挂掉。
    val wantsRelease = allTasks.any { it.name.contains("Release") }
    if (wantsRelease && !hasReleaseSigning) {
        throw GradleException(
            "缺少 Android 发布签名材料。请在 launcher/src-tauri/gen/android/keystore.properties " +
            "填写 storeFile / storePassword / keyAlias / keyPassword，或设置 " +
            "PRISMREV_ANDROID_KEYSTORE、PRISMREV_ANDROID_KEYSTORE_PASSWORD、" +
            "PRISMREV_ANDROID_KEY_ALIAS、PRISMREV_ANDROID_KEY_PASSWORD。" +
            "debug 构建（--debug）不需要这些。"
        )
    }
}

android {
    compileSdk = 36
    namespace = "com.example.tauriandroidapp"
    defaultConfig {
        manifestPlaceholders["usesCleartextTraffic"] = "false"
        applicationId = "com.example.tauriandroidapp"
        minSdk = 31
        targetSdk = 36
        versionCode = tauriProperties.getProperty("tauri.android.versionCode", "1").toInt()
        versionName = tauriProperties.getProperty("tauri.android.versionName", "1.0")
        ndk {
            abiFilters += listOf("arm64-v8a")
        }
    }
    signingConfigs {
        create("release") {
            if (hasReleaseSigning) {
                storeFile = releaseStoreFile
                storePassword = releaseStorePassword
                keyAlias = releaseKeyAlias
                keyPassword = releaseKeyPassword
            }
        }
    }
    buildTypes {
        getByName("debug") {
            manifestPlaceholders["usesCleartextTraffic"] = "true"
            isDebuggable = true
            isJniDebuggable = true
            isMinifyEnabled = false
            packaging {                jniLibs.keepDebugSymbols.add("*/arm64-v8a/*.so")
                jniLibs.keepDebugSymbols.add("*/armeabi-v7a/*.so")
                jniLibs.keepDebugSymbols.add("*/x86/*.so")
                jniLibs.keepDebugSymbols.add("*/x86_64/*.so")
            }
        }
    getByName("release") {
        isMinifyEnabled = true
        signingConfig = signingConfigs.getByName("release")
        proguardFiles(
                *fileTree(".") { include("**/*.pro") }
                    .plus(getDefaultProguardFile("proguard-android-optimize.txt"))
                    .toList().toTypedArray()
            )
        }
    }
    kotlinOptions {
        jvmTarget = "1.8"
    }
    buildFeatures {
        buildConfig = true
    }
    packaging {
        jniLibs {
            // Keep only arm64-v8a; the Tauri rust plugin builds all ABIs but we
            // only ship one.
            exclude("**/armeabi-v7a/**")
            exclude("**/x86/**")
            exclude("**/x86_64/**")
        }
    }
}

rust {
    rootDirRel = "../../../"
}

dependencies {
    implementation("androidx.webkit:webkit:1.14.0")
    implementation("androidx.appcompat:appcompat:1.7.1")
    implementation("androidx.activity:activity-ktx:1.10.1")
    implementation("com.google.android.material:material:1.12.0")
    implementation("androidx.lifecycle:lifecycle-process:2.10.0")
    implementation("androidx.games:games-activity:4.4.0")
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation("androidx.test.ext:junit:1.1.4")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.5.0")
}

apply(from = "tauri.build.gradle.kts")