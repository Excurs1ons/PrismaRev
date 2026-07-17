// SHARC GI query entry — reads cached radiance from the resolved buffer.
//
// This is a compute shader that, for each pixel, looks up the SHARC hash grid
// at the GBuffer surface position+normal and writes the cached indirect
// radiance to an output image. The LightingPass then adds this to direct
// lighting.
//
// When gi_mode == SHARC_MODE_OFF, this pass is skipped entirely (Rust side).
// When gi_mode == SHARC_MODE_UPDATE, only Update+Resolve run (no query).
// When gi_mode == SHARC_MODE_ON, this query runs after Update+Resolve.
//
//   slangc sharc_query.slang -profile spirv_1_5 -target spirv \
//          -entry computeMain -stage compute -o sharc_query.comp.spv

import common;
// Use #include (NOT `import sharc.common`) so the SHARC_GRID_* `#define` macros
// from sharc/common.slang are visible in this translation unit. `import` is a
// module boundary and does not forward preprocessor macros — that is what broke
// CI with "undefined identifier SHARC_GRID_LOGARITHM_BASE".
#include "sharc/common.slang"

// set 0, binding 0 — frame UBO
[[vk::binding(0, 0)]]
ConstantBuffer<FrameUBO> frame;

// set 0, binding 1 — GBufferA (normal + roughness)
[[vk::binding(1, 0)]]
Texture2D<float4> gbufferA;

// set 0, binding 2 — GBufferB (position + depth)
[[vk::binding(2, 0)]]
Texture2D<float4> gbufferB;

// set 0, binding 3 — GI output (R16G16B16A16_SFLOAT, indirect radiance)
[[vk::binding(3, 0)]]
RWTexture2D<float4> giOutput;

// set 1, binding 0 — SHARC hash entries
[[vk::binding(0, 1)]]
RWStructuredBuffer<uint64_t> sharc_hash_entries;

// set 1, binding 1 — SHARC accumulation
[[vk::binding(1, 1)]]
RWStructuredBuffer<SharcAccumulationData> sharc_accumulation;

// set 1, binding 2 — SHARC resolved (read-only in query)
[[vk::binding(2, 1)]]
RWStructuredBuffer<SharcPackedData> sharc_resolved;

// Push constants
struct SharcQueryPushConstants
{
    uint   outputWidth;
    uint   outputHeight;
    uint   gbufferWidth;
    uint   gbufferHeight;
    uint   sharcCapacity;
    float  sharcSceneScale;
    float3 cameraPosition;
    float  _padding;
};
[[vk::push_constant]]
SharcQueryPushConstants pc;

[shader("compute")]
[numthreads(8, 8, 1)]
void computeMain(uint3 dispatchID : SV_DispatchThreadID)
{
    if (dispatchID.x >= pc.outputWidth || dispatchID.y >= pc.outputHeight)
        return;

    float2 uv = (float2(dispatchID.xy) + 0.5) / float2(pc.outputWidth, pc.outputHeight);
    uint2 gbufferCoord = uint2(uv * float2(pc.gbufferWidth, pc.gbufferHeight));

    float4 gba = gbufferA.Load(int3(gbufferCoord, 0));
    float4 gbb = gbufferB.Load(int3(gbufferCoord, 0));

    float3 world_pos = gbb.xyz;
    float  depth     = gbb.w;

    // Sky pixel — no GI
    if (depth >= 10000.0)
    {
        giOutput[dispatchID.xy] = float4(0, 0, 0, 0);
        return;
    }

    float3 normal = normalize(gba.xyz);

    // Build SHARC parameters
    HashGridParameters gridParams;
    gridParams.cameraPosition = pc.cameraPosition;
    gridParams.logarithmBase  = SHARC_GRID_LOGARITHM_BASE;
    gridParams.sceneScale     = pc.sharcSceneScale;
    gridParams.levelBias      = SHARC_GRID_LEVEL_BIAS;

    HashMapData hashMapData;
    hashMapData.capacity = pc.sharcCapacity;
    hashMapData.hashEntriesBuffer = sharc_hash_entries;

    SharcParameters sharcParams;
    sharcParams.gridParameters = gridParams;
    sharcParams.hashMapData    = hashMapData;
    sharcParams.radianceScale  = 1e3;
    sharcParams.enableAntiFireflyFilter = true;
    sharcParams.accumulationBuffer = sharc_accumulation;
    sharcParams.resolvedBuffer     = sharc_resolved;

    SharcHitData hitData;
    hitData.positionWorld = world_pos;
    hitData.normalWorld   = normal;

    float3 radiance;
    bool found = SharcGetCachedRadiance(sharcParams, hitData, radiance, false);

    if (found)
        giOutput[dispatchID.xy] = float4(radiance, 1.0);
    else
        giOutput[dispatchID.xy] = float4(0, 0, 0, 0);
}
