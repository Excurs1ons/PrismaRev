//! Integration tests for the `prism-asset-cli` 二进制 (std::process::Command).
//!
//! These tests 构建 a minimal .pak, 调用 the CLI 二进制 and 验证 its
//! stdout / stderr 输出 — ensuring the validate-shortcut and metadata
//! features 功 as expected.

use std::path::{Path, PathBuf};
use std::process::Command;

use prism_asset_core::{AssetId, AssetType};
use prism_asset_package::PackageBuilder;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// 定位 the compiled prism-asset-cli 二进制
///
/// `cargo test` places the test 可执行文件 in `target/debug/deps/`, while the
/// CLI 二进制 lives in `target/debug/` (or `target/debug/prism-asset-cli.exe` on
/// Windows We walk 上 from the test exe to 查找 it.
fn cli_binary() -> PathBuf {
    let mut path = std::env::current_exe().expect("current exe");
    // 弹出 the 二进制 name, then the "deps" directory if present.
    path.pop(); // binary name
    if path.ends_with("deps") {
        path.pop(); // deps
    }
    path.push(if cfg!(windows) {
        "prism-asset-cli.exe"
    } else {
        "prism-asset-cli"
    });
    if !path.exists() {
        // 回退 maybe we're in a 工作区 目标 dir.
        path.pop();
        path.push("debug");
        path.push(if cfg!(windows) {
            "prism-asset-cli.exe"
        } else {
            "prism-asset-cli"
        });
    }
    assert!(path.exists(), "CLI binary not found: {}", path.display());
    path
}

/// 构建 a minimal 有效 .pak with a single 资源
fn build_minimal_pak(dir: &Path, name: &str) -> PathBuf {
    let id = AssetId::from_raw(0x1000_0001);
    let mut builder = PackageBuilder::new();
    builder.add_asset(id, AssetType::Binary, b"hello world".to_vec(), &[]);
    let pak_bytes = builder.build().expect("build minimal .pak");
    let pak_path = dir.join(name);
    std::fs::write(&pak_path, &pak_bytes).expect("write .pak");
    pak_path
}

/// Run the CLI 二进制 with the given arguments, returning (stdout, stderr).
fn run_cli(args: &[&str]) -> (String, String) {
    let output = Command::new(cli_binary())
        .args(args)
        .output()
        .expect("failed to run prism-asset-cli");

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (stdout, stderr)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn cli_no_args_shows_help() {
    let (stdout, stderr) = run_cli(&[]);
    // Should show 用法 text on stdout.
    assert!(
        stdout.contains("Usage:"),
        "help text should contain Usage:\n{stdout}"
    );
    assert!(
        stdout.contains("prism-asset-cli"),
        "help text should mention prism-asset-cli\n{stdout}"
    );
    assert!(stderr.is_empty(), "no error expected on stderr:\n{stderr}");
}

#[test]
fn cli_nonexistent_pak_reports_error() {
    let (_stdout, stderr) = run_cli(&["C:/this/does/not/exist.pak"]);
    // The validate 命令 returns an 错误 → should appear on stderr.
    assert!(
        stderr.contains("error") || stderr.contains("Error"),
        "stderr should mention error:\n{stderr}"
    );
    // The 二进制 should exit non-zero, but we only check 输出 text.
}

#[test]
fn cli_validate_with_meta_json_shows_asset_names() {
    let dir = tempfile::tempdir().expect("temp dir");
    let pak_path = build_minimal_pak(dir.path(), "game.pak");

    // 写入 a .pak.meta.json alongside the .pak.
    let meta = serde_json::json!({
        "pak": "game.pak",
        "format": "RPAK",
        "version": 1,
        "asset_count": 1,
        "total_size": 123,
        "assets": [{
            "id": "0x10000001",
            "path": "my_asset.bin",
            "type": "binary",
            "importer": "raw-importer",
            "size": 11,
            "compressed_size": null,
            "compression_ratio": null
        }]
    });
    let meta_path = pak_path.with_extension("pak.meta.json");
    std::fs::write(&meta_path, serde_json::to_string_pretty(&meta).unwrap())
        .expect("write .pak.meta.json");

    let (stdout, stderr) = run_cli(&[&pak_path.to_string_lossy()]);

    assert!(stderr.is_empty(), "no error expected:\n{stderr}");
    assert!(stdout.contains("RPAK"), "should show magic:\n{stdout}");
    assert!(
        stdout.contains("my_asset.bin"),
        "should show asset name from metadata:\n{stdout}"
    );
    assert!(
        stdout.contains("binary"),
        "should show asset type:\n{stdout}"
    );
}

#[test]
fn cli_validate_without_meta_json_shows_fallback() {
    let dir = tempfile::tempdir().expect("temp dir");
    let pak_path = build_minimal_pak(dir.path(), "game.pak");
    // Intentionally *not* writing a .pak.meta.json.

    let (stdout, stderr) = run_cli(&[&pak_path.to_string_lossy()]);

    assert!(stderr.is_empty(), "no error expected:\n{stderr}");
    assert!(stdout.contains("RPAK"), "should show magic:\n{stdout}");
    // 回退 标签 should mention 二进制 and 资源
    assert!(
        stdout.contains("binary") && stdout.contains("asset"),
        "fallback should mention asset type:\n{stdout}"
    );
    // Tip about rebuilding should appear.
    assert!(
        stdout.contains("Tip / 提示"),
        "should show tip about rebuilding:\n{stdout}"
    );
}

#[test]
fn cli_validate_subcommand_works() {
    let dir = tempfile::tempdir().expect("temp dir");
    let pak_path = build_minimal_pak(dir.path(), "game.pak");

    let (stdout, stderr) = run_cli(&["validate", &pak_path.to_string_lossy()]);

    assert!(stderr.is_empty(), "no error expected:\n{stderr}");
    assert!(
        stdout.contains("RPAK"),
        "validate subcommand should show magic:\n{stdout}"
    );
    assert!(stdout.contains("1"), "should show 1 asset:\n{stdout}");
}

#[test]
fn cli_help_subcommand_shows_help() {
    let (stdout, stderr) = run_cli(&["help"]);
    assert!(
        stdout.contains("Usage:"),
        "help subcommand should show usage:\n{stdout}"
    );
    assert!(stderr.is_empty(), "no error on stderr:\n{stderr}");
}
