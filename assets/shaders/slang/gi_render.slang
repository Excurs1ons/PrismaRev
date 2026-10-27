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

#include "path_integrator.slang"

// ---- Push constants ----
struct RenderPush
{
    float4x4 inv_view_proj;  // NDC [-1,1]^2 (y-flipped, depth [0,1]) → world
    float4   camera_pos;     // .w unused
    float4   light_dir;      // .w unused
    float4   light_color;    // .w unused
    uint4    params;         // .x = width, .y = height, .z = max_bounce,
                             // .w = num_samples | GI_PARAMS_ORTHOGRAPHIC
};

// ---- params.w bit layout (mirrored on the CPU side when the pass is wired) ----
static const uint GI_PARAMS_SAMPLES_MASK = 0x7FFFFFFFu; // bits 0..30: samples per pixel
static const uint GI_PARAMS_ORTHOGRAPHIC = 1u << 31u;  // bit 31: orthographic camera

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
// NDC z = 0 is the near plane and z = 1 the far plane (Vulkan depth range
// [0,1]), so unprojecting both gives the ray through this pixel; near→far is
// the camera forward. A perspective camera starts every ray at the eye, while
// an orthographic camera casts parallel rays from the pixel's near-plane point.
// Mirrors `pt_render.slang` (`#10`).
void generate_ray(uint px, uint py, float jx, float jy,
    out float3 origin, out float3 dir)
{
    uint w = push.params.x;
    uint h = push.params.y;

    // Pixel center + jitter → NDC [-1, 1]^2 (y-flipped: -1=top, +1=bottom)
    float ndc_x = ((float(px) + 0.5 + jx) / float(w)) * 2.0 - 1.0;
    float ndc_y = ((float(py) + 0.5 + jy) / float(h)) * 2.0 - 1.0;

    // Unproject both depth planes; near→far is the primary-ray direction.
    float4 world_near = mul(push.inv_view_proj, float4(ndc_x, ndc_y, 0.0, 1.0));
    float4 world_far = mul(push.inv_view_proj, float4(ndc_x, ndc_y, 1.0, 1.0));
    float3 near_pt = world_near.xyz / world_near.w;
    float3 far_pt = world_far.xyz / world_far.w;

    dir = normalize(far_pt - near_pt);
    bool orthographic = (push.params.w & GI_PARAMS_ORTHOGRAPHIC) != 0u;
    origin = orthographic ? near_pt : push.camera_pos.xyz;
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

    uint num_samples = push.params.w & GI_PARAMS_SAMPLES_MASK;
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
            0u, 0u, push.params.z, 1000.0, seed);

        accum += L;
    }

    float3 result = accum / float(num_samples);
    outputImage[uint2(tid.x, tid.y)] = float4(result, 1.0);
}
