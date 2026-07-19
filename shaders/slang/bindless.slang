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
// Layout must match Rust `GpuMaterial` (96 bytes, 16-byte aligned).
struct GpuMaterial
{
    float4 base_color;                          // offset 0
    float4 metallic_roughness_emissive;         // offset 16: x=metallic, y=roughness, z=emissive, w=emissive_strength
    uint   albedo_idx;                          // offset 32
    uint   normal_idx;                          // offset 36
    uint   metallic_roughness_idx;             // offset 40
    uint   emissive_idx;                        // offset 44
    // Second block (advanced PBR)
    float4 transmission_factor;                 // offset 48: x=transmission, y=ior, z=translucency, w=anisotropy
    float4 clearcoat;                           // offset 64: x=clearcoat, y=clearcoat_roughness, z=reserved, w=reserved
    uint   transmission_tex_idx;                // offset 80
    uint   clearcoat_tex_idx;                   // offset 84
    uint   _pad0;                               // offset 88
    uint   _pad1;                               // offset 92
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
//   model(64) | material_slot(4) | env_handle(4) | albedo_idx(4) | normal_idx(4)
//   | debug_flags(4) | _pad(12)
struct PbrBindlessPush
{
    float4x4 model;
    uint     material_slot;
    uint     env_handle;
    uint     albedo_idx;
    uint     normal_idx;
    uint     debug_flags;   // PBR component toggle bitmask (14 bits)
    uint3    _padding;
};
[[vk::push_constant]]
PbrBindlessPush pc;

static const float PI          = 3.14159265359;
static const float IBL_MAX_LOD = 8.0;

static const uint SAMPLER_LINEAR_WRAP  = 0u;
static const uint SAMPLER_LINEAR_CLAMP = 1u;
static const uint SAMPLER_NEAREST      = 2u;
static const uint SAMPLER_SHADOW       = 3u;

// PBR component toggle bits (mirror scene_bindless.slang and app.rs).
// flags=0 -> raw baseColor (unlit).
static const uint PBR_FLAG_DIRECT       = 0u;
static const uint PBR_FLAG_AMBIENT_IBL  = 1u;
static const uint PBR_FLAG_SPECULAR     = 2u;
static const uint PBR_FLAG_METALLIC     = 3u;
static const uint PBR_FLAG_ROUGHNESS    = 4u;
static const uint PBR_FLAG_DIFFUSE_IBL  = 5u;
static const uint PBR_FLAG_SPECULAR_IBL = 6u;
static const uint PBR_FLAG_MULTILIGHT   = 7u;
static const uint PBR_FLAG_SHADOW       = 8u;
static const uint PBR_FLAG_EMISSIVE     = 9u;
static const uint PBR_FLAG_TRANSMISSION = 10u;
static const uint PBR_FLAG_TRANSLUCENCY = 11u;
static const uint PBR_FLAG_ANISOTROPY   = 12u;
static const uint PBR_FLAG_CLEARCOAT    = 13u;

bool flag(uint b) { return (pc.debug_flags & (1u << b)) != 0u; }

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

// ---- Advanced material BRDFs (gated by flag bits). Mirror the
// implementations in scene_bindless.slang. ----

/// Thin-surface transmission approximation.
float3 eval_transmission(GpuMaterial mat, MeshVaryings in, float3 v, float3 n)
{
    float3 light_dir = normalize(frame.lightDirection.xyz);
    float3 radiance = frame.lightColor.rgb * frame.lightDirection.w;
    float metallic = mat.metallic_roughness_emissive.x;
    float3 base_color = mat.base_color.rgb;
    float transmission = mat.transmission_factor.x;
    float n_dot_l_back = max(dot(-n, light_dir), 0.0);
    return transmission * (1.0 - metallic) * base_color * radiance * n_dot_l_back;
}

/// Translucency / subsurface approximation using wrap-lighting.
float3 eval_translucency(GpuMaterial mat, float3 n, float3 v, float3 l, float3 radiance)
{
    float3 base_color = mat.base_color.rgb;
    float translucency = mat.transmission_factor.z;
    float wrap = translucency;
    float n_dot_l_wrap = (max(dot(n, l) + wrap, 0.0) / (1.0 + wrap));
    return translucency * base_color * radiance * n_dot_l_wrap;
}

/// Anisotropic GGX specular lobe.
float3 eval_anisotropy(GpuMaterial mat, float3 n, float3 v, float3 l, float3 h,
                       float roughness, float3 f0, float3 radiance, float shadow,
                       float3 tangent)
{
    float anisotropy = mat.transmission_factor.w;
    if (anisotropy <= 0.0)
        return float3(0.0);

    float3 t = normalize(tangent);
    if (dot(t, t) < 0.001)
        t = normalize(cross(n, float3(0.0, 1.0, 0.0)));
    t = normalize(t - n * dot(n, t));
    float3 b = cross(n, t);

    float3 h_ts = float3(dot(h, t), dot(h, b), dot(h, n));

    float ax = max(roughness * (1.0 + anisotropy), 0.001);
    float ay = max(roughness / (1.0 + anisotropy), 0.001);
    float ax2 = ax * ax;
    float ay2 = ay * ay;

    float hx2 = h_ts.x * h_ts.x;
    float hy2 = h_ts.y * h_ts.y;
    float hz2 = h_ts.z * h_ts.z;
    float denom = hx2 / ax2 + hy2 / ay2 + hz2;
    float d = 1.0 / (PI * ax * ay * denom * denom);

    float n_dot_v = max(dot(n, v), 0.0);
    float n_dot_l = max(dot(n, l), 0.0);
    float g = geometry_smith(n, v, l, roughness);
    float3 f = fresnel_schlick(max(dot(h, v), 0.0), f0);

    float3 specular = (d * g * f) / (4.0 * n_dot_v * n_dot_l + 1e-4);
    return specular * radiance * n_dot_l * shadow;
}

/// Clearcoat: a second dielectric GGX specular lobe.
float3 eval_clearcoat(GpuMaterial mat, float3 n, float3 v, float3 l, float3 h,
                      float3 radiance, float shadow)
{
    float clearcoat_factor = mat.clearcoat.x;
    float clearcoat_roughness = mat.clearcoat.y;
    if (clearcoat_factor <= 0.0)
        return float3(0.0);

    float n_dot_v = max(dot(n, v), 0.0);
    float n_dot_l = max(dot(n, l), 0.0);
    if (n_dot_l <= 0.0 || n_dot_v <= 0.0)
        return float3(0.0);

    float3 f0_cc = float3(0.04);
    float ndf_cc = distribution_ggx(n, h, clearcoat_roughness);
    float g_cc = geometry_smith(n, v, l, clearcoat_roughness);
    float3 f_cc = fresnel_schlick(max(dot(h, v), 0.0), f0_cc);
    float3 specular_cc = (ndf_cc * g_cc * f_cc) / (4.0 * n_dot_v * n_dot_l + 1e-4);
    return clearcoat_factor * specular_cc * radiance * n_dot_l * shadow;
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
        if (sampled.a < 0.5)
        {
            discard;
        }
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
    if (!flag(PBR_FLAG_METALLIC))  metallic   = 0.0;
    if (!flag(PBR_FLAG_ROUGHNESS)) roughness  = 0.5;
    roughness = clamp(roughness, 0.04, 1.0);

    // Normal: derive from tangent-space normal map if a valid slot is
    // bound, else use the vertex geometric normal.
    float3 n;
    if (mat.normal_idx != 0xFFFFFFFFu)
    {
        float3 n_ts = bindlessSrvs[NonUniformResourceIndex(mat.normal_idx)]
            .Sample(globalSamplers[NonUniformResourceIndex(SAMPLER_LINEAR_WRAP)], in.uv).xyz;
        n_ts = n_ts * 2.0 - 1.0;
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
    float  n_dot_v   = max(dot(n, v), 0.0);

    float3 radiance = frame.lightColor.rgb * frame.lightDirection.w;

    // Shadow (rasterized). 1.0 = lit, 0.0 = shadowed. Neutral = 1.0.
    float shadow = flag(PBR_FLAG_SHADOW) ? 1.0 : 1.0; // No shadow map in bindless.slang (legacy)
    // TODO: wire shadow map sampling for bindless.slang if needed.

    float  ndf = distribution_ggx(n, h, roughness);
    float  g   = geometry_smith(n, v, light_dir, roughness);
    float3 f   = fresnel_schlick(max(dot(h, v), 0.0), f0);

    float3 specular = (ndf * g * f) / (4.0 * n_dot_v * n_dot_l + 1e-4);
    float3 kD_direct = (float3(1.0) - f) * (1.0 - metallic);

    float3 direct = float3(0.0);
    if (flag(PBR_FLAG_DIRECT))
    {
        float3 diffuse = kD_direct * albedo / PI;
        float3 spec_term = flag(PBR_FLAG_SPECULAR) ? specular : float3(0.0);
        direct = (diffuse + spec_term) * radiance * n_dot_l * shadow;
    }

    // IBL (image-based). Gated by AMBIENT_IBL master flag.
    float3 f_ibl  = fresnel_schlick_roughness(max(dot(n, v), 0.0), f0, roughness);
    float3 kd_ibl = (float3(1.0) - f_ibl) * (1.0 - metallic);
    float3 diffuse_ibl  = float3(0.0);
    float3 specular_ibl = float3(0.0);
    if (flag(PBR_FLAG_AMBIENT_IBL))
    {
        if (flag(PBR_FLAG_DIFFUSE_IBL))
        {
            float3 irradiance = sample_irradiance(n);
            diffuse_ibl = kd_ibl * irradiance * albedo;
        }
        if (flag(PBR_FLAG_SPECULAR_IBL))
        {
            float3 prefiltered = sample_specular(r, roughness);
            float2 brdf_uv = float2(max(dot(n, v), 0.0), roughness);
            // BRDF LUT is in the bindless table at pc.env_handle
            float2 brdf = bindlessSrvs[NonUniformResourceIndex(pc.env_handle)]
                .SampleLevel(globalSamplers[NonUniformResourceIndex(SAMPLER_LINEAR_CLAMP)], brdf_uv, 0.0).rg;
            specular_ibl = prefiltered * (f_ibl * brdf.x + brdf.y);
        }
    }

    // ---- Emissive (gated). Neutral = 0. ----
    float3 emissive = float3(0.0);
    if (flag(PBR_FLAG_EMISSIVE))
    {
        float emissive_intensity = mat.metallic_roughness_emissive.z;
        float emissive_strength  = mat.metallic_roughness_emissive.w;
        emissive = emissive_intensity * emissive_strength;
        if (mat.emissive_idx != 0xFFFFFFFFu)
        {
            float3 e = bindlessSrvs[NonUniformResourceIndex(mat.emissive_idx)]
                .Sample(globalSamplers[NonUniformResourceIndex(SAMPLER_LINEAR_WRAP)], in.uv).rgb;
            emissive = e * emissive_intensity * emissive_strength;
        }
    }

    // ---- Advanced material terms (gated, neutral = 0). ----
    float3 transmission = float3(0.0);
    float3 translucency = float3(0.0);
    float3 aniso_spec   = float3(0.0);
    float3 clearcoat    = float3(0.0);
    if (flag(PBR_FLAG_TRANSMISSION)) transmission = eval_transmission(mat, in, v, n);
    if (flag(PBR_FLAG_TRANSLUCENCY)) translucency = eval_translucency(mat, n, v, light_dir, radiance);
    if (flag(PBR_FLAG_ANISOTROPY))   aniso_spec   = eval_anisotropy(mat, n, v, light_dir, h, roughness, f0, radiance, shadow, in.tangent);
    if (flag(PBR_FLAG_CLEARCOAT))    clearcoat    = eval_clearcoat(mat, n, v, light_dir, h, radiance, shadow);

    // ---- Compose (linear). ----
    // The albedo baseline is only shown when DIRECT is OFF (default = raw
    // baseColor). When DIRECT is ON, the surface is lit by the Cook-Torrance
    // direct lobe (which carries albedo via kD*albedo/PI), so raw albedo is
    // NOT added again -- otherwise shadowed faces (NdotL~0) stay bright and
    // direct-light shading looks flat.
    float3 color = flag(PBR_FLAG_DIRECT) ? direct : albedo;
    color += diffuse_ibl + specular_ibl;
    color += emissive;
    color += transmission + translucency + aniso_spec + clearcoat;

    // Reinhard tonemap
    color = color / (color + 1.0);
    return float4(color, 1.0);
}
