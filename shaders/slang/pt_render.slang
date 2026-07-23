// Real-time path-tracing renderer — compute shader with temporal accumulation.
//
// Each frame traces ONE sample per pixel and accumulates the result across
// frames (1 sample/frame → noise converges over time). The accumulation
// buffer is reset when the camera moves or the user requests it.
//
// Dispatch: ((width+15)/16, (height+15)/16, 1) with numthreads(16,16,1).
// Requires: VK_KHR_ray_query (RTX hardware).
//
// Output: The accumulated radiance divided by sample count is written to
// the output image (PT_COLOR_H in the render graph), which PostPass tonemaps.

import path_integrator;

// ---- Push constants ----
struct PtPush
{
    float4x4 inv_view_proj;  // NDC [-1,1]^2 (y-flipped, depth [0,1]) → world
    float4   camera_pos;     // .w unused
    float4   light_dir;      // xyz = dir TO light, w = pre-scaled radiance
    uint4    params;         // x = width, y = height, z = max_bounce, w = packed(frame_count|reset<<31)
};

// ---- Descriptors (set 0) ----
[[vk::binding(0, 0)]]
[format("rgba32f")]
RWTexture2D<float4> accumImage;     // persistent accumulation buffer (RGBA32F)

[[vk::binding(1, 0)]]
[format("r32ui")]
RWTexture2D<uint> sampleCount;      // per-pixel sample counter (R32_UINT)

[[vk::binding(2, 0)]]
RaytracingAccelerationStructure tlas;

[[vk::binding(3, 0)]]
ByteAddressBuffer vertexData;

[[vk::binding(4, 0)]]
StructuredBuffer<uint> indices;

[[vk::binding(5, 0)]]
[format("rgba32f")]
RWTexture2D<float4> outputImage;    // resolved output (PIXEL_COLOR_H)

[[vk::push_constant]]
PtPush push;

// ---- Ray generation from pixel coordinates ----
void generate_ray(uint px, uint py, float jx, float jy,
    out float3 origin, out float3 dir)
{
    uint w = push.params.x;
    uint h = push.params.y;

    // Pixel center + jitter → NDC [-1, 1]^2 (y-flipped: -1=top, +1=bottom)
    float ndc_x = ((float(px) + 0.5 + jx) / float(w)) * 2.0 - 1.0;
    float ndc_y = ((float(py) + 0.5 + jy) / float(h)) * 2.0 - 1.0;

    // Unproject from clip space at depth 0 (= near plane in [0,1] depth)
    float4 clip = float4(ndc_x, ndc_y, 0.0, 1.0);
    float4 world = mul(push.inv_view_proj, clip);
    float3 world_pt = world.xyz / world.w;

    origin = push.camera_pos.xyz;
    dir = normalize(world_pt - origin);
}

// ---- Compute entry point ----
[shader("compute")]
[numthreads(16, 16, 1)]
void ptMain(uint3 tid : SV_DispatchThreadID)
{
    uint w = push.params.x;
    uint h = push.params.y;
    if (tid.x >= w || tid.y >= h)
        return;

    uint2 pixel = uint2(tid.x, tid.y);

    // Unpack reset from params.w bit 31
    uint frame_count = push.params.w & 0x7FFFFFFFu;
    uint reset = (push.params.w >> 31u) & 1u;

    // ---- Reset accumulation on demand ----
    if (reset != 0u)
    {
        accumImage[pixel] = float4(0.0, 0.0, 0.0, 0.0);
        sampleCount[pixel] = 0u;
        // Still trace one sample so the first post-reset frame has data.
    }

    // ---- Trace one sample with pixel jitter ----
    uint base_seed = tid.x * 73856093u + tid.y * 19349663u + frame_count * 2567483615u;
    uint seed = base_seed;

    float jx = rand_float(seed) - 0.5;
    float jy = rand_float(seed) - 0.5;

    float3 origin, dir;
    generate_ray(tid.x, tid.y, jx, jy, origin, dir);

    float3 L = TraceRadiance(
        origin, dir,
        tlas, vertexData, indices,
        push.light_dir.xyz, float3(1.0, 1.0, 1.0),
        push.params.z, seed);

    // ---- Accumulate ----
    float3 prev = accumImage[pixel].rgb;
    uint   count = sampleCount[pixel];

    float3 new_accum = prev + L;
    uint   new_count = count + 1u;

    accumImage[pixel] = float4(new_accum, 0.0);
    sampleCount[pixel] = new_count;

    // Output the resolved result to the output image (PT_COLOR_H in the
    // render graph, which PostPass reads as Sampler2D for tonemapping).
    float3 result = new_count > 0u ? new_accum / float(new_count) : float3(0.0);
    outputImage[pixel] = float4(result, 1.0);
}
