//! Slang reflection -> Rust binding codegen for PrismaRev.
//!
//! Reads the `reflection/*.json` emitted by `slangc -reflection-json`
//! (see `shaders/compile.sh`) and generates a Rust module describing each
//! shader's resource bindings: descriptor set/binding indices, resource kinds,
//! and push-constant sizes. The generated file is committed to the repo so the
//! engine builds on hosts without slangc (Termux/Android).
//!
//! Run on a desktop/CI host after recompiling shaders:
//!   cargo run -p xtask --bin shader-bindgen -- \
//!     shaders/reflection crates/prism-render/src/shader_bindings.rs
//!
//! This is intentionally a standalone tool (NOT a build.rs) so the normal
//! `cargo build` never needs slangc.

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
    /// e.g. "descriptorTableSlot", "pushConstantBuffer", "uniform".
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
}

#[derive(Debug, Deserialize)]
struct EntryPoint {
    name: String,
    #[serde(default)]
    stage: Option<String>,
    #[serde(default)]
    parameters: Vec<Parameter>,
}

/// A resolved binding fact we care about for Rust codegen.
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

/// Fallback push-constant sizes for shaders whose slangc reflection omits the
/// `size` field on the `pushConstantBuffer` parameter (mesh/pbr/gizmo do;
/// bindless includes it). These mirror the `#[repr(C)]` structs in
/// `pbr_push.rs` — the source of truth is the Rust layout, verified by tests.
const PUSH_SIZE_FALLBACK: &[(&str, u32)] = &[
    ("mesh", 64),
    ("pbr", 92),
    ("gizmo", 64),
    ("bindless", 96),
    ("overlay", 0),
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
    // Only emit descriptor-set slots and push-constant buffers. Slang also
    // reflects vertex-shader `in` parameters (kind "vertexInput" / no
    // descriptor binding) — those are not Vulkan descriptor bindings and must
    // be skipped.
    let kind = match b.kind.as_str() {
        "pushConstantBuffer" | "pushConstant" => BindKind::PushConstant {
            size: b.size.unwrap_or(0),
        },
        "descriptorTableSlot" | "uniform" => {
            // Distinguish UBO vs texture via the type shape.
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
        // vertex inputs, stage inputs, etc. — not descriptor bindings
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

fn process_file(path: &Path, out: &mut String) -> Result<()> {
    let raw = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let refl: Reflection = serde_json::from_str(&raw)
        .with_context(|| format!("parse reflection JSON {}", path.display()))?;

    let shader = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("shader")
        .to_string();
    let mod_name = shader.replace('-', "_");

    // Gather global params + entry-point params.
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
    // Dedup by (set, binding, name).
    resolved.sort_by(|a, b| (a.set, a.binding, &a.name).cmp(&(b.set, b.binding, &b.name)));
    resolved.dedup_by(|a, b| a.set == b.set && a.binding == b.binding && a.name == b.name);

    out.push_str(&format!("\npub mod {mod_name} {{\n"));
    out.push_str(&format!(
        "    //! Bindings reflected from shaders/slang/{shader}.slang\n"
    ));

    // Entry points.
    if !refl.entry_points.is_empty() {
        out.push_str("\n    /// Entry point names (for VkPipelineShaderStageCreateInfo).\n");
        for ep in &refl.entry_points {
            let stage = ep.stage.clone().unwrap_or_default().to_uppercase();
            let cname = to_screaming_snake(&ep.name);
            out.push_str(&format!(
                "    pub const ENTRY_{cname}: &str = \"{}\"; // stage: {stage}\n",
                ep.name
            ));
        }
    }

    // Descriptor bindings grouped by set.
    let mut by_set: BTreeMap<u32, Vec<&ResolvedBinding>> = BTreeMap::new();
    let mut push_size: Option<u32> = None;
    for r in &resolved {
        match &r.kind {
            BindKind::PushConstant { size } => {
                // Real slangc omits `size` for some shaders; fall back to the
                // known Rust-side layout (see pbr_push.rs + its tests).
                let sz = *size.max(&fallback_push_size(&shader));
                push_size = Some(push_size.unwrap_or(0).max(sz));
            }
            _ => by_set.entry(r.set).or_default().push(r),
        }
    }

    if let Some(sz) = push_size {
        out.push_str(&format!(
            "\n    /// Push-constant block size in bytes (reflected).\n    pub const PUSH_CONSTANT_SIZE: u32 = {sz};\n"
        ));
    }

    for (set, binds) in &by_set {
        out.push_str(&format!("\n    // --- descriptor set {set} ---\n"));
        for r in binds {
            let cname = to_screaming_snake(&r.name);
            let dtype = r.kind.descriptor_type().unwrap_or("/* unknown */");
            out.push_str(&format!(
                "    pub const {cname}_SET: u32 = {};\n    pub const {cname}_BINDING: u32 = {}; // {dtype}\n",
                r.set, r.binding
            ));
        }
    }

    out.push_str("}\n");
    Ok(())
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let in_dir = PathBuf::from(
        args.next()
            .context("usage: shader-bindgen <reflection_dir> <out_rs>")?,
    );
    let out_path = PathBuf::from(
        args.next()
            .context("usage: shader-bindgen <reflection_dir> <out_rs>")?,
    );

    let mut out = String::new();
    out.push_str("// @generated by xtask/shader-bindgen from Slang reflection JSON.\n");
    out.push_str("// DO NOT EDIT. Regenerate: cargo run -p xtask --bin shader-bindgen -- \\\n");
    out.push_str("//   shaders/reflection crates/prism-render/src/shader_bindings.rs\n");
    out.push_str("#![allow(dead_code)]\n");

    let mut files: Vec<PathBuf> = std::fs::read_dir(&in_dir)
        .with_context(|| format!("read dir {}", in_dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|e| e == "json").unwrap_or(false))
        .collect();
    files.sort();

    if files.is_empty() {
        anyhow::bail!(
            "no reflection JSON found in {} — run shaders/compile.sh first",
            in_dir.display()
        );
    }

    for f in &files {
        process_file(f, &mut out)?;
    }

    std::fs::write(&out_path, &out).with_context(|| format!("write {}", out_path.display()))?;
    println!(
        "wrote {} ({} shader module(s))",
        out_path.display(),
        files.len()
    );
    Ok(())
}
