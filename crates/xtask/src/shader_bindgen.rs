//! Slang reflection -> Rust 绑定 codegen for PrismaRev.
//!
//! Reads the `reflection/*.json` emitted by `slangc -reflection-json`
//! (see `shaders/compile.sh`) and generates a Rust 模块 describing each
//! shader's 资源 bindings: 描述符 set/binding indices, 资源 kinds,
//! and push-constant sizes. The generated file is committed to the repo so the
//! engine builds on hosts without slangc (Termux/Android).
//!
//! Run on a desktop/CI host after recompiling shaders:
//!   cargo run -p xtask --bin shader-bindgen -- \
//!     shaders/reflection crates/prism-render/src/shader_bindings.rs
//!
//! This is intentionally a standalone tool (NOT a build.rs) so the 法线
//! `cargo 构建 never needs slangc.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

/// Top-level reflection document.
#[derive(Debug, Deserialize)]
struct Reflection {
    #[serde(default)]
    parameters: Vec<Parameter>,
    #[serde(default, rename = "entryPoints")]
    entry_points: Vec<EntryPoint>,
}

#[derive(Debug, Deserialize)]
struct Parameter {
    name: String,
    #[serde(default)]
    binding: Option<Binding>,
    #[serde(rename = "type")]
    ty: Option<TypeInfo>,
}

#[derive(Debug, Deserialize)]
struct Binding {
    /// e.g. "descriptorTableSlot", "pushConstantBuffer", uniform
    kind: String,
    #[serde(default)]
    index: u32,
    #[serde(default)]
    space: u32,
    #[serde(default)]
    size: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct TypeInfo {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default, rename = "baseShape")]
    base_shape: Option<String>,
    /// For 结构体 types (constantBuffer → elementType → 结构体 with fields).
    #[serde(default, rename = "elementType")]
    element_type: Option<StructTypeInfo>,
}

/// 信息 about a 结构体 类型 nested inside a parameter's 类型
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct StructTypeInfo {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    fields: Option<Vec<StructField>>,
}

/// A single field inside a 结构体 定义 in the reflection JSON.
#[derive(Debug, Deserialize)]
struct StructField {
    name: String,
    #[serde(rename = "type")]
    ty: FieldType,
    #[serde(default)]
    binding: FieldBinding,
}

/// 类型 信息 for a 结构体 field (more detailed than TypeInfo).
#[derive(Debug, Deserialize)]
struct FieldType {
    /// 标量 向量 矩阵
    #[serde(default)]
    kind: String,
    /// "float32", "uint32", "int32"
    #[serde(default, rename = "scalarType")]
    scalar_type: Option<String>,
    /// For vectors: 2, 3, or 4
    #[serde(default, rename = "elementCount")]
    element_count: Option<u32>,
    /// For matrices: 4
    #[serde(default, rename = "rowCount")]
    row_count: Option<u32>,
    /// For matrices: 4
    #[serde(default, rename = "columnCount")]
    column_count: Option<u32>,
    /// For vectors/matrices: the element 类型
    #[serde(default, rename = "elementType")]
    element_type: Option<Box<FieldType>>,
}

/// 绑定 信息 for a 结构体 field.
#[derive(Debug, Default, Deserialize)]
struct FieldBinding {
    #[serde(default)]
    offset: Option<u32>,
    #[serde(default)]
    size: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct EntryPoint {
    name: String,
    #[serde(default)]
    stage: Option<String>,
    #[serde(default)]
    parameters: Vec<Parameter>,
}

/// 映射表 a Slang reflection field 类型 to its Rust 类型 name.
fn field_type_to_rust(ft: &FieldType) -> String {
    match ft.kind.as_str() {
        "scalar" => match ft.scalar_type.as_deref() {
            Some("float32") => "f32".into(),
            Some("uint32") => "u32".into(),
            Some("int32") => "i32".into(),
            Some("float16") => "f16".into(),
            Some("bool") => "u32".into(),
            _ => "u32".into(),
        },
        "vector" => {
            let count = ft.element_count.unwrap_or(4);
            let elem = ft
                .element_type
                .as_deref()
                .map(field_type_to_rust)
                .unwrap_or_else(|| "f32".into());
            format!("[{elem}; {count}]")
        }
        "matrix" => {
            let rows = ft.row_count.unwrap_or(4);
            let cols = ft.column_count.unwrap_or(4);
            let elem = ft
                .element_type
                .as_deref()
                .map(field_type_to_rust)
                .unwrap_or_else(|| "f32".into());
            format!("[[{elem}; {cols}]; {rows}]")
        }
        _ => "u32".into(),
    }
}

/// Generate a Rust 结构体 定义 from reflected 推送 常量 fields.
fn emit_push_struct(struct_name: &str, fields: &[StructField]) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "\n/// Push-constant struct (auto-generated from Slang `{}`).\n",
        struct_name
    ));
    out.push_str("#[repr(C)]\n#[derive(Clone, Copy, Default)]\n");
    out.push_str(&format!("pub struct {struct_name} {{\n"));
    for f in fields {
        let rust_type = field_type_to_rust(&f.ty);
        // 发射 doc 注释 showing the reflected offset+size for easy 验证
        let off = f.binding.offset.unwrap_or(0);
        let sz = f.binding.size.unwrap_or(0);
        out.push_str(&format!(
            "    /// offset {off}, size {sz}\n    pub {}: {},\n",
            f.name, rust_type
        ));
    }
    out.push_str("}\n");
    out
}

/// A resolved 绑定 fact we care about for Rust codegen.
struct ResolvedBinding {
    name: String,
    set: u32,
    binding: u32,
    kind: BindKind,
}

enum BindKind {
    UniformBuffer,
    CombinedImageSampler,
    PushConstant { size: u32 },
}

/// 回退 push-constant sizes for shaders whose slangc reflection omits the
/// 大小 field on the `pushConstantBuffer` 参数 Newer Slang releases
/// (e.g. 2026.13.1) stopped emitting 大小 on the 参数 绑定 so we
/// keep 回退 values here. Struct-level 布局 is now auto-generated from
/// the reflection JSON (see `emit_push_struct`).
const PUSH_SIZE_FALLBACK: &[(&str, u32)] = &[
    ("overlay", 0),
    // LightingPushConstants: 4×u32 + 4×f32 = 32 字节
    ("lighting", 32),
    // SharcQueryPushConstants: 7 fields padded to 48.
    ("sharc_query", 48),
];

fn fallback_push_size(shader: &str) -> u32 {
    PUSH_SIZE_FALLBACK
        .iter()
        .find(|(s, _)| *s == shader)
        .map(|(_, sz)| *sz)
        .unwrap_or(0)
}

impl BindKind {
    fn descriptor_type(&self) -> Option<&'static str> {
        match self {
            BindKind::UniformBuffer => Some("UNIFORM_BUFFER"),
            BindKind::CombinedImageSampler => Some("COMBINED_IMAGE_SAMPLER"),
            _ => None,
        }
    }
}

fn classify(p: &Parameter) -> Option<ResolvedBinding> {
    let b = p.binding.as_ref()?;
    // Only 发射 descriptor-set slots and push-constant buffers. Slang also
    // reflects vertex-shader `in` parameters (kind "vertexInput" / no
    // 描述符 绑定 — those are not Vulkan 描述符 bindings and must
    // be skipped.
    let kind = match b.kind.as_str() {
        "pushConstantBuffer" | "pushConstant" => BindKind::PushConstant {
            size: b.size.unwrap_or(0),
        },
        "descriptorTableSlot" | "uniform" => {
            // Distinguish UBO vs 纹理 via the 类型 shape.
            let shape =
                p.ty.as_ref()
                    .and_then(|t| t.base_shape.clone().or_else(|| t.kind.clone()))
                    .unwrap_or_default();
            if shape.contains("texture")
                || shape.contains("Texture")
                || shape.contains("sampler")
                || shape.contains("Sampler")
                || shape.contains("resource")
            {
                BindKind::CombinedImageSampler
            } else {
                BindKind::UniformBuffer
            }
        }
        // 顶点 inputs, 阶段 inputs, etc. — not 描述符 bindings
        _ => return None,
    };
    Some(ResolvedBinding {
        name: p.name.clone(),
        set: b.space,
        binding: b.index,
        kind,
    })
}

fn to_screaming_snake(s: &str) -> String {
    let mut out = String::new();
    let mut prev_lower = false;
    for c in s.chars() {
        if c.is_ascii_uppercase() && prev_lower {
            out.push('_');
        }
        if c == '-' || c == ' ' {
            out.push('_');
            prev_lower = false;
            continue;
        }
        out.push(c.to_ascii_uppercase());
        prev_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
    }
    out
}

fn process_file(path: &Path) -> Result<(String, String)> {
    let raw = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let refl: Reflection = serde_json::from_str(&raw)
        .with_context(|| format!("parse reflection JSON {}", path.display()))?;

    let shader = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("shader")
        .to_string();
    let mod_name = shader.replace('-', "_");

    // Gather 全局 params + entry-point params.
    let mut resolved: Vec<ResolvedBinding> = Vec::new();
    for p in &refl.parameters {
        if let Some(r) = classify(p) {
            resolved.push(r);
        }
    }
    for ep in &refl.entry_points {
        for p in &ep.parameters {
            if let Some(r) = classify(p) {
                resolved.push(r);
            }
        }
    }
    // Dedup by 集合 绑定 name).
    resolved.sort_by(|a, b| (a.set, a.binding, &a.name).cmp(&(b.set, b.binding, &b.name)));
    resolved.dedup_by(|a, b| a.set == b.set && a.binding == b.binding && a.name == b.name);

    let mut out = String::new();
    // File header
    out.push_str("// @generated by xtask/shader-bindgen from Slang reflection.\n");
    out.push_str("// DO NOT EDIT. Regenerate: `cd xtask && cargo run --bin shader-bindgen -- \\\n");
    out.push_str("//   ../shaders/reflection ../crates/prism-render/src/shader_bindings`\n");
    out.push_str(&format!(
        "\n//! Bindings reflected from `shaders/slang/{shader}.slang`.\n\n"
    ));
    out.push_str("#![allow(dead_code, non_snake_case)]\n\n");

    // Entry points.
    if !refl.entry_points.is_empty() {
        out.push_str("\n/// Entry point names (for VkPipelineShaderStageCreateInfo).\n");
        for ep in &refl.entry_points {
            let stage = ep.stage.clone().unwrap_or_default().to_uppercase();
            let cname = to_screaming_snake(&ep.name);
            out.push_str(&format!(
                "pub const ENTRY_{cname}: &str = \"{}\"; // stage: {stage}\n",
                ep.name
            ));
        }
    }

    // 描述符 bindings grouped by 集合
    let mut by_set: BTreeMap<u32, Vec<&ResolvedBinding>> = BTreeMap::new();
    let mut push_size: Option<u32> = None;
    for r in &resolved {
        match &r.kind {
            BindKind::PushConstant { size } => {
                // Real slangc omits 大小 for some shaders; fall 后 to the
                // known Rust-side 布局 (see pbr_push.rs + its tests).
                let sz = *size.max(&fallback_push_size(&shader));
                push_size = Some(push_size.unwrap_or(0).max(sz));
            }
            _ => by_set.entry(r.set).or_default().push(r),
        }
    }

    if let Some(sz) = push_size {
        out.push_str(&format!(
            "\n/// Push-constant block size in bytes (reflected).\npub const PUSH_CONSTANT_SIZE: u32 = {sz};\n"
        ));
    }

    for (set, binds) in &by_set {
        out.push_str(&format!("\n// --- descriptor set {set} ---\n"));
        for r in binds {
            let cname = to_screaming_snake(&r.name);
            let dtype = r.kind.descriptor_type().unwrap_or("/* unknown */");
            out.push_str(&format!(
                "pub const {cname}_SET: u32 = {};\npub const {cname}_BINDING: u32 = {}; // {dtype}\n",
                r.set, r.binding
            ));
        }
    }

    // --- Auto-generated push-constant 结构体 definitions ---
    // Scan 全局 and entry-point parameters for push-constant buffers whose
    // types carry 结构体 field 信息 发射 a #[repr(C)] Rust 结构体 for each.
    // This replaces the hand-written push-constant structs in the engine.
    for p in &refl.parameters {
        if let Some(ety) = p
            .binding
            .as_ref()
            .filter(|b| b.kind == "pushConstantBuffer" || b.kind == "pushConstant")
            .and(p.ty.as_ref())
            .and_then(|t| t.element_type.as_ref())
            .filter(|e| e.fields.is_some())
        {
            let struct_name = ety.name.as_deref().unwrap_or("PushConstants");
            if let Some(ref fields) = ety.fields {
                out.push_str(&emit_push_struct(struct_name, fields));
            }
        }
    }
    // Same for entry-point parameters (some shaders declare 推送 constants
    // at the entry-point level instead of globally).
    for ep in &refl.entry_points {
        for p in &ep.parameters {
            if let Some(ety) = p
                .binding
                .as_ref()
                .filter(|b| b.kind == "pushConstantBuffer" || b.kind == "pushConstant")
                .and(p.ty.as_ref())
                .and_then(|t| t.element_type.as_ref())
                .filter(|e| e.fields.is_some())
            {
                let struct_name = ety.name.as_deref().unwrap_or("PushConstants");
                if let Some(ref fields) = ety.fields {
                    out.push_str(&emit_push_struct(struct_name, fields));
                }
            }
        }
    }

    Ok((mod_name, out))
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let in_dir = PathBuf::from(
        args.next()
            .context("usage: shader-bindgen <reflection_dir> <out_dir>")?,
    );
    let out_dir = PathBuf::from(
        args.next()
            .context("usage: shader-bindgen <reflection_dir> <out_dir>")?,
    );

    let mut files: Vec<PathBuf> = std::fs::read_dir(&in_dir)
        .with_context(|| format!("read dir {}", in_dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|e| e == "json").unwrap_or(false))
        .collect();
    files.sort();

    if files.is_empty() {
        anyhow::bail!(
            "no reflection JSON found in {} — run assets/shaders/compile.sh first",
            in_dir.display()
        );
    }

    std::fs::create_dir_all(&out_dir).with_context(|| format!("create {}", out_dir.display()))?;

    let mut mod_rs = String::new();
    mod_rs.push_str("// @generated by xtask/shader-bindgen from Slang reflection.\n");
    mod_rs.push_str(
        "// DO NOT EDIT. Regenerate: `cd crates/xtask && cargo run --bin shader-bindgen -- \\\\\n",
    );
    mod_rs.push_str(
        "//   ../../assets/shaders/reflection ../../crates/prism-render/src/shader_bindings`\n",
    );
    mod_rs.push_str("#![allow(dead_code, non_snake_case)]\n\n");

    for f in &files {
        let (mod_name, content) = process_file(f)?;
        // 写入 per-shader file: {out_dir}/{mod_name}.rs
        let mod_path = out_dir.join(format!("{mod_name}.rs"));
        std::fs::write(&mod_path, &content)
            .with_context(|| format!("write {}", mod_path.display()))?;
        // Add to mod.rs
        mod_rs.push_str(&format!("pub mod {mod_name};\n"));
    }

    // 写入 mod.rs
    let mod_rs_path = out_dir.join("mod.rs");
    std::fs::write(&mod_rs_path, &mod_rs)
        .with_context(|| format!("write {}", mod_rs_path.display()))?;

    println!("wrote {} module(s) to {}", files.len(), out_dir.display());
    Ok(())
}
