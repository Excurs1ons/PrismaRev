// Rasterized shadow map — depth-only pass (the rasterized fallback for the
// hybrid adaptive shadow system described in docs/DESIGN.md §2.3).
//
// When `VK_KHR_ray_query` is unavailable the renderer falls back to a classic
// depth-only shadow map: the scene is rendered from the light's point of view
// into a depth texture, which the lighting pass samples (with a comparison
// sampler) to decide lit vs shadowed.
//
// This module holds both stages:
//   vertexMain   — transform by model then light-space view-projection
//   fragmentMain — empty (the pipeline has zero color attachments)
//
// Compile (see compile.sh):
//   slangc shadowmap.slang -profile spirv_1_5 -target spirv \
//          -entry vertexMain   -stage vertex   -o shadowmap.vert.spv
//   slangc shadowmap.slang -profile spirv_1_5 -target spirv \
//          -entry fragmentMain -stage fragment -o shadowmap.frag.spv
import common;

// Push constant: model (per-instance world transform) + light-space
// view-projection. 128 bytes total (two mat4). No UBO needed.
struct ShadowPush
{
    float4x4 model;
    float4x4 lightViewProj;
};
[[vk::push_constant]]
ShadowPush push;

[shader("vertex")]
float4 vertexMain(MeshVertexIn in) : SV_Position
{
    float4 worldPos = mul(push.model, float4(in.position, 1.0));
    return mul(push.lightViewProj, worldPos);
}

[shader("fragment")]
void fragmentMain()
{
    // Depth-only: the rasterizer writes depth from clipPos; no color output.
}
