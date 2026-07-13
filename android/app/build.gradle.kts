plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
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
    implementation("androidx.appcompat:appcompat:1.6.1")
    implementation("androidx.core:core-ktx:1.15.0")
}

// Sync user-provided environment/texture resources from the repo's top-level
// assets/ directory into the APK assets so they are bundled and loadable at
// runtime. The engine scans for *.hdr by name, so files keep their own names
// (e.g. valley_of_desolation_1k.hdr) — no rename needed.
val syncPrismaAssets by tasks.registering(Copy::class) {
    from(rootDir.resolve("../assets"))
    include("*.hdr", "*.exr", "*.png", "*.jpg", "*.jpeg", "*.ktx2", "*.env")
    into(layout.projectDirectory.dir("src/main/assets"))
}

tasks.named("preBuild") {
    dependsOn(syncPrismaAssets)
}
