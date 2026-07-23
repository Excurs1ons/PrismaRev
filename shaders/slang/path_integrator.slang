// Path integrator - shared multi-bounce path tracing core.
//
// Slang module imported by GI bakers, offline renderers, and the real-time PT
// pass. It only computes the radiance arriving at a point from a given
// direction by tracing paths through the scene. Consumers decide what to do
// with that radiance (SH projection, pixel color, ...).
//
// Geometry + materials are module-global bindings (so `TraceRadiance`'s
// signature stays small): one combined vertex/index buffer, one TLAS, a
// per-instance metadata SSBO (`PtInstanceMeta[]`), and the shared
// `GpuMaterial[]` SSBO + bindless texture table. Albedo is resolved at the ray
// hit exactly like the rasterizer: instance index -> material slot -> material
// -> albedo texture (if bound) else scalar base color.
module path_integrator;

// ---- Vertex layout constants (shared by all consumers) ----
// Matches prism-render `mesh::Vertex`, repr(C), 60 bytes.
public static const uint VERTEX_STRIDE   = 60u;
public static const uint POSITION_OFFSET = 0u;
public static const uint NORMAL_OFFSET   = 12u;
public static const uint UV_OFFSET       = 36u;

// ---- Descriptor bindings (consumers must declare the same sets) ----
// Set 0: per-consumer local bindings. b0..b5 are consumer-specific (accum/
// output images, TLAS, vertex/index buffers). b6 + b7 are shared here:
[[vk::binding(6, 0)]]
StructuredBuffer<PtInstanceMeta> instance_meta; // per-instance material slot + bases

[[vk::binding(7, 0)]]
StructuredBuffer<GpuMaterial> materials; // shared material SSBO

// Set 1: bindless texture table (shared with the rasterizer).
[[vk::binding(0, 1)]]
SamplerState globalSamplers[];
[[vk::binding(1, 1)]]
Texture2D bindlessSrvs[];

static const uint SAMPLER_LINEAR_WRAP = 0u;

// ---- Pseudo-random number generation (Wang hash) ----
// Deterministic hash-based RNG seeded by the caller. Each call advances the
// seed and returns a uniform uint in [0, 2^32).
public uint wang_hash(inout uint seed)
{
    seed = (seed ^ 61u) ^ (seed >> 16u);
    seed *= 9u;
    seed = seed ^ (seed >> 4u);
    seed *= 0x27d4eb2du;
    seed = seed ^ (seed >> 15u);
    return seed;
}

// Float in [0, 1) from the current seed.
public float rand_float(inout uint seed)
{
    return float(wang_hash(seed)) / 4294967296.0;
}

// ---- Cosine-weighted hemisphere sampling ----
// Given a unit normal `n` and two uniform random numbers `u1, u2` in [0,1),
// returns a unit direction in the hemisphere oriented around `n`, distributed
// with pdf = cos(theta)/pi (Lambertian importance sampling).
public float3 cosine_hemisphere_sample(float3 n, float u1, float u2)
{
    float theta = acos(sqrt(max(1e-8, 1.0 - u1)));
    float phi = 6.283185307 * u2; // 2 * PI

    float sin_t, cos_t;
    sincos(theta, sin_t, cos_t);
    float sin_p, cos_p;
    sincos(phi, sin_p, cos_p);
    float3 local_dir = float3(sin_t * cos_p, sin_t * sin_p, cos_t);

    // Build orthonormal basis from the normal.
    float3 T = abs(n.y) < 0.999
        ? normalize(cross(n, float3(0.0, 1.0, 0.0)))
        : normalize(cross(n, float3(1.0, 0.0, 0.0)));
    float3 B = cross(n, T);

    return local_dir.x * T + local_dir.y * B + local_dir.z * n;
}

// ---- Procedural sky radiance ----
// Bright blue sky above (zenith), hazy horizon, dark ground below.
public float3 sky_radiance(float3 dir)
{
    float t = clamp(dir.y, 0.0, 1.0);
    return lerp(float3(0.55, 0.60, 0.65), float3(0.25, 0.45, 0.90), t);
}

// ---- Per-instance metadata (mirrors `PtInstanceMeta` in pt_pass.rs) ----
struct PtInstanceMeta
{
    uint material_slot;  // index into `materials[]`
    uint index_base;     // base index into the combined index buffer
    uint vertex_base;    // base vertex into the combined vertex buffer
    uint _pad;
};

// ---- GpuMaterial (mirrors scene_frag.slang / GpuMaterial, 96 bytes) ----
struct GpuMaterial
{
    float4 base_color;                  // offset 0
    float4 metallic_roughness_emissive; // offset 16: x=metallic, y=roughness, z=emissive, w=emissive_strength
    uint   albedo_idx;                  // offset 32
    uint   normal_idx;                  // offset 36
    uint   metallic_roughness_idx;      // offset 40
    uint   emissive_idx;                // offset 44
    float4 transmission_factor;         // offset 48
    float4 clearcoat;                   // offset 64
    uint   transmission_tex_idx;        // offset 80
    uint   occlusion_idx;               // offset 84
    float  normal_scale;                // offset 88
    float  occlusion_strength;          // offset 92
};

// ---- Read the three vertices of a hit triangle ----
// `instance_id` is the TLAS instanceCustomIndex (= PtInstanceMeta index);
// `prim_idx` is the primitive index within that instance's BLAS. Returns the
// triangle's three positions and UVs via out-params.
public void read_hit_triangle(
    uint instance_id, uint prim_idx,
    ByteAddressBuffer vertexData, StructuredBuffer<uint> indices,
    out float3 v0, out float3 v1, out float3 v2,
    out float2 uv0, out float2 uv1, out float2 uv2)
{
    PtInstanceMeta meta = instance_meta[instance_id];
    uint i0 = indices[meta.index_base + prim_idx * 3 + 0];
    uint i1 = indices[meta.index_base + prim_idx * 3 + 1];
    uint i2 = indices[meta.index_base + prim_idx * 3 + 2];

    // Vertex addresses are absolute (combined buffer), so no +vertex_base
    // needed on the byte offset - the stored indices already index the
    // combined buffer (remapped at flatten time). But the byte offset uses
    // the vertex's position within the combined buffer directly.
    v0 = asfloat(vertexData.Load3(i0 * VERTEX_STRIDE + POSITION_OFFSET));
    v1 = asfloat(vertexData.Load3(i1 * VERTEX_STRIDE + POSITION_OFFSET));
    v2 = asfloat(vertexData.Load3(i2 * VERTEX_STRIDE + POSITION_OFFSET));
    uv0 = asfloat(vertexData.Load2(i0 * VERTEX_STRIDE + UV_OFFSET));
    uv1 = asfloat(vertexData.Load2(i1 * VERTEX_STRIDE + UV_OFFSET));
    uv2 = asfloat(vertexData.Load2(i2 * VERTEX_STRIDE + UV_OFFSET));
}

// ---- Resolve the hit surface albedo ----
// Mirrors scene_frag.slang: prefer the albedo texture (if bound), else the
// scalar base color factor. `uv` is the interpolated hit UV. Uses SampleLevel
// (mip 0) because RayQuery has no derivatives.
float3 resolve_albedo(GpuMaterial mat, float2 uv)
{
    if (mat.albedo_idx != 0xFFFFFFFFu)
    {
        float3 sampled = bindlessSrvs[NonUniformResourceIndex(mat.albedo_idx)]
            .SampleLevel(globalSamplers[NonUniformResourceIndex(SAMPLER_LINEAR_WRAP)], uv, 0.0).rgb;
        return sampled * mat.base_color.rgb;
    }
    return mat.base_color.rgb;
}

// ---- Core path tracing function ----
// Trace a path starting from `origin` in direction `dir`, accumulating
// direct-sun bounces through up to `max_bounce` diffuse inter-reflections
// with Russian-roulette termination. Geometry/materials are module globals.
//
// Parameters:
//   origin     - world-space ray origin (probe position or camera)
//   dir        - unit direction of the ray
//   light_dir  - unit direction TO the directional light (sun)
//   light_color- radiance of the directional light
//   max_bounce - maximum path depth (1 = only direct sun at first hit)
//   seed       - inout RNG seed (per-path, advanced on each random call)
//
// Returns the total radiance arriving at `origin` from direction `dir`.
public float3 TraceRadiance(
    float3 origin,
    float3 dir,
    RaytracingAccelerationStructure tlas,
    ByteAddressBuffer vertexData,
    StructuredBuffer<uint> indices,
    float3 light_dir,
    float3 light_color,
    uint max_bounce,
    inout uint seed)
{
    float3 total_radiance = float3(0.0, 0.0, 0.0);
    float3 throughput = float3(1.0, 1.0, 1.0);
    float3 pos = origin;

    for (uint bounce = 0u; bounce < max_bounce; bounce++)
    {
        // Trace ray against TLAS.
        RayDesc ray;
        ray.Origin = pos;
        ray.Direction = dir;
        ray.TMin = 0.01;
        ray.TMax = 1000.0;

        // No back-face culling + force opaque (all geometry is opaque).
        RayQuery<RAY_FLAG_FORCE_OPAQUE> q;
        q.TraceRayInline(tlas, RAY_FLAG_FORCE_OPAQUE, 0xFF, ray);
        while (q.Proceed()) { }

        if (q.CommittedStatus() == COMMITTED_TRIANGLE_HIT)
        {
            // Instance index -> per-instance metadata -> material slot.
            uint instance_id = q.CommittedInstanceID();
            uint prim_idx = q.CommittedPrimitiveIndex();

            float3 v0, v1, v2;
            float2 uv0, uv1, uv2;
            read_hit_triangle(instance_id, prim_idx, vertexData, indices,
                v0, v1, v2, uv0, uv1, uv2);

            float2 bary = q.CommittedTriangleBarycentrics();
            float w0 = 1.0 - bary.x - bary.y;
            float3 hit_pos = pos + dir * q.CommittedRayT();

            // Geometric normal from winding. Flip to face the incoming ray.
            float3 edge1 = v1 - v0;
            float3 edge2 = v2 - v0;
            float3 hit_normal = cross(edge1, edge2);
            float tri_area2 = dot(hit_normal, hit_normal);
            if (tri_area2 < 1e-12)
                break; // degenerate triangle, terminate this path
            hit_normal = hit_normal * rsqrt(tri_area2);
            if (dot(hit_normal, dir) > 0.0)
                hit_normal = -hit_normal;

            // Resolve albedo from the hit material (texture or scalar).
            PtInstanceMeta meta = instance_meta[instance_id];
            GpuMaterial mat = materials[meta.material_slot];
            float2 hit_uv = w0 * uv0 + bary.x * uv1 + bary.y * uv2;
            float3 hit_albedo = resolve_albedo(mat, hit_uv);

            // Direct sun contribution at this bounce.
            float ndotl = max(dot(hit_normal, light_dir), 0.0);
            total_radiance += throughput * hit_albedo * light_color * ndotl;

            // Prepare for next bounce.
            throughput *= hit_albedo;

            // Russian roulette termination.
            float p = max(max(throughput.r, throughput.g), throughput.b);
            if (bounce + 1u >= max_bounce)
                break;
            if (p < 1e-4)
                break;

            float surv = min(p, 1.0);
            float rr = rand_float(seed);
            if (rr >= surv)
                break;
            throughput /= surv;

            // Sample new direction (cosine-weighted hemisphere).
            float u1 = rand_float(seed);
            float u2 = rand_float(seed);
            dir = cosine_hemisphere_sample(hit_normal, u1, u2);

            // Offset origin to avoid self-intersection.
            pos = hit_pos + 0.001 * hit_normal;
        }
        else
        {
            // Missed the scene: sky radiance.
            total_radiance += throughput * sky_radiance(dir);
            break;
        }
    }

    return total_radiance;
}
