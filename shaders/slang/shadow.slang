// RayQuery shadow pass — inline ray traversal against TLAS for soft shadows.
//
// Runs as a compute shader at half resolution by default (configurable via
// RenderSettings.ray_query_resolution_scale). Reads GBufferB (world position
// + linear depth) and GBufferA (normal) to cast visibility rays toward the
// light direction. Writes R8_UNORM shadow factor (1=lit, 0=shadowed).
//
// Requires VK_KHR_ray_query (inline ray queries, no RT pipeline needed).
//
//   slangc shadow.slang -profile spirv_1_5 -target spirv \
//          -entry computeMain -stage compute -o shadow.comp.spv

import common;

// set 0, binding 0 - frame UBO (camera + light)
[[vk::binding(0, 0)]]
ConstantBuffer<FrameUBO> frame;

// set 0, binding 1 — GBufferA (normal + roughness), read-only
[[vk::binding(1, 0)]]
Texture2D<float4> gbufferA;

// set 0, binding 2 — GBufferB (position + depth), read-only
[[vk::binding(2, 0)]]
Texture2D<float4> gbufferB;

// set 0, binding 3 — shadow output (R8_UNORM, 1=lit)
[[vk::binding(3, 0)]]
RWTexture2D<float> shadowOutput;

// set 0, binding 4 — TLAS for ray queries
[[vk::binding(4, 0)]]
RaytracingAccelerationStructure tlas;

// Push constants: output dimensions + light params
struct ShadowPushConstants
{
    uint outputWidth;
    uint outputHeight;
    uint gbufferWidth;
    uint gbufferHeight;
    // Light direction (normalized) + range (max shadow distance)
    float3 lightDir;
    float  lightRange;
    // Bias to avoid self-intersection acne
    float  normalBias;
    float  _padding0;
    float  _padding1;
    float  _padding2;
};
[[vk::push_constant]]
ShadowPushConstants pc;

[shader("compute")]
[numthreads(8, 8, 1)]
void computeMain(uint3 dispatchID : SV_DispatchThreadID)
{
    if (dispatchID.x >= pc.outputWidth || dispatchID.y >= pc.outputHeight)
        return;

    // Sample GBuffer at the scaled coordinate
    float2 uv = (float2(dispatchID.xy) + 0.5) / float2(pc.outputWidth, pc.outputHeight);
    uint2 gbufferCoord = uint2(uv * float2(pc.gbufferWidth, pc.gbufferHeight));

    float4 gba = gbufferA.Load(int3(gbufferCoord, 0));
    float4 gbb = gbufferB.Load(int3(gbufferCoord, 0));

    float3 world_pos = gbb.xyz;
    float  depth     = gbb.w;

    // Sky pixel (far plane) — no shadow
    if (depth >= 10000.0)
    {
        shadowOutput[dispatchID.xy] = 1.0;
        return;
    }

    float3 normal = normalize(gba.xyz);

    // Ray origin offset along normal to prevent acne
    float3 origin = world_pos + normal * pc.normalBias;

    // Ray direction: toward the light
    float3 dir = normalize(pc.lightDir);

    // Skip if surface faces away from light
    if (dot(normal, dir) <= 0.0)
    {
        shadowOutput[dispatchID.xy] = 0.0;
        return;
    }

    // Inline ray query — no hit shader, just visibility test
    RayQuery<RAY_FLAG_ACCEPT_FIRST_HIT_AND_END_SEARCH
           | RAY_FLAG_SKIP_CLOSEST_HIT_SHADER
           | RAY_FLAG_CULL_BACK_FACING_TRIANGLES> rq;

    RayDesc ray;
    ray.Origin = origin;
    ray.Direction = dir;
    ray.TMin = 0.01;
    ray.TMax = pc.lightRange;

    rq.TraceRayInline(
        tlas,          // acceleration structure
        RAY_FLAG_NONE, // additional flags
        0xFF,          // instance mask (all)
        ray
    );

    rq.Proceed();

    bool shadowed = rq.CommittedStatus() == COMMITTED_TRIANGLE_HIT;

    shadowOutput[dispatchID.xy] = shadowed ? 0.0 : 1.0;
}
