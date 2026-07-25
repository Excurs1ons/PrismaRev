// Real-time path-tracing renderer with ReSTIR DI — compute shader with
// temporal accumulation.
//
// Each frame traces ONE sample per pixel and accumulates the result across
// frames (1 sample/frame -> noise converges over time).
//
// Dispatch: ((width+15)/16, (height+15)/16, 1) with numthreads(16,16,1).
// Requires: VK_KHR_ray_query (RTX hardware).
//
// Output: The accumulated radiance ÷ sample count is written to
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
//     b8 StructuredBuffer<PtAnalyticLight> ptLights (module-global)
//     b9 StructuredBuffer<PtEmissiveTri> ptEmissive (module-global)
//    b10 StructuredBuffer<ReSTIRReservoir> prevReservoir (temporal reuse)
//    b11 RWStructuredBuffer<ReSTIRReservoir> currReservoir (write for next frame)
//   set 1 (shared bindless): globalSamplers[] + bindlessSrvs[] (module-global)
// The b6/b7/b8/b9 + set 1 bindings are declared in path_integrator.slang (imported).

#include "path_integrator.slang"

// ---- ReSTIR constants ----
static const uint RESTIR_M = 8u;               // RIS candidates per pixel
static const uint RESTIR_SPATIAL_SAMPLES = 4u; // spatial reuse neighbors

// ---- Push constants ----
struct PtPush
{
    float4x4 inv_view_proj;  // 0: NDC -> world
    float4   camera_pos;     // 64: .xyz = camera pos, .w = exposure
    float4   light_dir;      // 80: xyz = dir TO light, w = pre-scaled radiance
    float4   light_color;    // 96: rgb = directional color, w = ambient factor
    uint4    params;         // 112: x=w, y=h, z=max_bounce, w=packed(frame|reset<<31)
    float    ray_max_distance; // 128: max ray length
    uint     max_iterations;   // 132: 0 = accumulate forever
    uint     num_lights;       // 136: analytic lights in ptLights
    uint     num_emissive;     // 140: emissive triangles in ptEmissive
};

// ---- Descriptors (set 0) ----
[[vk::binding(0, 0)]]
[format("rgba32f")]
RWTexture2D<float4> accumImage;

[[vk::binding(1, 0)]]
[format("r32ui")]
RWTexture2D<uint> sampleCount;

[[vk::binding(2, 0)]]
RaytracingAccelerationStructure tlas;

[[vk::binding(3, 0)]]
ByteAddressBuffer vertexData;

[[vk::binding(4, 0)]]
StructuredBuffer<uint> indices;

[[vk::binding(5, 0)]]
[format("rgba32f")]
RWTexture2D<float4> outputImage;

[[vk::binding(10, 0)]]
StructuredBuffer<ReSTIRReservoir> prevReservoir;

[[vk::binding(11, 0)]]
RWStructuredBuffer<ReSTIRReservoir> currReservoir;

[[vk::push_constant]]
PtPush push;

// ---- Groupshared memory for ReSTIR spatial reuse ----
groupshared ReSTIRReservoir gs_reservoirs[16][16];

// ---- Ray generation ----
void generate_ray(uint px, uint py, float jx, float jy,
    out float3 origin, out float3 dir)
{
    uint w = push.params.x;
    uint h = push.params.y;
    float ndc_x = ((float(px) + 0.5 + jx) / float(w)) * 2.0 - 1.0;
    float ndc_y = ((float(py) + 0.5 + jy) / float(h)) * 2.0 - 1.0;
    float4 clip = float4(ndc_x, ndc_y, 0.0, 1.0);
    float4 world = mul(push.inv_view_proj, clip);
    float3 world_pt = world.xyz / world.w;
    origin = push.camera_pos.xyz;
    dir = normalize(world_pt - origin);
}

// ---- Trace a ray and return hit data ----
bool trace_primary_ray(float3 origin, float3 dir, float ray_max_distance,
    out float3 hit_pos, out float3 hit_normal, out SurfaceMaterial surf,
    out PtInstanceMeta meta, out float2 hit_uv, out float3 view_dir)
{
    RayDesc ray;
    ray.Origin = origin;
    ray.Direction = dir;
    ray.TMin = 0.01;
    ray.TMax = ray_max_distance;

    RayQuery<RAY_FLAG_NONE> q;
    q.TraceRayInline(tlas, RAY_FLAG_NONE, 0xFF, ray);
    while (q.Proceed())
    {
        if (q.CandidateType() == CANDIDATE_NON_OPAQUE_TRIANGLE)
        {
            uint ci = q.CandidateInstanceID();
            uint cp = q.CandidatePrimitiveIndex();
            float2 cb = q.CandidateTriangleBarycentrics();
            float3 cv0, cv1, cv2;
            float2 cuv0, cuv1, cuv2;
            float3 cn0, cn1, cn2;
            read_hit_triangle(ci, cp, vertexData, indices,
                cv0, cv1, cv2, cuv0, cuv1, cuv2, cn0, cn1, cn2);
            float cw = 1.0 - cb.x - cb.y;
            float2 cuv = cw * cuv0 + cb.x * cuv1 + cb.y * cuv2;
            PtInstanceMeta cm = instance_meta[ci];
            GpuMaterial c_mat = materials[cm.material_slot];
            if (mat_albedo_alpha(c_mat, cuv) >= 0.5)
                q.CommitNonOpaqueTriangleHit();
        }
    }

    if (q.CommittedStatus() != COMMITTED_TRIANGLE_HIT)
        return false;

    uint instance_id = q.CommittedInstanceID();
    uint prim_idx = q.CommittedPrimitiveIndex();

    float3 v0, v1, v2;
    float2 uv0, uv1, uv2;
    float3 n0, n1, n2;
    read_hit_triangle(instance_id, prim_idx, vertexData, indices,
        v0, v1, v2, uv0, uv1, uv2, n0, n1, n2);

    float2 bary = q.CommittedTriangleBarycentrics();
    float w0 = 1.0 - bary.x - bary.y;
    hit_pos = origin + dir * q.CommittedRayT();

    float3 e1 = v1 - v0;
    float3 e2 = v2 - v0;
    float3 geo_normal = cross(e1, e2);
    if (dot(geo_normal, geo_normal) < 1e-12) return false;
    geo_normal *= rsqrt(dot(geo_normal, geo_normal));

    hit_normal = normalize(w0 * n0 + bary.x * n1 + bary.y * n2);
    if (dot(hit_normal, dir) > 0.0)
        hit_normal = -hit_normal;

    view_dir = -dir;

    meta = instance_meta[instance_id];
    GpuMaterial mat = materials[meta.material_slot];
    hit_uv = w0 * uv0 + bary.x * uv1 + bary.y * uv2;
    surf = resolve_material(mat, hit_uv);
    return true;
}

// ---- Evaluate a single light for ReSTIR (bounce 0 direct) ----
float3 eval_direct_light(uint light_idx, float3 hit_pos, float3 hit_normal,
    float3 view_dir, SurfaceMaterial surf, float3 sun_dir, float3 sun_radiance,
    float ray_max_distance, out float out_pdf)
{
    // Resolve the selected light
    float3 L_dir, L_rad;
    float dist2;
    restir_resolve_light(light_idx, hit_pos, L_dir, L_rad, dist2,
        sun_dir, sun_radiance);

    out_pdf = 0.0;
    if (dot(L_rad, L_rad) < 1e-10 || dot(hit_normal, L_dir) <= 0.0)
        return float3(0.0);

    // Shadow ray
    float shadow = 1.0;
    {
        RayDesc sr;
        sr.Origin = hit_pos + 0.0001 * L_dir;
        sr.Direction = L_dir;
        sr.TMin = 0.001;
        sr.TMax = ray_max_distance;
        RayQuery<RAY_FLAG_SKIP_CLOSEST_HIT_SHADER> sq;
        sq.TraceRayInline(tlas, RAY_FLAG_SKIP_CLOSEST_HIT_SHADER, 0xFF, sr);
        while (sq.Proceed())
        {
            if (sq.CandidateType() == CANDIDATE_NON_OPAQUE_TRIANGLE)
            {
                uint si = sq.CandidateInstanceID();
                uint sp = sq.CandidatePrimitiveIndex();
                float2 sb = sq.CandidateTriangleBarycentrics();
                float3 sv0, sv1, sv2;
                float2 suv0, suv1, suv2;
                float3 sn0, sn1, sn2;
                read_hit_triangle(si, sp, vertexData, indices,
                    sv0, sv1, sv2, suv0, suv1, suv2, sn0, sn1, sn2);
                float sw = 1.0 - sb.x - sb.y;
                float2 suv = sw * suv0 + sb.x * suv1 + sb.y * suv2;
                PtInstanceMeta sm = instance_meta[si];
                GpuMaterial smat = materials[sm.material_slot];
                if (mat_albedo_alpha(smat, suv) >= 0.5)
                {
                    sq.CommitNonOpaqueTriangleHit();
                    break;
                }
            }
        }
        if (sq.CommittedStatus() == COMMITTED_TRIANGLE_HIT)
            shadow = 0.0;
    }

    float3 brdf = eval_brdf(surf, hit_normal, view_dir, L_dir);
    float p_brdf = eval_brdf_pdf(surf, hit_normal, view_dir, L_dir);

    // PDF for ReSTIR-selected light = restir_eff_pdf from the reservoir.
    // The caller provides it via out_pdf and composites MIS weight.
    // Here we just return the BRDF*radiance*shadow term.
    return brdf * L_rad * shadow;
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

    // ---- RNG ----
    uint seed = tid.x * 73856093u + tid.y * 19349663u + frame_count * 2567483615u;
    float jx = rand_float(seed) - 0.5;
    float jy = rand_float(seed) - 0.5;

    float3 origin, dir;
    generate_ray(tid.x, tid.y, jx, jy, origin, dir);

    float3 total_radiance = float3(0.0, 0.0, 0.0);
    float3 throughput = float3(1.0, 1.0, 1.0);

    float3 sun_radiance = push.light_color.rgb * push.light_dir.w;

    // ---- Bounce 0: primary ray + ReSTIR direct light ----
    float3 hit_pos, hit_normal;
    SurfaceMaterial surf;
    PtInstanceMeta meta;
    float2 hit_uv;
    float3 view_dir;

    if (trace_primary_ray(origin, dir, push.ray_max_distance,
        hit_pos, hit_normal, surf, meta, hit_uv, view_dir))
    {
        // ---- Emissive at hit ----
        total_radiance += throughput * surf.emissive;

        // ---- ReSTIR DI ----
        // RIS initial sampling
        ReSTIRReservoir reservoir = restir_ris_init(
            surf, hit_normal, view_dir, hit_pos,
            push.light_dir.xyz, sun_radiance,
            push.num_lights, push.num_emissive,
            RESTIR_M, seed);

        uint linear_idx = pixel.y * push.params.x + pixel.x;

        // Temporal reuse (always combine with previous, no reprojection)
        ReSTIRReservoir prev = prevReservoir[linear_idx];
        reservoir = restir_combine(reservoir, prev, 1.0, seed);

        // Groupshared write + barrier for spatial reuse
        uint2 ltid = uint2(tid.x & 15u, tid.y & 15u);
        gs_reservoirs[ltid.y][ltid.x] = reservoir;
        GroupMemoryBarrierWithGroupSync();

        // Spatial reuse from random neighbors within the thread group
        [unroll]
        for (uint si = 0u; si < RESTIR_SPATIAL_SAMPLES; si++)
        {
            uint2 nbr = uint2(
                (ltid.x + 1u + uint(rand_float(seed) * 14.0)) & 15u,
                (ltid.y + 1u + uint(rand_float(seed) * 14.0)) & 15u);
            ReSTIRReservoir nbr_r = gs_reservoirs[nbr.y][nbr.x];
            reservoir = restir_combine(reservoir, nbr_r, 1.0, seed);
        }

        // Write reservoir for next frame's temporal reuse
        currReservoir[linear_idx] = reservoir;

        // ---- ReSTIR-selected direct light ----
        float restir_eff_pdf = restir_effective_pdf(reservoir);
        if (restir_eff_pdf > 0.0)
        {
            float3 L_dir, L_rad;
            float dist2;
            restir_resolve_light(reservoir.light_idx, hit_pos, L_dir, L_rad, dist2,
                push.light_dir.xyz, sun_radiance);

            float shadow = 1.0;
            if (dot(L_rad, L_rad) > 1e-10 && dot(hit_normal, L_dir) > 0.0)
            {
                // Shadow ray
                RayDesc sr;
                sr.Origin = hit_pos + 0.0001 * L_dir;
                sr.Direction = L_dir;
                sr.TMin = 0.001;
                sr.TMax = push.ray_max_distance;
                RayQuery<RAY_FLAG_SKIP_CLOSEST_HIT_SHADER> sq;
                sq.TraceRayInline(tlas, RAY_FLAG_SKIP_CLOSEST_HIT_SHADER, 0xFF, sr);
                while (sq.Proceed())
                {
                    if (sq.CandidateType() == CANDIDATE_NON_OPAQUE_TRIANGLE)
                    {
                        uint si = sq.CandidateInstanceID();
                        uint sp = sq.CandidatePrimitiveIndex();
                        float2 sb = sq.CandidateTriangleBarycentrics();
                        float3 sv0, sv1, sv2;
                        float2 suv0, suv1, suv2;
                        float3 sn0, sn1, sn2;
                        read_hit_triangle(si, sp, vertexData, indices,
                            sv0, sv1, sv2, suv0, suv1, suv2, sn0, sn1, sn2);
                        float sw = 1.0 - sb.x - sb.y;
                        float2 suv = sw * suv0 + sb.x * suv1 + sb.y * suv2;
                        PtInstanceMeta sm = instance_meta[si];
                        GpuMaterial smat = materials[sm.material_slot];
                        if (mat_albedo_alpha(smat, suv) >= 0.5)
                        {
                            sq.CommitNonOpaqueTriangleHit();
                            break;
                        }
                    }
                }
                if (sq.CommittedStatus() == COMMITTED_TRIANGLE_HIT)
                    shadow = 0.0;

                float3 brdf_re = eval_brdf(surf, hit_normal, view_dir, L_dir);
                float p_brdf_re = eval_brdf_pdf(surf, hit_normal, view_dir, L_dir);
                total_radiance += throughput * brdf_re * L_rad * shadow
                    / max(restir_eff_pdf + p_brdf_re, 1e-20);
            }

            // ---- Point-light NEE fallback when ReSTIR-selected light is occluded ----
            // ReSTIR tends to select the sun even in shadow (target = BRDF*radiance
            // doesn't account for visibility). When occluded the contribution is 0, so
            // we sample one random analytic light as a fallback to keep shadow regions
            // from converging to only 10%-of-frames point-light samples.
            if (shadow < 0.5 && push.num_lights > 0u)
            {
                uint fli = uint(rand_float(seed) * float(push.num_lights));
                fli = min(fli, push.num_lights - 1u);
                float3 fl_dir, fl_rad;
                float fl_dist2;
                restir_resolve_light(fli + 1u, hit_pos, fl_dir, fl_rad, fl_dist2,
                    push.light_dir.xyz, sun_radiance);
                if (dot(fl_rad, fl_rad) > 1e-10 && dot(hit_normal, fl_dir) > 0.0)
                {
                    float fl_shadow = 1.0;
                    RayDesc fsr;
                    fsr.Origin = hit_pos + 0.0001 * fl_dir;
                    fsr.Direction = fl_dir;
                    fsr.TMin = 0.001;
                    fsr.TMax = push.ray_max_distance;
                    RayQuery<RAY_FLAG_SKIP_CLOSEST_HIT_SHADER> fsq;
                    fsq.TraceRayInline(tlas, RAY_FLAG_SKIP_CLOSEST_HIT_SHADER, 0xFF, fsr);
                    while (fsq.Proceed())
                    {
                        if (fsq.CandidateType() == CANDIDATE_NON_OPAQUE_TRIANGLE)
                        {
                            uint fi = fsq.CandidateInstanceID();
                            uint fp = fsq.CandidatePrimitiveIndex();
                            float2 fb = fsq.CandidateTriangleBarycentrics();
                            float3 fv0, fv1, fv2;
                            float2 fuv0, fuv1, fuv2;
                            float3 fn0, fn1, fn2;
                            read_hit_triangle(fi, fp, vertexData, indices,
                                fv0, fv1, fv2, fuv0, fuv1, fuv2, fn0, fn1, fn2);
                            float fw = 1.0 - fb.x - fb.y;
                            float2 fuv = fw * fuv0 + fb.x * fuv1 + fb.y * fuv2;
                            PtInstanceMeta fm = instance_meta[fi];
                            GpuMaterial fmat = materials[fm.material_slot];
                            if (mat_albedo_alpha(fmat, fuv) >= 0.5)
                            {
                                fsq.CommitNonOpaqueTriangleHit();
                                break;
                            }
                        }
                    }
                    if (fsq.CommittedStatus() == COMMITTED_TRIANGLE_HIT)
                        fl_shadow = 0.0;

                    if (fl_shadow > 0.0)
                    {
                        float p_nee_fl = 1.0 / max(float(push.num_lights), 1.0);
                        float3 brdf_fl = eval_brdf(surf, hit_normal, view_dir, fl_dir);
                        float p_brdf_fl = eval_brdf_pdf(surf, hit_normal, view_dir, fl_dir);
                        total_radiance += throughput * brdf_fl * fl_rad * fl_shadow
                            / max(p_nee_fl + p_brdf_fl, 1e-20);
                    }
                }
            }
        }

        // ---- Emissive triangle NEE (bounce 0, independent of ReSTIR) ----
        if (push.num_emissive > 0u)
        {
            uint ei = uint(rand_float(seed) * float(push.num_emissive));
            ei = min(ei, push.num_emissive - 1u);
            PtEmissiveTri et = ptEmissive[ei];
            float r1 = sqrt(rand_float(seed));
            float r2 = rand_float(seed);
            float b1 = 1.0 - r1;
            float b2 = r2 * r1;
            float b0 = 1.0 - b1 - b2;
            float3 et_pt = b0 * et.v0.xyz + b1 * et.v1.xyz + b2 * et.v2.xyz;
            float3 et_n = et.normal.xyz;
            float3 to_l = et_pt - hit_pos;
            float l_dist2 = dot(to_l, to_l);
            float l_dist = sqrt(l_dist2);
            float3 L_dir_et = to_l / max(l_dist, 1e-8);
            float cos_light_et = abs(dot(et_n, -L_dir_et));
            float p_nee_et = (1.0 / max(float(push.num_emissive) * et.area, 1e-8))
                * l_dist2 / max(cos_light_et, 1e-8);

            float et_shadow = 1.0;
            if (dot(hit_normal, L_dir_et) > 0.0 && cos_light_et > 1e-4)
            {
                RayDesc sr;
                sr.Origin = hit_pos + 0.0001 * L_dir_et;
                sr.Direction = L_dir_et;
                sr.TMin = 0.001;
                sr.TMax = max(l_dist - 0.001, 0.001);
                RayQuery<RAY_FLAG_SKIP_CLOSEST_HIT_SHADER> sq;
                sq.TraceRayInline(tlas, RAY_FLAG_SKIP_CLOSEST_HIT_SHADER, 0xFF, sr);
                while (sq.Proceed())
                {
                    if (sq.CandidateType() == CANDIDATE_NON_OPAQUE_TRIANGLE)
                    {
                        uint si = sq.CandidateInstanceID();
                        uint sp = sq.CandidatePrimitiveIndex();
                        float2 sb = sq.CandidateTriangleBarycentrics();
                        float3 sv0, sv1, sv2;
                        float2 suv0, suv1, suv2;
                        float3 sn0, sn1, sn2;
                        read_hit_triangle(si, sp, vertexData, indices,
                            sv0, sv1, sv2, suv0, suv1, suv2, sn0, sn1, sn2);
                        float sw = 1.0 - sb.x - sb.y;
                        float2 suv = sw * suv0 + sb.x * suv1 + sb.y * suv2;
                        PtInstanceMeta sm = instance_meta[si];
                        GpuMaterial smat = materials[sm.material_slot];
                        if (mat_albedo_alpha(smat, suv) >= 0.5)
                        {
                            sq.CommitNonOpaqueTriangleHit();
                            break;
                        }
                    }
                }
                if (sq.CommittedStatus() == COMMITTED_TRIANGLE_HIT)
                    et_shadow = 0.0;
            }
            float3 et_brdf = eval_brdf(surf, hit_normal, view_dir, L_dir_et);
            float p_brdf_et = eval_brdf_pdf(surf, hit_normal, view_dir, L_dir_et);
            total_radiance += throughput * et_brdf * et.radiance.rgb * et_shadow
                * p_nee_et / max(p_nee_et * p_nee_et + p_brdf_et * p_brdf_et, 1e-20);
        }

        // ---- Russian roulette + sample next direction (bounce 0 → bounce 1) ----
        float p_rr = max(max(throughput.r, throughput.g), throughput.b);
        if (p_rr >= 1e-4)
        {
            float surv = min(p_rr, 1.0);
            if (rand_float(seed) < surv)
            {
                throughput /= surv;

                // Sample next direction
                if (surf.transmission > 0.0 && surf.metallic < 0.5)
                {
                    float cos_theta = max(dot(hit_normal, view_dir), 0.0);
                    float F = fresnel_dielectric(cos_theta, surf.ior);
                    if (rand_float(seed) < F)
                    {
                        float3 refl_dir = reflect(-view_dir, hit_normal);
                        dir = refl_dir;
                        origin = hit_pos + 0.0001 * dir;
                    }
                    else
                    {
                        float eta = 1.0 / surf.ior;
                        float3 refr_dir = refract(dir, hit_normal, eta);
                        if (dot(refr_dir, refr_dir) < 0.001)
                        {
                            dir = reflect(-view_dir, hit_normal);
                            origin = hit_pos + 0.0001 * dir;
                        }
                        else
                        {
                            dir = normalize(refr_dir);
                            origin = hit_pos - 0.0001 * dir;
                            throughput *= surf.trans_color / max(1.0 - F, 1e-7);
                        }
                    }
                }
                else
                {
                    float prob_diffuse = surf.roughness;
                    float3 new_dir;
                    if (rand_float(seed) < prob_diffuse)
                    {
                        float u1 = rand_float(seed);
                        float u2 = rand_float(seed);
                        new_dir = cosine_hemisphere_sample(hit_normal, u1, u2);
                    }
                    else
                    {
                        float real_roughness = surf.roughness * surf.roughness;
                        float3 h = ggx_sample(seed, real_roughness, hit_normal);
                        new_dir = reflect(-view_dir, h);
                        if (dot(hit_normal, new_dir) <= 0.0) { p_rr = 0; } // kill path
                    }
                    if (dot(hit_normal, new_dir) > 0.0)
                    {
                        float brdf_pdf = eval_brdf_pdf(surf, hit_normal, view_dir, new_dir);
                        if (brdf_pdf > 1e-7)
                        {
                            float3 brdf_cos = eval_brdf(surf, hit_normal, view_dir, new_dir);
                            throughput *= brdf_cos / brdf_pdf;
                            dir = new_dir;
                            origin = hit_pos + 0.0001 * new_dir;
                        }
                        else { p_rr = 0; }
                    }
                    else { p_rr = 0; }
                }
            }
            else { p_rr = 0; }
        }
        else { p_rr = 0; }

        // ---- Bounces 1+ (indirect, no NEE) ----
        for (uint bounce = 1u; bounce < push.params.z; bounce++)
        {
            if (p_rr < 1e-4) break;

            // Trace ray
            RayDesc ray;
            ray.Origin = origin;
            ray.Direction = dir;
            ray.TMin = 0.01;
            ray.TMax = push.ray_max_distance;

            RayQuery<RAY_FLAG_NONE> q;
            q.TraceRayInline(tlas, RAY_FLAG_NONE, 0xFF, ray);
            while (q.Proceed())
            {
                if (q.CandidateType() == CANDIDATE_NON_OPAQUE_TRIANGLE)
                {
                    uint ci = q.CandidateInstanceID();
                    uint cp = q.CandidatePrimitiveIndex();
                    float2 cb = q.CandidateTriangleBarycentrics();
                    float3 cv0, cv1, cv2;
                    float2 cuv0, cuv1, cuv2;
                    float3 cn0, cn1, cn2;
                    read_hit_triangle(ci, cp, vertexData, indices,
                        cv0, cv1, cv2, cuv0, cuv1, cuv2, cn0, cn1, cn2);
                    float cw = 1.0 - cb.x - cb.y;
                    float2 cuv = cw * cuv0 + cb.x * cuv1 + cb.y * cuv2;
                    PtInstanceMeta cm = instance_meta[ci];
                    GpuMaterial c_mat = materials[cm.material_slot];
                    if (mat_albedo_alpha(c_mat, cuv) >= 0.5)
                        q.CommitNonOpaqueTriangleHit();
                }
            }

            if (q.CommittedStatus() == COMMITTED_TRIANGLE_HIT)
            {
                uint iid = q.CommittedInstanceID();
                uint pid = q.CommittedPrimitiveIndex();
                float3 v0, v1, v2;
                float2 uv0, uv1, uv2;
                float3 n0, n1, n2;
                read_hit_triangle(iid, pid, vertexData, indices,
                    v0, v1, v2, uv0, uv1, uv2, n0, n1, n2);

                float2 b = q.CommittedTriangleBarycentrics();
                float wb = 1.0 - b.x - b.y;

                float3 e1 = v1 - v0;
                float3 e2 = v2 - v0;
                float3 gn = cross(e1, e2);
                if (dot(gn, gn) < 1e-12) break;
                gn *= rsqrt(dot(gn, gn));

                float3 hn = normalize(wb * n0 + b.x * n1 + b.y * n2);
                if (dot(hn, dir) > 0.0) hn = -hn;

                float3 vw_dir = -dir;
                float3 hp = origin + dir * q.CommittedRayT();
                float2 huv = wb * uv0 + b.x * uv1 + b.y * uv2;

                PtInstanceMeta m = instance_meta[iid];
                GpuMaterial mat = materials[m.material_slot];
                SurfaceMaterial hit_surf = resolve_material(mat, huv);

                // Emissive
                total_radiance += throughput * hit_surf.emissive;

                // ---- Bounce NEE: sample directional sun at indirect hit ----
                // Without NEE in bounce 1+, shadow regions get indirect light
                // only when the BRDF-sampled direction accidentally hits a
                // sun-lit surface — high variance produces colored noise.
                {
                    float bn_shadow = 1.0;
                    float3 bn_dir = push.light_dir.xyz;
                    if (dot(hn, bn_dir) > 0.0)
                    {
                        RayDesc sr;
                        sr.Origin = hp + 0.0001 * bn_dir;
                        sr.Direction = bn_dir;
                        sr.TMin = 0.001;
                        sr.TMax = push.ray_max_distance;
                        RayQuery<RAY_FLAG_SKIP_CLOSEST_HIT_SHADER> sq;
                        sq.TraceRayInline(tlas, RAY_FLAG_SKIP_CLOSEST_HIT_SHADER, 0xFF, sr);
                        while (sq.Proceed())
                        {
                            if (sq.CandidateType() == CANDIDATE_NON_OPAQUE_TRIANGLE)
                            {
                                uint si = sq.CandidateInstanceID();
                                uint sp = sq.CandidatePrimitiveIndex();
                                float2 sb = sq.CandidateTriangleBarycentrics();
                                float3 sv0, sv1, sv2;
                                float2 suv0, suv1, suv2;
                                float3 sn0, sn1, sn2;
                                read_hit_triangle(si, sp, vertexData, indices,
                                    sv0, sv1, sv2, suv0, suv1, suv2, sn0, sn1, sn2);
                                float sw = 1.0 - sb.x - sb.y;
                                float2 suv = sw * suv0 + sb.x * suv1 + sb.y * suv2;
                                PtInstanceMeta sm = instance_meta[si];
                                GpuMaterial smat = materials[sm.material_slot];
                                if (mat_albedo_alpha(smat, suv) >= 0.5)
                                {
                                    sq.CommitNonOpaqueTriangleHit();
                                    break;
                                }
                            }
                        }
                        if (sq.CommittedStatus() == COMMITTED_TRIANGLE_HIT)
                            bn_shadow = 0.0;
                    }
                    float3 bn_brdf = eval_brdf(hit_surf, hn, vw_dir, bn_dir);
                    float p_brdf_bn = eval_brdf_pdf(hit_surf, hn, vw_dir, bn_dir);
                    float p_nee_bn = 1.0; // sun is deterministic
                    total_radiance += throughput * bn_brdf * sun_radiance * bn_shadow
                        * p_nee_bn / max(p_nee_bn * p_nee_bn + p_brdf_bn * p_brdf_bn, 1e-20);
                }

                // Russian roulette
                float p = max(max(throughput.r, throughput.g), throughput.b);
                if (bounce + 1u >= push.params.z) break;
                if (p < 1e-4) break;

                float sv = min(p, 1.0);
                float rr = rand_float(seed);
                if (rr >= sv) break;
                throughput /= sv;

                // Next direction
                if (hit_surf.transmission > 0.0 && hit_surf.metallic < 0.5)
                {
                    float ct = max(dot(hn, vw_dir), 0.0);
                    float F = fresnel_dielectric(ct, hit_surf.ior);
                    if (rand_float(seed) < F)
                    {
                        dir = reflect(-vw_dir, hn);
                        origin = hp + 0.0001 * dir;
                    }
                    else
                    {
                        float eta = 1.0 / hit_surf.ior;
                        float3 rd = refract(dir, hn, eta);
                        if (dot(rd, rd) < 0.001)
                        {
                            dir = reflect(-vw_dir, hn);
                            origin = hp + 0.0001 * dir;
                        }
                        else
                        {
                            dir = normalize(rd);
                            origin = hp - 0.0001 * dir;
                            throughput *= hit_surf.trans_color / max(1.0 - F, 1e-7);
                        }
                    }
                }
                else
                {
                    float pd = hit_surf.roughness;
                    float3 nd;
                    if (rand_float(seed) < pd)
                    {
                        float u1 = rand_float(seed);
                        float u2 = rand_float(seed);
                        nd = cosine_hemisphere_sample(hn, u1, u2);
                    }
                    else
                    {
                        float rr2 = hit_surf.roughness * hit_surf.roughness;
                        float3 h = ggx_sample(seed, rr2, hn);
                        nd = reflect(-vw_dir, h);
                        if (dot(hn, nd) <= 0.0) { p_rr = 0; break; }
                    }
                    if (dot(hn, nd) <= 0.0) { break; }
                    float bpdf = eval_brdf_pdf(hit_surf, hn, vw_dir, nd);
                    if (bpdf <= 1e-7) break;
                    throughput *= eval_brdf(hit_surf, hn, vw_dir, nd) / bpdf;
                    dir = nd;
                    origin = hp + 0.0001 * nd;
                }
            }
            else
            {
                // Miss: env map
                total_radiance += throughput * envCube.SampleLevel(dir, 0.0).rgb;
                break;
            }
        }
    }
    else
    {
        // ---- Primary ray missed: HDR sky ----
        total_radiance += throughput * envCube.SampleLevel(dir, 0.0).rgb;
    }

    // ---- Firefly clamp: prevent individual bright samples from creating
    // persistent sparkles in the accumulation. The bound 50.0 corresponds to
    // ~140× a typical diffuse-sun bounce (0.36), so it only clips outliers.
    float ff_luma = dot(total_radiance, float3(0.2126, 0.7152, 0.0722));
    if (ff_luma > 50.0)
        total_radiance *= 50.0 / ff_luma;

    // ---- Accumulate ----
    float3 prev = accumImage[pixel].rgb;
    uint   count = sampleCount[pixel];

    float3 new_accum = prev + total_radiance;
    uint   new_count = count + 1u;

    accumImage[pixel] = float4(new_accum, 0.0);
    sampleCount[pixel] = new_count;

    float3 result = new_count > 0u ? new_accum / float(new_count) : float3(0.0);
    result *= push.camera_pos.w;
    outputImage[pixel] = float4(result, 1.0);
}
