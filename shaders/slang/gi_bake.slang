// GI bake compute shader — offline probe-volume baker (GPU ray-query).
//
// Each thread bakes one probe: casts N rays (Fibonacci sphere) against the
// TLAS, computes direct lighting (directional light × hit albedo), and
// projects the incoming radiance onto 9 order-2 SH coefficients with cosine
// weighting (pre-multiplied cosine convolution, matching gi.rs contract).
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

// ---- Vertex layout (matches prism-render Vertex: pos[3], normal[3], color[4], uv[2], tangent[3], _pad) ----
struct GpuVertex
{
    float3 position;
    float3 normal;
    float4 color;    // .rgb = albedo (linear)
    float2 uv;
    float3 tangent;
    float  _pad;
};

// ---- Push constants ----
struct BakePush
{
    float4 light_dir;    // xyz = normalized direction TO light, w unused
    float4 light_color;  // xyz = directional light radiance, w unused
    uint   num_rays;     // number of sample directions per probe
};

// ---- Descriptors (set 0) ----
[[vk::binding(0, 0)]]
RWTexture3D<float4> probeVolume;

[[vk::binding(1, 0)]]
ConstantBuffer<ProbeVolumeInfo> probeInfo;

[[vk::binding(2, 0)]]
RaytracingAccelerationStructure tlas;

[[vk::binding(3, 0)]]
StructuredBuffer<GpuVertex> vertices;

[[vk::binding(4, 0)]]
StructuredBuffer<uint> indices;

[[vk::binding(5, 0)]]
StructuredBuffer<float4> instanceAlbedo; // per-instance albedo override (rgb), w = unused

[[vk::push_constant]]
BakePush push;

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

        RayQuery<RAY_FLAG_CULL_BACK_FACING_TRIANGLES> q;
        q.TraceRayInline(tlas, RAY_FLAG_NONE, 0xFF, ray);
        q.Proceed();

        float3 radiance = float3(0.0, 0.0, 0.0);

        if (q.CommittedStatus() == COMMITTED_TRIANGLE_HIT)
        {
            // Get hit triangle vertices.
            uint prim_idx = q.CommittedPrimitiveIndex();
            uint instance_id = q.CommittedInstanceIndex();
            uint i0 = indices[prim_idx * 3 + 0];
            uint i1 = indices[prim_idx * 3 + 1];
            uint i2 = indices[prim_idx * 3 + 2];

            float3 v0 = vertices[i0].position;
            float3 v1 = vertices[i1].position;
            float3 v2 = vertices[i2].position;

            // Barycentric interpolation for albedo.
            float2 bary = q.CommittedTriangleBarycentrics();
            float w0 = 1.0 - bary.x - bary.y;
            float3 albedo_v0 = vertices[i0].color.rgb;
            float3 albedo_v1 = vertices[i1].color.rgb;
            float3 albedo_v2 = vertices[i2].color.rgb;
            float3 hit_albedo = w0 * albedo_v0 + bary.x * albedo_v1 + bary.y * albedo_v2;

            // Per-instance albedo override (if available).
            if (instance_id < 1024u)
            {
                float4 inst_alb = instanceAlbedo[instance_id];
                if (inst_alb.w > 0.5)
                    hit_albedo = inst_alb.rgb;
            }

            // Direct lighting: NdotL * light_color * albedo.
            float3 hit_normal = normalize(cross(v1 - v0, v2 - v0));
            float ndotl = max(dot(hit_normal, push.light_dir.xyz), 0.0);
            radiance = hit_albedo * push.light_color.xyz * ndotl;
        }

        // Cosine-weighted SH projection.
        // The probe "surface normal" for cosine weighting is the ray direction
        // itself (we're integrating the sphere, cosine lobe centered on each
        // probe's local up — but for a full-sphere probe we use cos(theta)
        // relative to the probe's implicit up = +Y).
        float cos_w = max(dir.y, 0.0); // hemisphere weighting (upper)
        float w = weight_base * cos_w;

        float basis[SH_COEFF_COUNT];
        sh_basis(dir, basis);

        for (uint c = 0; c < SH_COEFF_COUNT; c++)
        {
            sh[c] += radiance * basis[c] * w;
        }
    }

    // Write to 3D texture (coefficient-major depth slices).
    uint dz = dims.z;
    for (uint c = 0; c < SH_COEFF_COUNT; c++)
    {
        uint tex_z = c * dz + tid.z;
        probeVolume[uint3(tid.x, tid.y, tex_z)] = float4(sh[c], 0.0);
    }
}
