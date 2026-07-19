// Mesh vertex stage + Blinn-Phong fragment stage.
//
// Equivalent to shaders/mesh.vert + shaders/mesh.frag (GLSL). Compile with:
//   slangc mesh.slang -profile spirv_1_5 -target spirv \
//          -entry vertexMain -entry fragmentMain -o mesh.spv
// or per-stage to mesh.vert.spv / mesh.frag.spv (see compile.sh).
import common;

// set 0, binding 0 — shared per-frame UBO.
[[vk::binding(0, 0)]]
ConstantBuffer<FrameUBO> frame;

// Push constant: model matrix (mat4, 64 bytes).
struct MeshPush
{
    float4x4 model;
};
[[vk::push_constant]]
MeshPush push;

[shader("vertex")]
MeshVaryings vertexMain(MeshVertexIn in)
{
    MeshVaryings out;
    float4 worldPos = mul(push.model, float4(in.position, 1.0));
    out.worldPos    = worldPos.xyz;
    // Normal transform via mat3(model) — assumes uniform scale.
    float3x3 normalMat = float3x3(push.model);
    out.worldNormal = normalize(mul(normalMat, in.normal));
    out.color       = in.color;
    out.uv          = in.uv;
    out.tangent     = in.tangent;
    out.clipPos     = mul(frame.viewProj, worldPos);
    return out;
}

static const float SHININESS = 32.0;

[shader("fragment")]
float4 fragmentMain(MeshVaryings in) : SV_Target
{
    float3 N = normalize(in.worldNormal);
    float3 L = normalize(frame.lightDirection.xyz);
    float3 V = normalize(frame.cameraPosition.xyz - in.worldPos);
    float3 H = normalize(L + V);

    float intensity      = frame.lightDirection.w;
    float ambient_factor = frame.lightColor.w;

    float3 ambient = ambient_factor * in.color;

    float NdotL   = max(dot(N, L), 0.0);
    float3 diffuse = NdotL * frame.lightColor.rgb * in.color * intensity;

    float NdotH    = max(dot(N, H), 0.0);
    float3 specular = pow(NdotH, SHININESS) * frame.lightColor.rgb * intensity;

    return float4(ambient + diffuse + specular, 1.0);
}
