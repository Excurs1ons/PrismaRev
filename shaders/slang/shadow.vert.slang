// Shadow map depth-only vertex stage.
//
// Transforms each vertex by the per-instance model matrix, then by the
// light-space view-projection, writing only the clip-space position. The
// matching fragment stage (shadow.frag) is empty: this is a depth-only pass
// (the graphics pipeline is created with `color_attachment_count = 0`), so we
// only need the rasterizer's depth write — no color output.
//
// Compile:
//   slangc shadow.vert.slang -profile spirv_1_5 -target spirv \
//          -entry vertexMain -stage vertex -o shadow.vert.spv
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
