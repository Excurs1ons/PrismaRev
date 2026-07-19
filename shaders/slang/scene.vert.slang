// Vertex stage for the RenderGraph forward scene pass.
//
// Pairs with scene.frag.slang to form the full scene pipeline. The push-constant
// block (ScenePush) matches scene.frag.slang byte-for-byte so both stages share
// the same pipeline layout.
//
// Bindings:
//   set 0, binding 0 — per-frame UBO (FrameUBO)
// Push constants: model (mat4) + lightViewProj (mat4) + debug (vec4).
import common;

// set 0, binding 0 — shared per-frame UBO.
[[vk::binding(0, 0)]]
ConstantBuffer<FrameUBO> frame;

// Push constant: must match scene.frag.slang's ScenePush exactly (144 bytes)
// so the VkPipelineLayout is shared. lightViewProj and debug are unused here
// but required for size parity.
struct ScenePush
{
    float4x4 model;
    float4x4 lightViewProj;
    float4   debug;
};
[[vk::push_constant]]
ScenePush pc;

[shader("vertex")]
MeshVaryings vertexMain(MeshVertexIn in)
{
    MeshVaryings out;
    float4 worldPos = mul(pc.model, float4(in.position, 1.0));
    out.worldPos    = worldPos.xyz;
    // Normal transform via mat3(model) — assumes uniform scale.
    float3x3 normalMat = float3x3(pc.model);
    out.worldNormal = normalize(mul(normalMat, in.normal));
    out.color       = in.color;
    out.uv          = in.uv;
    out.tangent     = in.tangent;
    out.clipPos     = mul(frame.viewProj, worldPos);
    return out;
}