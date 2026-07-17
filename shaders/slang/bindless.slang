// Bindless PBR fragment stage — descriptor-indexing variant of pbr.slang.
//
// Material parameters live in a per-frame SSBO (`materials[]`); per-draw
// state is reduced to the model matrix, the material slot, the env cubemap
// bindless handle, and the albedo / normal-map bindless slots. Albedo /
// normal / metallic-roughness / emissive textures are sampled through the
// same bindless SRV table that the rest of the engine uses (`bindless` set
// in `Rust` BindlessTextureTable). The 1x1 magenta fallback is reserved
// at slot 0.
//
// This file is the descriptor mirror of the Rust `PbrBindlessPushConstants`
// (push constants), `GpuMaterial` (SSBO), and `BindlessTextureTable` (SRV
// set). Any field added or moved here must be reflected in those types and
// regenerated via `cargo run -p xtask --bin shader-bindgen -- \
// shaders/reflection crates/prism-render/src/shader_bindings.rs`.

import common;

// ---- set 0, binding 0 — per-frame UBO (unchanged) -------------------
[[vk::binding(0, 0)]]
ConstantBuffer<FrameUBO> frame;

// ---- set 0, binding 1 — material SSBO --------------------------------
// One record per material; the shader indexes by `pc.material_slot`.
// Layout must match Rust `GpuMaterial` (48 bytes, 16-byte aligned).
struct GpuMaterial
{
    float4 base_color;                          // offset 0
    float4 metallic_roughness_emissive;         // offset 16: x=metallic, y=roughness, z=emissive
    uint   albedo_idx;                          // offset 32
    uint   normal_idx;                          // offset 36
    uint   metallic_roughness_idx;             // offset 40
    uint   emissive_idx;                        // offset 44
};
[[vk::binding(1, 0)]]
StructuredBuffer<GpuMaterial> materials;

// ---- set 1, binding 0 — bindless SAMPLER array (4 modes) ------------
[[vk::binding(0, 1)]]
SamplerState globalSamplers[];

// ---- set 1, binding 1 — bindless SAMPLED_IMAGE array -----------------
// Slot 0 is the magenta fallback; the rest hold user textures (albedo,
// normal, metallic-roughness, emissive, etc.).
[[vk::binding(1, 1)]]
Texture2D bindlessSrvs[];

// ---- set 2, binding 0 - IBL environment cubemap ----------------------
// Combined image sampler pointing at the engine's environment cubemap
// (`IblResources` in Rust). Shared with the legacy PBR path, which binds
// the same descriptor set at set 1. Here it lives at set 2 so the bindless
// table (samplers + 2D SRVs) can stay at set 1 untouched.
[[vk::binding(0, 2)]]
SamplerCube envCube;

// ---- push constants -------------------------------------------------
// Byte layout mirrors Rust `PbrBindlessPushConstants` (96 bytes):
//   model(64) | material_slot(4) | env_handle(4) | albedo_idx(4) | normal_idx(4) | _pad(16)
struct PbrBindlessPush
{
    float4x4 model;
    uint     material_slot;
    uint     env_handle;
    uint     albedo_idx;
    uint     normal_idx;
    uint4    _padding;
};
[[vk::push_constant]]
PbrBindlessPush pc;

static const float PI          = 3.14159265359;
static const float IBL_MAX_LOD = 8.0;

static const uint SAMPLER_LINEAR_WRAP  = 0u;
static const uint SAMPLER_LINEAR_CLAMP = 1u;
static const uint SAMPLER_NEAREST      = 2u;
static const uint SAMPLER_SHADOW       = 3u;

// NonUniformResourceIndex: required by the Vulkan spec whenever the index
// into a descriptor array can differ across a quad/subgroup. Even when
// the handle is uniform here, marking it is safe and future-proofs
// per-material handles.
//
// IBL: the engine's environment cubemap is bound at set 2 (SamplerCube
// envCube), shared with the legacy PBR path. Diffuse irradiance is sampled
// at a fixed mid mip (approximating a convolved irradiance map), specular
// is sampled at a roughness-driven mip using the cubemap's mip chain as a
// cheap prefiltered-env approximation. `pc.env_handle` is kept in the push
// constant for a future bindless-cube-array path but is unused here.
float3 sample_irradiance(float3 n)
{
    return envCube.SampleLevel(n, 4.0).rgb;
}

float3 sample_specular(float3 r, float roughness)
{
    return envCube.SampleLevel(r, roughness * IBL_MAX_LOD).rgb;
}

float3 fresnel_schlick(float cos_theta, float3 f0)
{
    return f0 + (1.0 - f0) * pow(clamp(1.0 - cos_theta, 0.0, 1.0), 5.0);
}

float3 fresnel_schlick_roughness(float cos_theta, float3 f0, float roughness)
{
    return f0 + (max(float3(1.0 - roughness), f0) - f0) * pow(clamp(1.0 - cos_theta, 0.0, 1.0), 5.0);
}

float distribution_ggx(float3 n, float3 h, float roughness)
{
    float a = roughness * roughness;
    float a2 = a * a;
    float n_dot_h = max(dot(n, h), 0.0);
    float n_dot_h2 = n_dot_h * n_dot_h;
    float denom = n_dot_h2 * (a2 - 1.0) + 1.0;
    return a2 / max(PI * denom * denom, 1e-5);
}

float geometry_schlick_ggx(float n_dot_v, float roughness)
{
    float r = roughness + 1.0;
    float k = (r * r) / 8.0;
    return n_dot_v / (n_dot_v * (1.0 - k) + k);
}

float geometry_smith(float3 n, float3 v, float3 l, float roughness)
{
    float n_dot_v = max(dot(n, v), 0.0);
    float n_dot_l = max(dot(n, l), 0.0);
    return geometry_schlick_ggx(n_dot_v, roughness) * geometry_schlick_ggx(n_dot_l, roughness);
}

float3 aces(float3 x)
{
    const float a = 2.51, b = 0.03, c = 2.43, d = 0.59, e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), 0.0, 1.0);
}

[shader("fragment")]
float4 fragmentMain(MeshVaryings in) : SV_Target
{
    // ---- Material -------------------------------------------------------
    GpuMaterial mat = materials[pc.material_slot];

    // Base color: prefer the texture if a valid slot was bound, else use
    // the scalar. Texture index 0xFFFFFFFF means "no texture".
    float3 albedo;
    if (mat.albedo_idx != 0xFFFFFFFFu)
    {
        float4 sampled = bindlessSrvs[NonUniformResourceIndex(mat.albedo_idx)]
            .Sample(globalSamplers[NonUniformResourceIndex(SAMPLER_LINEAR_WRAP)], in.uv);
        // Alpha-test for cut-out foliage / alpha-masked geometry (Sponza
        // plants): discard texels below the threshold so they don't render
        // as opaque quads. Opaque materials have alpha = 1 and pass through.
        if (sampled.a < 0.5)
        {
            discard;
        }
        // Approximate sRGB -> linear (gamma 2.2 is fine for the
        // high-energy pre-tonemap range; the post pass tone-maps anyway).
        albedo = pow(sampled.rgb, float3(2.2));
    }
    else
    {
        albedo = mat.base_color.rgb;
    }

    // Metallic / roughness: same pattern, packed (B=metallic, G=roughness
    // per glTF) when a texture is present.
    float metallic;
    float roughness;
    if (mat.metallic_roughness_idx != 0xFFFFFFFFu)
    {
        float4 mr = bindlessSrvs[NonUniformResourceIndex(mat.metallic_roughness_idx)]
            .Sample(globalSamplers[NonUniformResourceIndex(SAMPLER_LINEAR_WRAP)], in.uv);
        metallic  = mr.b * mat.metallic_roughness_emissive.x;
        roughness = mr.g * mat.metallic_roughness_emissive.y;
    }
    else
    {
        metallic  = mat.metallic_roughness_emissive.x;
        roughness = mat.metallic_roughness_emissive.y;
    }
    roughness = clamp(roughness, 0.04, 1.0);

    // Normal: derive from tangent-space normal map if a valid slot is
    // bound, else use the vertex geometric normal.
    float3 n;
    if (mat.normal_idx != 0xFFFFFFFFu)
    {
        float3 n_ts = bindlessSrvs[NonUniformResourceIndex(mat.normal_idx)]
            .Sample(globalSamplers[NonUniformResourceIndex(SAMPLER_LINEAR_WRAP)], in.uv).xyz;
        n_ts = n_ts * 2.0 - 1.0;
        // TBN basis: tangent (in.tangent), bitangent = cross(n, t),
        // normal (in.worldNormal). The handedness sign is folded into
        // the bitangent for left-handed UVs.
        float3 t = normalize(in.tangent);
        float3 b = cross(in.worldNormal, t);
        n = normalize(n_ts.x * t + n_ts.y * b + n_ts.z * in.worldNormal);
    }
    else
    {
        n = normalize(in.worldNormal);
    }

    float3 v = normalize(frame.cameraPosition.xyz - in.worldPos);
    float3 r = reflect(-v, n);

    float3 f0 = lerp(float3(0.04), albedo, metallic);

    float3 light_dir = normalize(frame.lightDirection.xyz);
    float3 h         = normalize(v + light_dir);
    float  n_dot_l   = max(dot(n, light_dir), 0.0);

    float3 radiance = frame.lightColor.rgb * frame.lightDirection.w;
    float  ndf = distribution_ggx(n, h, roughness);
    float  g   = geometry_smith(n, v, light_dir, roughness);
    float3 f   = fresnel_schlick(max(dot(h, v), 0.0), f0);

    float3 numerator   = ndf * g * f;
    float  denominator = 4.0 * max(dot(n, v), 0.0) * n_dot_l + 1e-4;
    float3 specular    = numerator / denominator;

    float3 kd     = (float3(1.0) - f) * (1.0 - metallic);
    float3 direct = (kd * albedo / PI + specular) * radiance * n_dot_l;

    float3 f_ibl  = fresnel_schlick_roughness(max(dot(n, v), 0.0), f0, roughness);
    float3 kd_ibl = (float3(1.0) - f_ibl) * (1.0 - metallic);

    // Image-based lighting. The env cubemap is NOT pre-convolved into a
    // diffuse irradiance map, so sampling it raw tints everything with the
    // sky color (blue). Until a real irradiance convolution is added, use a
    // neutral hemispheric ambient bright enough to read albedo indoors:
    // top-facing normals get a brighter sky term, bottom-facing a ground term.
    // Specular IBL uses the same neutral constant (no env-cube sample) so no
    // blue reflection splashes appear on rough surfaces.
    float  ambient = frame.lightColor.w;
    float3 up       = float3(0.0, 1.0, 0.0);
    float  hemi     = dot(n, up) * 0.5 + 0.5;          // 0..1
    float3 sky_term = float3(0.50, 0.50, 0.50);
    float3 gnd_term = float3(0.25, 0.25, 0.25);
    float3 irradiance = lerp(gnd_term, sky_term, hemi);
    float3 diffuse_ibl = irradiance * albedo;
    // Specular IBL: neutral constant reflection scaled by roughness. No
    // env-cube sample (would tint blue). Fresnel-roughness still gives
    // metals/grazing a brighter sheen.
    float2 brdf         = float2(1.0 - roughness, roughness);
    float3 specular_ibl = irradiance * (f_ibl * brdf.x + brdf.y);
    float3 ibl = (kd_ibl * diffuse_ibl + specular_ibl) * ambient;

    // Emissive: texture (multiplied by per-material intensity) or scalar.
    float3 emissive = mat.metallic_roughness_emissive.zzz;
    if (mat.emissive_idx != 0xFFFFFFFFu)
    {
        float3 e = bindlessSrvs[NonUniformResourceIndex(mat.emissive_idx)]
            .Sample(globalSamplers[NonUniformResourceIndex(SAMPLER_LINEAR_WRAP)], in.uv).rgb;
        emissive = e * mat.metallic_roughness_emissive.z;
    }

    float3 color = direct + ibl + emissive;

    // Reinhard tonemap (more predictable than ACES for tuning - no white
    // crush in the mid range). Exposure scales the HDR color before the
    // curve; 1.0 leaves it as-is.
    float exposure = 1.2;
    color = color * exposure;
    color = color / (color + 1.0);
    return float4(color, 1.0);
}
