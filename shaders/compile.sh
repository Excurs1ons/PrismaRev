#!/usr/bin/env bash
# Compile PrismaRev Slang shaders to SPIR-V + emit reflection JSON.
#
# This runs on a DESKTOP / CI host (Windows/Linux/macOS x86_64), NOT on the
# Termux/aarch64 device — the official slangc prebuilts are glibc/MSVC binaries.
# On Android the engine ships the pre-compiled .spv produced here.
#
# Output per stage:
#   <name>.<stage>.spv          — SPIR-V for the Vulkan pipeline
#   reflection/<name>.json      — slang reflection (drives Rust binding codegen)
#
# Requires `slangc` on PATH (from a Slang release), or set SLANGC=/path/to/slangc.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC="$HERE/slang"
OUT="$HERE"                      # .spv land next to the existing GLSL .spv
REFL="$HERE/reflection"
SLANGC="${SLANGC:-slangc}"
PROFILE="${SLANG_PROFILE:-spirv_1_5}"

if ! command -v "$SLANGC" >/dev/null 2>&1 && [ ! -x "$SLANGC" ]; then
  echo "ERROR: slangc not found. Install a Slang release or set SLANGC=/path/to/slangc" >&2
  echo "  e.g. tools/slang/bin/slangc (desktop only — glibc binary won't run under Termux)" >&2
  exit 1
fi

mkdir -p "$REFL"

# name : entry:stage pairs (space separated). stage = vert|frag (matches the
# .spv filenames the engine include_bytes!'s in renderer.rs / pbr_push.rs, e.g.
# mesh.vert.spv / mesh.frag.spv).
# Slang entry points are vertexMain / fragmentMain (see the [shader(...)] attrs).
compile_stage() {
  local name="$1" entry="$2" stage="$3"
  # Map slang stage names (vertex/fragment) to the file extension the engine
  # expects (vert/frag), so the generated .spv matches include_bytes! paths.
  case "$stage" in
    vertex)   ext=vert ;;
    fragment) ext=frag ;;
    *)        ext="$stage" ;;
  esac
  local out_spv="$OUT/${name}.${ext}.spv"
  echo "  $name :: $entry -> ${name}.${ext}.spv"
  "$SLANGC" "$SRC/${name}.slang" \
    -profile "$PROFILE" \
    -target spirv \
    -entry "$entry" \
    -stage "$stage" \
    -fvk-use-entrypoint-name \
    -o "$out_spv"
}

emit_reflection() {
  # One reflection JSON per module (covers all entry points + bindings).
  local name="$1"; shift
  local entries=()
  while [ "$#" -gt 0 ]; do entries+=(-entry "$1" -stage "$2"); shift 2; done
  echo "  reflect $name -> reflection/${name}.json"
  # slangc requires an SPIR-V output path even for reflection-only runs.
  # Write to a throwaway file and delete it immediately so we don't leave
  # .tmp.spv litter in reflection/ (which would pollute git status).
  # (We can't use /dev/null: on Windows bash that maps to a literal "nul"
  # path slangc can't open.)
  local tmp="$REFL/${name}.tmp.spv"
  "$SLANGC" "$SRC/${name}.slang" \
    -profile "$PROFILE" \
    -target spirv \
    "${entries[@]}" \
    -reflection-json "$REFL/${name}.json" \
    -o "$tmp"
  rm -f "$tmp"
}

# fix_spv <file.spv>
# Slang emits an illegal `ArrayStride` decoration on UniformConstant runtime
# arrays of opaque types (sampled images / samplers) used for bindless
# descriptor indexing. SPIR-V validation rejects explicit layout decorations on
# such runtime arrays (VUID-StandaloneSpirv-None-10684), so the module fails to
# load under the validation layer. fix_spirv.py strips only those illegal
# decorations. Safe to run on any .spv; it leaves legal strides untouched.
fix_spv() {
  local spv="$1"
  if command -v python3 >/dev/null 2>&1; then
    python3 "$HERE/fix_spirv.py" "$spv" "$spv"
  else
    echo "  WARNING: python3 not found; skipping ArrayStride fix for $spv" >&2
  fi
}

echo "Compiling Slang shaders (slangc = $SLANGC, profile = $PROFILE)..."

# mesh: vertex + fragment (Blinn-Phong)
compile_stage mesh vertexMain vertex
compile_stage mesh fragmentMain fragment
emit_reflection mesh vertexMain vertex fragmentMain fragment

# pbr: fragment only (reuses mesh.slang vertex stage at pipeline level)
compile_stage pbr fragmentMain fragment
emit_reflection pbr fragmentMain fragment

# gizmo: vertex + fragment
compile_stage gizmo vertexMain vertex
compile_stage gizmo fragmentMain fragment
emit_reflection gizmo vertexMain vertex fragmentMain fragment

# overlay: vertex + fragment
compile_stage overlay vertexMain vertex
compile_stage overlay fragmentMain fragment
emit_reflection overlay vertexMain vertex fragmentMain fragment

# shadow: compute (RayQuery inline shadow pass, half-res)
SHADOW_ENTRY="computeMain"
SHADOW_STAGE="compute"
echo "  shadow :: ${SHADOW_ENTRY} -> shadow.comp.spv"
"$SLANGC" "$SRC/shadow.slang" \
  -profile "$PROFILE" \
  -target spirv \
  -entry "$SHADOW_ENTRY" \
  -stage "$SHADOW_STAGE" \
  -fvk-use-entrypoint-name \
  -o "$OUT/shadow.comp.spv"
echo "  reflect shadow -> reflection/shadow.json"
"$SLANGC" "$SRC/shadow.slang" \
  -profile "$PROFILE" \
  -target spirv \
  -entry "$SHADOW_ENTRY" \
  -stage "$SHADOW_STAGE" \
  -reflection-json "$REFL/shadow.json" \
  -o "$REFL/shadow.tmp.spv"
rm -f "$REFL/shadow.tmp.spv"

# shadowmap: vertex + fragment (rasterized depth-only shadow map fallback)
compile_stage shadowmap vertexMain vertex
compile_stage shadowmap fragmentMain fragment

# scene: vertex + fragment (forward PBR + IBL RenderGraph path).
# These are separate source files (scene.vert.slang / scene.frag.slang),
# not a single scene.slang, so compile them explicitly.
echo "  scene :: vertexMain -> scene.vert.spv"
"$SLANGC" "$SRC/scene.vert.slang" \
  -profile "$PROFILE" -target spirv -entry vertexMain -stage vertex \
  -fvk-use-entrypoint-name -o "$OUT/scene.vert.spv"
echo "  scene :: fragmentMain -> scene.frag.spv"
"$SLANGC" "$SRC/scene.frag.slang" \
  -profile "$PROFILE" -target spirv -entry fragmentMain -stage fragment \
  -fvk-use-entrypoint-name -o "$OUT/scene.frag.spv"

# skybox: vertex + fragment (environment cubemap background).
# Single source file with two entry points (vertexMain / fragmentMain).
echo "  skybox :: vertexMain -> skybox.vert.spv"
"$SLANGC" "$SRC/skybox.slang" \
  -profile "$PROFILE" -target spirv -entry vertexMain -stage vertex \
  -fvk-use-entrypoint-name -o "$OUT/skybox.vert.spv"
echo "  skybox :: fragmentMain -> skybox.frag.spv"
"$SLANGC" "$SRC/skybox.slang" \
  -profile "$PROFILE" -target spirv -entry fragmentMain -stage fragment \
  -fvk-use-entrypoint-name -o "$OUT/skybox.frag.spv"
echo "  reflect skybox -> reflection/skybox.json"
"$SLANGC" "$SRC/skybox.slang" \
  -profile "$PROFILE" -target spirv \
  -entry vertexMain -stage vertex -entry fragmentMain -stage fragment \
  -reflection-json "$REFL/skybox.json" \
  -o "$REFL/skybox.tmp.spv"
rm -f "$REFL/skybox.tmp.spv"

# sharc_query: compute (SHARC GI cache lookup, half-res)
SHARCQ_ENTRY="computeMain"
SHARCQ_STAGE="compute"
echo "  sharc_query :: ${SHARCQ_ENTRY} -> sharc_query.comp.spv"
"$SLANGC" "$SRC/sharc_query.slang" \
  -profile "$PROFILE" \
  -target spirv \
  -entry "$SHARCQ_ENTRY" \
  -stage "$SHARCQ_STAGE" \
  -I "$SRC" \
  -fvk-use-entrypoint-name \
  -o "$OUT/sharc_query.comp.spv"
echo "  reflect sharc_query -> reflection/sharc_query.json"
"$SLANGC" "$SRC/sharc_query.slang" \
  -profile "$PROFILE" \
  -target spirv \
  -entry "$SHARCQ_ENTRY" \
  -stage "$SHARCQ_STAGE" \
  -I "$SRC" \
  -reflection-json "$REFL/sharc_query.json" \
  -o "$REFL/sharc_query.tmp.spv"
rm -f "$REFL/sharc_query.tmp.spv"

# lighting: fragment (GBuffer + shadow + GI + IBL → HDR)
compile_stage lighting fragmentMain fragment
emit_reflection lighting fragmentMain fragment

# post: fragment (ACES tone map → swapchain)
compile_stage post fragmentMain fragment
emit_reflection post fragmentMain fragment

# bindless: fragment only. Pairs with mesh.vert.spv (from mesh.slang vertex)
# at pipeline-build time to form the bindless PBR draw pipeline.
compile_stage bindless fragmentMain fragment
fix_spv "$OUT/bindless.frag.spv"
emit_reflection bindless fragmentMain fragment

# scene_bindless: fragment only (RenderGraph ScenePass bindless PBR +
# rasterized shadow map). Pairs with mesh.vert.spv. Graph-path counterpart
# of bindless.slang with shadow-map sampling; lightViewProj is read from
# the per-frame UBO (not push constants) so the push constant stays under
# the 128-byte Vulkan limit.
compile_stage scene_bindless fragmentMain fragment
fix_spv "$OUT/scene_bindless.frag.spv"

echo "All Slang shaders compiled. SPIR-V in $OUT, reflection JSON in $REFL"
