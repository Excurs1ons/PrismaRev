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
  "$SLANGC" "$SRC/${name}.slang" \
    -profile "$PROFILE" \
    -target spirv \
    "${entries[@]}" \
    -reflection-json "$REFL/${name}.json" \
    -o /dev/null 2>/dev/null || \
  "$SLANGC" "$SRC/${name}.slang" \
    -profile "$PROFILE" \
    -target spirv \
    "${entries[@]}" \
    -reflection-json "$REFL/${name}.json" \
    -o "$REFL/${name}.tmp.spv"
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

echo "All Slang shaders compiled. SPIR-V in $OUT, reflection JSON in $REFL"
