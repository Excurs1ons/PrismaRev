// Post-process pass - fullscreen-triangle tonemap (Reinhard / ACES) from the
// offscreen HDR color attachment into the sRGB swapchain.
//
// The ScenePass renders the lit scene into an R16G16B16A16_SFLOAT HDR image;
// this pass samples that image, applies the selectable tonemap operator, and
// writes the result to the swapchain (an sRGB format, so the hardware encodes
// the linear output to sRGB on write).
//
// Sets (must match `PostPass::ensure_pipeline`):
//   set 0, binding 0 - HDR color texture (combined Sampler2D)
//
// Push constants: PostPush (4 bytes) - `tonemapMode` (0 = Reinhard, 1 = ACES).
//
// Compile:
//   slangc post.slang -profile spirv_1_5 -target spirv \
//          -entry vertexMain -stage vertex   -fvk-use-entrypoint-name -o post.vert.spv
//   slangc post.slang -profile spirv_1_5 -target spirv \
//          -entry fragmentMain -stage fragment -fvk-use-entrypoint-name -o post.frag.spv

// ---- set 0, binding 0 - HDR color attachment (combined image+sampler).
[[vk::binding(0, 0)]]
Sampler2D hdrTex;

// Push constants: tonemap operator selector (matches `tonemapMode` in FrameUBO).
public struct PostPush
{
    public uint tonemapMode;
};
[[vk::push_constant]]
PostPush pc;

// ACES filmic tonemap (Narkowicz 2016) - self-contained copy of the operator
// used in scene_frag.slang.
float3 aces(float3 x)
{
    const float a = 2.51, b = 0.03, c = 2.43, d = 0.59, e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), 0.0, 1.0);
}

[shader("vertex")]
public void vertexMain(
    uint vid : SV_VertexID,
    out float4 clipPos : SV_Position,
    out float2 uv : TEXCOORD0
)
{
    // Fullscreen triangle covering NDC [-1,1]^2. The three vertices are
    // (-1,-1), (3,-1), (-1,3); the clipped region is the whole screen. No
    // vertex buffer is bound - the corner is selected by `vid`.
    float2 pos = float2(
        (vid == 1u) ? 3.0 : -1.0,
        (vid == 2u) ? 3.0 : -1.0
    );
    clipPos = float4(pos, 0.0, 1.0);
    // Vulkan NDC y = -1 is the top; UV.y = 0 at the top. Match the framebuffer
    // top-left origin so the HDR image keeps the ScenePass orientation.
    uv = pos * 0.5 + 0.5;
}

[shader("fragment")]
public void fragmentMain(
    float4 clipPos : SV_Position,
    float2 uv : TEXCOORD0,
    out float4 outColor : SV_Target0
)
{
    float3 hdr = hdrTex.Sample(uv).rgb;
    if (pc.tonemapMode == 1u) {
        hdr = aces(hdr);
    } else {
        hdr = hdr / (hdr + 1.0);
    }
    outColor = float4(hdr, 1.0);
}
