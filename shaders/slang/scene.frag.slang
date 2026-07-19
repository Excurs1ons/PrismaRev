// Forward PBR scene pass for the RenderGraph path (graph_renderer.rs).
//
// Renders the scene with Cook-Torrance PBR + IBL, and applies the rasterized
// shadow map produced by `ShadowMapPass`: the world position is projected into
// light space and compared (with a comparison sampler) against the shadow
// depth to decide lit vs shadowed. This is the graph-path counterpart of the
// legacy `pbr.frag`; kept separate so the legacy renderer is untouched.
//
// Bindings:
//   set 0, binding 0 — per-frame UBO (FrameUBO)
//   set 1, binding 0 — IBL environment cubemap (combined image sampler)
//   set 2, binding 0 — shadow map depth texture
//   set 2, binding 1 — shadow comparison sampler
// Push constants: model (mat4) + lightViewProj (mat4) + debug (vec4).
//
// Compile:
//   slangc scene.frag.slang -profile spirv_1_5 -target spirv \
//          -entry fragmentMain -stage fragment -o scene.frag.spv
import common;

// set 0, binding 0 — per-frame UBO (shared with the Blinn-Phong path).
[[vk::binding(0, 0)]]
ConstantBuffer<FrameUBO> frame;

// set 1, binding 0 — IBL environment cubemap.
[[vk::binding(0, 1)]]
SamplerCube envCube;

// set 2 — rasterized shadow map (depth) + comparison sampler.
[[vk::binding(0, 2)]]
Texture2D<float> shadowMap;
[[vk::binding(1, 2)]]
SamplerComparisonState shadowSampler;

// Push constants: model matrix + light-space view-proj + debug selectors.
struct ScenePush
{
    float4x4 model;
    float4x4 lightViewProj;
    float4   debug; // x = debug_mode, y = normal_space
};
[[vk::push_constant]]
ScenePush pc;

static const float PI          = 3.14159265359;
static const float IBL_MAX_LOD = 8.0;

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
    float a       = roughness * roughness;
    float a2      = a * a;
    float n_dot_h = max(dot(n, h), 0.0);
    float n_dot_h2 = n_dot_h * n_dot_h;
    float denom   = n_dot_h2 * (a2 - 1.0) + 1.0;
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

/// Project a world position into the shadow map and return 1.0 (lit) or 0.0
/// (shadowed) using a comparison sampler. Returns 1.0 outside the light's
/// frustum (treated as lit).
float sample_shadow(float3 world_pos)
{
    float4 light_space = mul(pc.lightViewProj, float4(world_pos, 1.0));
    if (light_space.w <= 0.0)
        return 1.0;
    float3 proj = light_space.xyz / light_space.w;
    // NDC [-1,1] -> UV [0,1]; flip Y for Vulkan's top-left origin.
    float2 uv = float2(proj.x * 0.5 + 0.5, 1.0 - (proj.y * 0.5 + 0.5));
    float depth = proj.z; // orthographic light projection -> already [0,1]
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || depth > 1.0)
        return 1.0;
    // SampleCmpLevelZero returns 1.0 when `depth` passes the comparison
    // (fragment is closer than the stored shadow depth -> lit).
    return shadowMap.SampleCmpLevelZero(shadowSampler, uv, depth);
}

[shader("fragment")]
float4 fragmentMain(MeshVaryings in) : SV_Target
{
    float3 albedo   = in.color;
    float  roughness = clamp(0.5, 0.04, 1.0);

    float3 n = normalize(in.worldNormal);
    float3 v = normalize(frame.cameraPosition.xyz - in.worldPos);
    float3 r = reflect(-v, n);

    float3 f0 = float3(0.04);

    // --- Direct lighting (one directional light, world space) ---
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

    float3 kd     = (float3(1.0) - f);
    float3 direct = (kd * albedo / PI + specular) * radiance * n_dot_l;

    // --- Image-based lighting ---
    float3 f_ibl  = fresnel_schlick_roughness(max(dot(n, v), 0.0), f0, roughness);
    float3 kd_ibl = (float3(1.0) - f_ibl);

    float3 irradiance  = sample_irradiance(n);
    float3 diffuse_ibl = irradiance * albedo;

    float3 prefiltered = sample_specular(r, roughness);
    float2 brdf         = float2(1.0 - roughness, roughness);
    float3 specular_ibl = prefiltered * (f_ibl * brdf.x + brdf.y);

    float  ambient = frame.lightColor.w;
    float3 ibl     = (kd_ibl * diffuse_ibl + specular_ibl) * ambient;

    // --- Shadow (rasterized shadow map) ---
    float shadow = sample_shadow(in.worldPos);

    float3 color = (direct + ibl) * shadow;

    // Debug visualization override (reuse legacy debug modes).
    uint debug_mode = uint(pc.debug.x);
    if (debug_mode == 1u)      { color = albedo; }
    else if (debug_mode == 2u) { color = specular; }
    else if (debug_mode == 3u) { color = prefiltered; }
    else if (debug_mode == 4u) { color = ibl; }
    else if (debug_mode == 5u) { color = n * 0.5 + 0.5; }
    else                       { color = aces(color); }

    return float4(color, 1.0);
}
