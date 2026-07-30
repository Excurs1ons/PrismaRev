#!/bin/bash
export JAVA_HOME="C:\Program Files\Microsoft\jdk-17.0.18.8-hotspot"
export NDK_HOME="C:/Users/JasonGu/AppData/Local/Android/Sdk/ndk/30.0.14904198"
export ANDROID_NDK_HOME="C:/Users/JasonGu/AppData/Local/Android/Sdk/ndk/30.0.14904198"
export ANDROID_HOME="C:/Users/JasonGu/AppData/Local/Android/Sdk"
export JAVA_TOOL_OPTIONS="-Djavax.net.ssl.trustStoreType=Windows-ROOT -Dcom.sun.net.ssl.checkRevocation=false -Dcom.sun.security.disableRevocation=true"
cd "F:/repos/PrismaRev/launcher"
pnpm tauri android build --target aarch64 --apk --split-per-abi > "F:/repos/PrismaRev/launcher/tauri_build_release.log" 2>&1
echo "BUILD_EXIT=$?"
