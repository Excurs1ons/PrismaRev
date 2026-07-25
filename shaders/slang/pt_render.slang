// Real-time path-tracing renderer - compute shader with temporal accumulation.
//
// Each frame traces ONE sample per pixel and accumulates the result across
// frames (1 sample/frame -> noise converges over time). The accumulation
// buffer is reset when the camera moves or the user requests it.
//
// Dispatch: ((width+15)/16, (height+15)/16, 1) with numthreads(16,16,1).
// Requires: VK_KHR_ray_query (RTX hardware).
//
// Output: The accumulated radiance divided by sample count is written to
// the output image (PT_COLOR_H in the render graph), which PostPass tonemaps.
//
// Bindings:
//   set 0 (PT-local):
//     b0 RWTexture2D<float4> accumImage
//     b1 RWTexture2D<uint>   sampleCount
//     b2 AccelerationStructure tlas
//     b3 ByteAddressBuffer vertexData (combined world-space vertices)
//     b4 StructuredBuffer<uint> indices (combined index buffer)
//     b5 RWTexture2D<float4> outputImage
//     b6 StructuredBuffer<PtInstanceMeta> instance_meta (module-global)
//     b7 StructuredBuffer<GpuMaterial> materials (module-global)
//   set 1 (shared bindless): globalSamplers[] + bindlessSrvs[] (module-global)
// The b6/b7 + set 1 bindings are declared in path_integrator.slang (imported).

import path_integrator;

// ---- Push constants ----
// `exposure` is packed into `camera_pos.w` to keep the push-constant struct
// compact. `ray_max_distance` is a separate float at the end - the struct is
// 132 bytes, which exceeds the Vulkan-guaranteed 128-byte push-constant
// minimum, so the device must advertise maxPushConstantsSize >= 132 (verified
// at device selection in context.rs).
struct PtPush
{
    float4x4 inv_view_proj;  // NDC [-1,1]^2 (y-flipped, depth [0,1]) -> world
    float4   camera_pos;     // .xyz = camera pos, .w = exposure
    float4   light_dir;      // xyz = dir TO light, w = pre-scaled radiance
    float4   light_color;    // rgb = directional light color, w = ambient factor
    uint4    params;         // x = width, y = height, z = max_bounce, w = packed(frame_count|reset<<31)
    float    ray_max_distance; // max world-space length of primary + shadow rays
    uint     max_iterations;   // 0 = accumulate forever; >0 = freeze after N samples/pixel
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

    // Pixel center + jitter -> NDC [-1, 1]^2 (y-flipped: -1=top, +1=bottom)
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

    // ---- Freeze when max iterations reached ----
    uint sample_n = sampleCount[pixel];
    if (push.max_iterations > 0u && sample_n >= push.max_iterations)
    {
        float3 resolved = sample_n > 0u ? accumImage[pixel].rgb / float(sample_n) : float3(0.0);
        resolved *= push.camera_pos.w;
        outputImage[pixel] = float4(resolved, 1.0);
        return;
    }

    // ---- Trace one sample with pixel jitter ----
    uint base_seed = tid.x * 73856093u + tid.y * 19349663u + frame_count * 2567483615u;
    uint seed = base_seed;

    float jx = rand_float(seed) - 0.5;
    float jy = rand_float(seed) - 0.5;

    float3 origin, dir;
    generate_ray(tid.x, tid.y, jx, jy, origin, dir);

    // Effective sun radiance = color * intensity (matches the rasterizer's
    // `radiance = frame.lightColor.rgb * frame.lightDirection.w`). Previously
    // this was hardcoded to white, ignoring the scene's light color/intensity
    // and producing wrong-colored highlights.
    float3 sun_radiance = push.light_color.rgb * push.light_dir.w;

    float3 L = TraceRadiance(
        origin, dir,
        tlas, vertexData, indices,
        push.light_dir.xyz, sun_radiance,
        push.params.z, push.ray_max_distance, seed);

    // ---- Accumulate ----
    float3 prev = accumImage[pixel].rgb;
    uint   count = sampleCount[pixel];

    float3 new_accum = prev + L;
    uint   new_count = count + 1u;

    accumImage[pixel] = float4(new_accum, 0.0);
    sampleCount[pixel] = new_count;

    // Output the resolved result to the output image (PT_COLOR_H in the
    // render graph, which PostPass reads as Sampler2D for tonemapping).
    // Apply exposure as a uniform brightness multiplier (matches the raster
    // pipeline's `scene_frag.slang`: `color *= frame.exposure`). Exposure is
    // packed into `camera_pos.w` to keep the push-constant struct at 128 bytes.
    float3 result = new_count > 0u ? new_accum / float(new_count) : float3(0.0);
    result *= push.camera_pos.w;
    outputImage[pixel] = float4(result, 1.0);
}
