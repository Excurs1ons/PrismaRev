// Gizmo (world-space XYZ axis triad) vertex + fragment.
// Equivalent to shaders/gizmo.vert + shaders/gizmo.frag.
//   slangc gizmo.slang -profile spirv_1_5 -target spirv \
//          -entry vertexMain -entry fragmentMain   (see compile.sh)

struct GizmoVertexIn
{
    float3 position : POSITION; // location 0
    float3 color    : COLOR0;   // location 1
};

struct GizmoVaryings
{
    float4 clipPos : SV_Position;
    float3 color   : COLOR0;    // location 0
};

struct GizmoPush
{
    float4x4 viewProj;
    float4x4 model;     // entity world transform * component scale
};
[[vk::push_constant]]
GizmoPush push;

[shader("vertex")]
GizmoVaryings vertexMain(GizmoVertexIn in)
{
    GizmoVaryings out;
    out.color   = in.color;
    float4 worldPos = mul(push.model, float4(in.position, 1.0));
    out.clipPos = mul(push.viewProj, worldPos);
    return out;
}

[shader("fragment")]
float4 fragmentMain(GizmoVaryings in) : SV_Target
{
    return float4(in.color, 1.0);
}
