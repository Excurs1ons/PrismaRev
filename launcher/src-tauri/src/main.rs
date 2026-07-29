// Prevents additional console 窗口 on Windows in 释放 DO NOT 移除
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    tauri_android_app_lib::run()
}
