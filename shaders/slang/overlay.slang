// Overlay (bitmap-font HUD) vertex + fragment.
// Equivalent to shaders/overlay.vert + shaders/overlay.frag.
//   slangc overlay.slang -profile spirv_1_5 -target spirv \
//          -entry vertexMain -entry fragmentMain   (see compile.sh)

struct OverlayVertexIn
{
    float2 pos   : POSITION;   // location 0 — clip space, already in [-1, 1]
    float2 uv    : TEXCOORD0;  // location 1 — atlas uv
    float4 color : COLOR0;     // location 2 — rgba
};

struct OverlayVaryings
{
    float4 clipPos : SV_Position;
    float2 uv      : TEXCOORD0; // location 0
    float4 color   : COLOR0;    // location 1
};

// set 0, binding 0 — font atlas (combined image sampler, matches the Rust
// COMBINED_IMAGE_SAMPLER descriptor and GLSL `sampler2D fontTex`).
[[vk::binding(0, 0)]]
Sampler2D fontTex;

[shader("vertex")]
OverlayVaryings vertexMain(OverlayVertexIn in)
{
    OverlayVaryings out;
    out.uv      = in.uv;
    out.color   = in.color;
    out.clipPos = float4(in.pos, 0.0, 1.0);
    return out;
}

[shader("fragment")]
float4 fragmentMain(OverlayVaryings in) : SV_Target
{
    float a = fontTex.Sample(in.uv).a;
    return float4(in.color.rgb, in.color.a * a);
}
