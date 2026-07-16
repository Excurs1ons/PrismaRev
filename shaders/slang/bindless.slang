// Bindless PBR fragment stage — the descriptor-indexing variant of pbr.slang.
//
// Instead of a dedicated set-1 cubemap binding, the IBL environment is looked
// up from a global bindless table (set 2) by a u32 handle passed through the
// push constant. This matches Rust `bindless::BindlessTextureTable` (a single
// runtime-sized COMBINED_IMAGE_SAMPLER array) + `TextureHandle`.
//
// Migration note: set 2 here must match the descriptor set index the pipeline
// layout assigns to BindlessTextureTable.layout. Set 0 (frame UBO) is
// unchanged. The old set-1 cubemap binding is dropped in this path.
//
//   slangc bindless.slang -profile spirv_1_5 -target spirv \
//          -entry fragmentMain -stage fragment -o bindless_pbr.frag.spv
import common;

// set 0, binding 0 — per-frame UBO (unchanged).
[[vk::binding(0, 0)]]
ConstantBuffer<FrameUBO> frame;

// set 2, binding 0 — bindless combined-image-sampler table. Cubemaps and 2D
// textures both live here; we declare the cube view used by IBL. A runtime
// array => index with NonUniformResourceIndex when the handle is divergent.
[[vk::binding(0, 2)]]
SamplerCube bindlessCubes[];

// Push constants: model + material + debug + the bindless env handle.
// Byte layout mirrors Rust PbrBindlessPushConstants (96 bytes):
//   model(64) albedoMetallic(16) roughness(4) debug_mode(4) normal_space(4)
//   env_handle(4)
struct PbrBindlessPush
{
    float4x4 model;
    float4   albedoMetallic;
    float    roughness;
    uint     debug_mode;
    uint     normal_space;
    uint     env_handle;     // index into bindlessCubes[]
};
[[vk::push_constant]]
PbrBindlessPush pc;

static const float PI          = 3.14159265359;
static const float IBL_MAX_LOD = 8.0;

// NonUniformResourceIndex: required by the Vulkan spec whenever the index into
// a descriptor array can differ across a quad/subgroup. Even when the handle is
// uniform here, marking it is safe and future-proofs per-material handles.
float3 sample_irradiance(float3 n)
{
    return bindlessCubes[NonUniformResourceIndex(pc.env_handle)].SampleLevel(n, 4.0).rgb;
}

float3 sample_specular(float3 r, float roughness)
{
    return bindlessCubes[NonUniformResourceIndex(pc.env_handle)]
        .SampleLevel(r, roughness * IBL_MAX_LOD).rgb;
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

float3 debug_normal(float3 n, float3 tangent)
{
    if (pc.normal_space == 1u)      { n = mul(frame.view, float4(n, 0.0)).xyz; }
    else if (pc.normal_space == 2u) { n = tangent; }
    n = normalize(n);
    return n * 0.5 + 0.5;
}

[shader("fragment")]
float4 fragmentMain(MeshVaryings in) : SV_Target
{
    float3 albedo    = pc.albedoMetallic.rgb;
    float  metallic  = pc.albedoMetallic.a;
    float  roughness = clamp(pc.roughness, 0.04, 1.0);

    float3 n = normalize(in.worldNormal);
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

    float3 irradiance  = sample_irradiance(n);
    float3 diffuse_ibl = irradiance * albedo;

    float3 prefiltered  = sample_specular(r, roughness);
    float2 brdf         = float2(1.0 - roughness, roughness);
    float3 specular_ibl = prefiltered * (f_ibl * brdf.x + brdf.y);

    float  ambient = frame.lightColor.w;
    float3 ibl     = (kd_ibl * diffuse_ibl + specular_ibl) * ambient;

    float3 color = direct + ibl;

    float3 debug_out;
    if (pc.debug_mode == 1u)      { debug_out = albedo; }
    else if (pc.debug_mode == 2u) { debug_out = specular; }
    else if (pc.debug_mode == 3u) { debug_out = prefiltered; }
    else if (pc.debug_mode == 4u) { debug_out = ibl; }
    else if (pc.debug_mode == 5u) { debug_out = debug_normal(n, in.tangent); }
    else                          { debug_out = aces(color); }

    return float4(debug_out, 1.0);
}
