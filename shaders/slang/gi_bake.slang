// GI bake compute shader — offline probe-volume baker (GPU ray-query).
//
// Each thread bakes one probe: casts N rays (Fibonacci sphere) against the
// TLAS, computes incoming radiance (sky for missed rays, direct sun bounce
// for hits), and projects it onto 9 order-2 SH coefficients with cosine
// weighting (pre-multiplied cosine convolution, matching gi.rs contract).
//
// Missed rays return sky radiance — this is what makes enclosed interiors
// darken: probes inside a room have all rays hit geometry (little/no sky),
// while outdoor probes see bright sky through their upward rays.
//
// Dispatch: (dims.x, dims.y, dims.z) — one thread per probe.
// Output: RWTexture3D in GENERAL layout, coefficient-major depth slices.
//
// Requires: VK_KHR_ray_query (hardware ray tracing).

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

// ---- Vertex layout (matches prism-render `mesh::Vertex`, repr(C), 56 bytes) ----
// position [f32;3] @ 0, normal [f32;3] @ 12, color [f32;3] @ 24,
// uv [f32;2] @ 36, tangent [f32;3] @ 44. Read via ByteAddressBuffer with
// explicit byte offsets to avoid any struct-layout ambiguity.
static const uint VERTEX_STRIDE   = 56u;
static const uint POSITION_OFFSET = 0u;
static const uint COLOR_OFFSET    = 24u;

// ---- Push constants ----
struct BakePush
{
    float4 light_dir;    // xyz = normalized direction TO light, w unused
    float4 light_color;  // xyz = directional light radiance, w unused
    uint   num_rays;     // number of sample directions per probe
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

[[vk::binding(5, 0)]]
StructuredBuffer<float4> instanceAlbedo; // per-instance override; w>0.5 = use

[[vk::push_constant]]
BakePush push;

// Sky radiance for a missed ray (escapes the scene). Bright blue sky above,
// dark ground below. Only the upper hemisphere contributes (cosine weighting).
float3 sky_radiance(float3 dir)
{
    float t = clamp(dir.y, 0.0, 1.0);
    // Horizon (bright haze) -> zenith (deep blue).
    return lerp(float3(0.55, 0.60, 0.65), float3(0.25, 0.45, 0.90), t);
}

// ---- Compute entry point ----
[shader("compute")]
[numthreads(1, 1, 1)]
void bakeMain(uint3 tid : SV_DispatchThreadID)
{
    uint3 dims = probeInfo.dims.xyz;
    if (tid.x >= dims.x || tid.y >= dims.y || tid.z >= dims.z)
        return;

    // World position of this probe.
    float3 world_pos = probeInfo.origin.xyz
                     + float3(tid.x, tid.y, tid.z) * probeInfo.spacing.xyz;

    // Accumulate SH coefficients (RGB).
    float3 sh[SH_COEFF_COUNT];
    for (uint c = 0; c < SH_COEFF_COUNT; c++)
        sh[c] = float3(0.0, 0.0, 0.0);

    uint N = push.num_rays;
    float weight_base = 4.0 * 3.14159265 / float(N);
    uint hit_count = 0; // diagnostic: how many rays hit geometry

    for (uint si = 0; si < N; si++)
    {
        // Fibonacci sphere direction.
        float y = 1.0 - 2.0 * (float(si) + 0.5) / float(N);
        float r = sqrt(max(0.0, 1.0 - y * y));
        float phi = float(si) * 2.399963; // golden angle
        float3 dir = float3(r * cos(phi), y, r * sin(phi));

        // Trace ray against TLAS.
        RayDesc ray;
        ray.Origin = world_pos;
        ray.Direction = dir;
        ray.TMin = 0.01;
        ray.TMax = 1000.0;

        // No back-face culling: interior probes must hit the INSIDE of
        // single-sided walls (which are back-facing relative to the ray),
        // otherwise rays leak through walls to the sky and interiors stay
        // bright. TMin guards against self-intersection.
        RayQuery<RAY_FLAG_NONE> q;
        q.TraceRayInline(tlas, RAY_FLAG_NONE, 0xFF, ray);
        q.Proceed();

        float3 radiance;

        if (q.CommittedStatus() == COMMITTED_TRIANGLE_HIT)
        {
            hit_count++;
            // Hit triangle: read vertex positions + colors from the flattened
            // world-space mesh (single BLAS, global primitive/vertex indices).
            uint prim_idx = q.CommittedPrimitiveIndex();
            uint i0 = indices[prim_idx * 3 + 0];
            uint i1 = indices[prim_idx * 3 + 1];
            uint i2 = indices[prim_idx * 3 + 2];

            float3 v0 = asfloat(vertexData.Load3(i0 * VERTEX_STRIDE + POSITION_OFFSET));
            float3 v1 = asfloat(vertexData.Load3(i1 * VERTEX_STRIDE + POSITION_OFFSET));
            float3 v2 = asfloat(vertexData.Load3(i2 * VERTEX_STRIDE + POSITION_OFFSET));

            // Barycentric interpolation of per-vertex albedo (material base color).
            float2 bary = q.CommittedTriangleBarycentrics();
            float w0 = 1.0 - bary.x - bary.y;
            float3 c0 = asfloat(vertexData.Load3(i0 * VERTEX_STRIDE + COLOR_OFFSET));
            float3 c1 = asfloat(vertexData.Load3(i1 * VERTEX_STRIDE + COLOR_OFFSET));
            float3 c2 = asfloat(vertexData.Load3(i2 * VERTEX_STRIDE + COLOR_OFFSET));
            float3 hit_albedo = w0 * c0 + bary.x * c1 + bary.y * c2;

            // Per-instance albedo override (if flagged).
            uint instance_id = q.CommittedInstanceIndex();
            if (instance_id < 1024u)
            {
                float4 inst_alb = instanceAlbedo[instance_id];
                if (inst_alb.w > 0.5)
                    hit_albedo = inst_alb.rgb;
            }

            // Direct sun bounce off the hit surface: NdotL * light * albedo.
            float3 hit_normal = normalize(cross(v1 - v0, v2 - v0));
            float ndotl = max(dot(hit_normal, push.light_dir.xyz), 0.0);
            radiance = hit_albedo * push.light_color.xyz * ndotl;
        }
        else
        {
            // Missed the scene: incoming sky radiance.
            radiance = sky_radiance(dir);
        }

        // Cosine-weighted SH projection (upper hemisphere, lobe centered on +Y).
        float cos_w = max(dir.y, 0.0);
        float w = weight_base * cos_w;

        float basis[SH_COEFF_COUNT];
        sh_basis(dir, basis);

        for (uint c = 0; c < SH_COEFF_COUNT; c++)
        {
            sh[c] += radiance * basis[c] * w;
        }
    }

    // Write to 3D texture (coefficient-major depth slices). The c=0 texel's
    // alpha carries the per-probe hit ratio (diagnostic: 0 = all rays missed
    // the TLAS, 1 = fully enclosed).
    uint dz = dims.z;
    float hit_ratio = float(hit_count) / float(N);
    for (uint c = 0; c < SH_COEFF_COUNT; c++)
    {
        uint tex_z = c * dz + tid.z;
        float alpha = (c == 0u) ? hit_ratio : 0.0;
        probeVolume[uint3(tid.x, tid.y, tex_z)] = float4(sh[c], alpha);
    }
}
