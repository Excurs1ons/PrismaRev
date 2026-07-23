// GI bake compute shader — offline probe-volume baker (GPU ray-query).
//
// Uses the shared path_integrator module for multi-bounce path tracing.
// Each thread bakes one probe: casts N rays (Fibonacci sphere) against the
// TLAS, uses path_integrator::TraceRadiance to compute incoming radiance
// (with multi-bounce, Russian roulette, and sky termination), and projects
// it onto 9 order-2 SH coefficients with cosine weighting.
//
// Dispatch: ((dims.x+3)/4, (dims.y+3)/4, dims.z) with numthreads(4,4,1) -
// one thread per probe (the guard clips the tail when dims isn't a multiple
// of 4). Output: RWTexture3D in GENERAL layout, coefficient-major depth slices.
//
// Requires: VK_KHR_ray_query (hardware ray tracing).

import path_integrator;

// ---- SH basis constants (must match gi.slang / gi.rs) ----
static const uint SH_COEFF_COUNT = 9u;
static const float SH_C0 = 0.282095;
static const float SH_C1 = 0.488603;
static const float SH_C2 = 1.092548;
static const float SH_C3 = 0.315392;
static const float SH_C4 = 0.546274;

// Evaluate the 9 SH basis functions for direction n. Returns basis[0..8].
void sh_basis(float3 n, out float basis[SH_COEFF_COUNT])
{
    basis[0] = SH_C0;
    basis[1] = SH_C1 * n.y;
    basis[2] = SH_C1 * n.z;
    basis[3] = SH_C1 * n.x;
    basis[4] = SH_C2 * n.x * n.y;
    basis[5] = SH_C2 * n.y * n.z;
    basis[6] = SH_C3 * (3.0 * n.z * n.z - 1.0);
    basis[7] = SH_C2 * n.x * n.z;
    basis[8] = SH_C4 * (n.x * n.x - n.y * n.y);
}

// ---- Probe volume metadata (matches gi.slang ProbeVolumeInfo) ----
struct ProbeVolumeInfo
{
    float4 origin;   // xyz = world pos of probe (0,0,0)
    float4 spacing;  // xyz = distance between probes
    uint4  dims;     // xyz = probe count per axis
};

// ---- Push constants ----
// NOTE: this struct is `repr(C)` and mirrored byte-for-byte by `BakePush` in
// `prism_bake_gi.rs` (size 56 = 3x vec4 + 2x uint). Keep the layout in lock-step.
struct BakePush
{
    float4 light_dir;      // xyz = normalized direction TO light, w unused
    float4 light_color;    // xyz = directional light radiance, w unused
    float4 probe_spacing;  // xyz = per-axis probe spacing (for inside-solid test ray length), w unused
    uint   num_rays;       // number of sample directions per probe
    uint   max_bounce;     // max path depth (1 = single bounce, 3 = three bounces)
};

// ---- Descriptors (set 0) ----
[[vk::binding(0, 0)]]
[format("rgba32f")]
RWTexture3D<float4> probeVolume;

[[vk::binding(1, 0)]]
ConstantBuffer<ProbeVolumeInfo> probeInfo;

[[vk::binding(2, 0)]]
RaytracingAccelerationStructure tlas;

[[vk::binding(3, 0)]]
ByteAddressBuffer vertexData;   // interleaved mesh::Vertex (56-byte stride)

[[vk::binding(4, 0)]]
StructuredBuffer<uint> indices;

[[vk::push_constant]]
BakePush push;

// ---- Compute entry point ----
[shader("compute")]
[numthreads(4, 4, 1)]
void bakeMain(uint3 tid : SV_DispatchThreadID)
{
    uint3 dims = probeInfo.dims.xyz;
    if (tid.x >= dims.x || tid.y >= dims.y || tid.z >= dims.z)
        return;

    // World position of this probe.
    float3 world_pos = probeInfo.origin.xyz
                     + float3(tid.x, tid.y, tid.z) * probeInfo.spacing.xyz;

    // ---- Inside-solid classification ----
    // Probes inside walls/floors are detected by counting short-range back-face
    // hits (the ray exits solid geometry within ~2× probe spacing). Only the
    // FIRST bounce from the probe position contributes to this vote.
    float3 sp = push.probe_spacing.xyz;
    float test_len = 2.0 * max(sp.x, max(sp.y, sp.z));
    static const float INSIDE_SOLID_THRESHOLD = 0.5; // >=50% short back-face hits

    // Accumulate SH coefficients (RGB).
    float3 sh[SH_COEFF_COUNT];
    for (uint c = 0u; c < SH_COEFF_COUNT; c++)
        sh[c] = float3(0.0, 0.0, 0.0);

    uint N = push.num_rays;
    float weight_base = 4.0 * 3.14159265 / float(N);
    uint first_hit_count = 0;   // diagnostic: first-bounce rays that hit geometry
    uint short_back_hits = 0u;  // rays exiting solid at short range (inside-solid, bounce 0 only)

    for (uint si = 0; si < N; si++)
    {
        // Fibonacci sphere direction for the first bounce.
        float y = 1.0 - 2.0 * (float(si) + 0.5) / float(N);
        float r = sqrt(max(0.0, 1.0 - y * y));
        float phi = float(si) * 2.399963; // golden angle
        float3 initial_dir = float3(r * cos(phi), y, r * sin(phi));

        // RNG seed: unique per thread + ray.
        uint seed = tid.x * 73856093u
                  + tid.y * 19349663u
                  + tid.z * 83492791u
                  + si * 2567483615u;

        // ---- Inside-solid detection: inspect the first bounce ----
        // We need to look at the first hit to cast the inside-solid vote.
        // Rather than doing a separate trace, we call TraceRadiance for the
        // full multi-bounce path AND manually check the first bounce.
        // Unfortunately TraceRadiance doesn't expose the first-bounce details,
        // so we do a dedicated first-ray check here for inside-solid voting,
        // then let TraceRadiance handle the full multi-bounce radiance.
        //
        // This means the first ray is traced TWICE per Fibonacci direction:
        // once for the inside-solid vote (cheap, just checks back-face + dist),
        // and once inside TraceRadiance (full path tracing with all bounces).
        // On modern RT hardware a single ray query is ~10-50 ns, so doubling
        // 64 rays × 18k probes (~2.3M extra rays) adds only ~0.1 seconds.
        {
            RayDesc ray;
            ray.Origin = world_pos;
            ray.Direction = initial_dir;
            ray.TMin = 0.01;
            ray.TMax = 1000.0;

            RayQuery<RAY_FLAG_FORCE_OPAQUE> q;
            q.TraceRayInline(tlas, RAY_FLAG_FORCE_OPAQUE, 0xFF, ray);
            while (q.Proceed()) { }

            if (q.CommittedStatus() == COMMITTED_TRIANGLE_HIT)
            {
                first_hit_count++;

                uint prim_idx = q.CommittedPrimitiveIndex();
                uint i0 = indices[prim_idx * 3 + 0];
                uint i1 = indices[prim_idx * 3 + 1];
                uint i2 = indices[prim_idx * 3 + 2];

                float3 v0 = asfloat(vertexData.Load3(i0 * VERTEX_STRIDE + POSITION_OFFSET));
                float3 v1 = asfloat(vertexData.Load3(i1 * VERTEX_STRIDE + POSITION_OFFSET));
                float3 v2 = asfloat(vertexData.Load3(i2 * VERTEX_STRIDE + POSITION_OFFSET));

                float3 edge1 = v1 - v0;
                float3 edge2 = v2 - v0;
                float3 hit_normal = cross(edge1, edge2);
                float tri_area2 = dot(hit_normal, hit_normal);
                if (tri_area2 >= 1e-12)
                {
                    hit_normal = hit_normal * rsqrt(tri_area2);
                    bool back_face = dot(hit_normal, initial_dir) > 0.0;
                    if (back_face && q.CommittedRayT() < test_len)
                        short_back_hits++;
                }
            }
        }

        // Full multi-bounce radiance via the shared path integrator.
        float3 L = TraceRadiance(
            world_pos, initial_dir,
            tlas, vertexData, indices,
            push.light_dir.xyz, push.light_color.xyz,
            push.max_bounce, seed);

        // Radiance SH projection using the INITIAL direction.
        // No cosine pre-convolution — the probe volume stores the radiance
        // function L(ω) directly. At runtime:
        //   * Diffuse  → EvalSH9Irradiance() applies the cosine A_l factors
        //   * Specular → EvalSH9Radiance() evaluates L(ω) directly
        // The A_l factors (see gi.slang) include the PI normalization
        // automatically, eliminating the old /PI mismatch between IBL and GI.
        // weight_base = 4π/N is the standard Monte Carlo weight for uniform
        // spherical sampling (pdf = 1/4π, so dω = 4π/N per sample).
        float w = weight_base;

        float basis[SH_COEFF_COUNT];
        sh_basis(initial_dir, basis);

        for (uint c = 0; c < SH_COEFF_COUNT; c++)
        {
            sh[c] += L * basis[c] * w;
        }
    }

    // ---- Inside-solid verdict ----
    if (float(short_back_hits) / float(N) >= INSIDE_SOLID_THRESHOLD)
    {
        uint dz = dims.z;
        for (uint c = 0u; c < SH_COEFF_COUNT; c++)
        {
            uint tex_z = c * dz + tid.z;
            float alpha = (c == 0u) ? -1.0 : 0.0; // -1 = inside-solid sentinel
            probeVolume[uint3(tid.x, tid.y, tex_z)] = float4(0.0, 0.0, 0.0, alpha);
        }
        return;
    }

    // Write to 3D texture (coefficient-major depth slices).
    uint dz = dims.z;
    float hit_ratio = float(first_hit_count) / float(N);
    for (uint c = 0u; c < SH_COEFF_COUNT; c++)
    {
        uint tex_z = c * dz + tid.z;
        float alpha = (c == 0u) ? hit_ratio : 0.0;
        probeVolume[uint3(tid.x, tid.y, tex_z)] = float4(sh[c], alpha);
    }
}
