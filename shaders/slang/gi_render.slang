// Offline path-tracing renderer — compute shader.
//
// Uses the shared path_integrator module for multi-bounce path tracing.
// Each thread renders one pixel: generates a ray from the camera through
// the pixel, calls path_integrator::TraceRadiance, accumulates samples,
// and writes the HDR result to a 2D output texture.
//
// Dispatch: ((width+15)/16, (height+15)/16, 1) with numthreads(16,16,1).
// Output: RWTexture2D RGBA32F (linear HDR, .a unused).
//
// Requires: VK_KHR_ray_query (hardware ray tracing).

import path_integrator;

// ---- Push constants ----
struct RenderPush
{
    float4x4 inv_view_proj;  // NDC [-1,1]^2 (y-flipped, depth [0,1]) → world
    float4   camera_pos;     // .w unused
    float4   light_dir;      // .w unused
    float4   light_color;    // .w unused
    uint4    params;         // .x = width, .y = height, .z = max_bounce, .w = num_samples
};

// ---- Descriptors (set 0) ----
[[vk::binding(0, 0)]]
[format("rgba32f")]
RWTexture2D<float4> outputImage;

[[vk::binding(1, 0)]]
RaytracingAccelerationStructure tlas;

[[vk::binding(2, 0)]]
ByteAddressBuffer vertexData;

[[vk::binding(3, 0)]]
StructuredBuffer<uint> indices;

[[vk::push_constant]]
RenderPush push;

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
void renderMain(uint3 tid : SV_DispatchThreadID)
{
    uint w = push.params.x;
    uint h = push.params.y;
    if (tid.x >= w || tid.y >= h)
        return;

    uint num_samples = push.params.w;
    float3 accum = float3(0.0, 0.0, 0.0);

    // RNG seed: unique per pixel.
    uint base_seed = tid.x * 73856093u + tid.y * 19349663u;

    for (uint sample_idx = 0u; sample_idx < num_samples; sample_idx++)
    {
        // Jitter within the pixel for anti-aliasing.
        uint seed = base_seed + sample_idx * 2567483615u;
        float jx = rand_float(seed) - 0.5;
        float jy = rand_float(seed) - 0.5;

        float3 origin, dir;
        generate_ray(tid.x, tid.y, jx, jy, origin, dir);

        float3 L = TraceRadiance(
            origin, dir,
            tlas, vertexData, indices,
            push.light_dir.xyz, push.light_color.xyz,
            push.params.z, seed);

        accum += L;
    }

    float3 result = accum / float(num_samples);
    outputImage[uint2(tid.x, tid.y)] = float4(result, 1.0);
}
