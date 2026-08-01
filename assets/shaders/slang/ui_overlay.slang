// UI overlay — screen-space colored quads + textured glyph quads drawn on
// top of the final scene.
//
// Two quad kinds share one pipeline:
//   - plain colored fills (backgrounds): uv = (0,0) → samples the atlas
//     reserve pixel, which is white, so alpha = 1 and the vertex color
//     passes through;
//   - glyph quads (text): uv spans the glyph's rect in the R8 glyph atlas
//     (fontdue-rasterized); the sampled alpha modulates the vertex color.
//
// Sets (must match `UiOverlay::create_pipeline`):
//   set 0, binding 0 - font glyph atlas (R8 UNORM, combined image sampler)
//
// Compile:
//   slangc ui_overlay.slang -profile spirv_1_5 -target spirv \
//          -entry vertexMain -stage vertex   -fvk-use-entrypoint-name -o ui_overlay.vert.spv
//   slangc ui_overlay.slang -profile spirv_1_5 -target spirv \
//          -entry fragmentMain -stage fragment -fvk-use-entrypoint-name -o ui_overlay.frag.spv

// ---- set 0, binding 0 - glyph atlas (combined image+sampler). Pixel (0,0)
// is reserved as white so untextured quads (uv = 0) sample 1.0.
[[vk::binding(0, 0)]]
Sampler2D glyphAtlas;

struct UiVertexIn
{
    float2 position : POSITION;  // location 0 — NDC position
    float2 uv       : TEXCOORD0; // location 1 — atlas uv (0,0 for plain quads)
    float4 color    : COLOR0;    // location 2 — RGBA color
};

struct UiVaryings
{
    float4 clipPos : SV_Position;
    float2 uv      : TEXCOORD0;  // location 0 — interpolated uv
    float4 color   : COLOR0;     // location 1 — interpolated color
};

[shader("vertex")]
UiVaryings vertexMain(UiVertexIn in)
{
    UiVaryings out;
    out.clipPos = float4(in.position, 0.0, 1.0);
    out.uv      = in.uv;
    out.color   = in.color;
    return out;
}

[shader("fragment")]
float4 fragmentMain(UiVaryings in) : SV_Target0
{
    // Glyph alpha modulates the vertex color (plain quads sample the white
    // reserve pixel → alpha = 1). Alpha-weighted rgb for correct blending
    // with the SRC_ALPHA/ONE_MINUS_SRC_ALPHA state.
    float a = glyphAtlas.Sample(in.uv).r;
    return float4(in.color.rgb * a, in.color.a * a);
}
