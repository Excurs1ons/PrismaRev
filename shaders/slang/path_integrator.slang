// Path integrator — shared multi-bounce path tracing core.
//
// Slang module imported by GI bakers, offline renderers, lightmap bakers,
// and any other consumer that needs physically-based multi-bounce radiance.
//
// The module is producer-agnostic: it only computes the radiance arriving at a
// point from a given direction by tracing paths through the scene. Consumers
// decide what to do with that radiance (SH projection, pixel color, etc.).
//
// This replaces the old single-bounce + SKY_AMBIENT approach with proper
// path-traced multi-bounce using Russian-roulette termination.
module path_integrator;

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
// Matches the sky model in the original baker (but without SKY_SCALE, since
// multi-bounce naturally carries sky energy through the bounce chain).
public float3 sky_radiance(float3 dir)
{
    float t = clamp(dir.y, 0.0, 1.0);
    return lerp(float3(0.55, 0.60, 0.65), float3(0.25, 0.45, 0.90), t);
}

// ---- Vertex layout constants (shared by all consumers) ----
// Matches prism-render `mesh::Vertex`, repr(C), 56 bytes.
public static const uint VERTEX_STRIDE   = 60u;
public static const uint POSITION_OFFSET = 0u;
public static const uint COLOR_OFFSET    = 24u;

// ---- Core path tracing function ----
// Trace a path starting from `origin` in direction `dir`, accumulating
// direct-sun bounces through up to `max_bounce` diffuse inter-reflections
// with Russian-roulette termination.
//
// Parameters:
//   origin     — world-space ray origin (probe position or camera)
//   dir        — unit direction of the ray
//   tlas       — top-level acceleration structure
//   vertexData — ByteAddressBuffer of interleaved Vertex (56-byte stride)
//   indices    — triangle index buffer
//   light_dir  — unit direction TO the directional light (sun)
//   light_color— radiance of the directional light
//   max_bounce — maximum path depth (1 = only direct sun at first hit)
//   seed       — inout RNG seed (per-path, advanced on each random call)
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
            // Read hit surface data.
            uint prim_idx = q.CommittedPrimitiveIndex();
            uint i0 = indices[prim_idx * 3 + 0];
            uint i1 = indices[prim_idx * 3 + 1];
            uint i2 = indices[prim_idx * 3 + 2];

            float3 v0 = asfloat(vertexData.Load3(i0 * VERTEX_STRIDE + POSITION_OFFSET));
            float3 v1 = asfloat(vertexData.Load3(i1 * VERTEX_STRIDE + POSITION_OFFSET));
            float3 v2 = asfloat(vertexData.Load3(i2 * VERTEX_STRIDE + POSITION_OFFSET));

            float2 bary = q.CommittedTriangleBarycentrics();
            float w0 = 1.0 - bary.x - bary.y;
            float3 c0 = asfloat(vertexData.Load3(i0 * VERTEX_STRIDE + COLOR_OFFSET));
            float3 c1 = asfloat(vertexData.Load3(i1 * VERTEX_STRIDE + COLOR_OFFSET));
            float3 c2 = asfloat(vertexData.Load3(i2 * VERTEX_STRIDE + COLOR_OFFSET));
            float3 hit_albedo = w0 * c0 + bary.x * c1 + bary.y * c2;

            // Validate barycentric reconstruction against hit point.
            float3 recon = w0 * v0 + bary.x * v1 + bary.y * v2;
            float3 hit_pos = pos + dir * q.CommittedRayT();
            hit_albedo *= step(length(recon - hit_pos), 0.5);

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
