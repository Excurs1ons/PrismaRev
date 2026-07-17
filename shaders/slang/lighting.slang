// Lighting pass — composites GBuffer + direct light (±RT shadow) + IBL + SHARC GI → HDR.
//
// Full-screen fragment shader. Reads the 3 GBuffer layers + shadow factor +
// GI radiance, evaluates PBR direct lighting, adds IBL ambient + cached GI,
// and writes the lit HDR color.
//
//   slangc lighting.slang -profile spirv_1_5 -target spirv \
//          -entry fragmentMain -stage fragment -o lighting.frag.spv
import common;

// set 0, binding 0 — frame UBO
[[vk::binding(0, 0)]]
ConstantBuffer<FrameUBO> frame;

// set 0, binding 1 — GBufferA (normal + roughness)
[[vk::binding(1, 0)]]
Texture2D<float4> gbufferA;

// set 0, binding 2 — GBufferB (position + depth)
[[vk::binding(2, 0)]]
Texture2D<float4> gbufferB;

// set 0, binding 3 — GBufferC (albedo + metallic)
[[vk::binding(3, 0)]]
Texture2D<float4> gbufferC;

// set 0, binding 4 — shadow factor (R8_UNORM, half-res)
[[vk::binding(4, 0)]]
Texture2D<float> shadowMap;

// set 0, binding 5 — GI radiance (R16G16B16A16_SFLOAT, half-res)
[[vk::binding(5, 0)]]
Texture2D<float4> giRadiance;

// set 0, binding 6 — IBL irradiance cubemap (combined sampler)
[[vk::binding(6, 0)]]
SamplerCube envCube;

// Push constants
struct LightingPushConstants
{
    uint  screenWidth;
    uint  screenHeight;
    uint  shadowMapWidth;   // half-res
    uint  giMapWidth;       // half-res
    float ambient;          // IBL ambient multiplier
    float _padding0;
    float _padding1;
    float _padding2;
};
[[vk::push_constant]]
LightingPushConstants pc;

static const float PI = 3.14159265359;

float3 fresnel_schlick(float cos_theta, float3 f0)
{
    return f0 + (1.0 - f0) * pow(clamp(1.0 - cos_theta, 0.0, 1.0), 5.0);
}

float3 fresnel_schlick_roughness(float cos_theta, float3 f0, float roughness)
{
    return f0 + (max(float3(1.0 - roughness), f0) - f0) *
           pow(clamp(1.0 - cos_theta, 0.0, 1.0), 5.0);
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

float geometry_smith(float3 n, float3 v, float3 l, float roughness)
{
    float r = roughness + 1.0;
    float k = (r * r) / 8.0;
    float n_dot_v = max(dot(n, v), 0.0);
    float n_dot_l = max(dot(n, l), 0.0);
    float gv = n_dot_v / (n_dot_v * (1.0 - k) + k);
    float gl = n_dot_l / (n_dot_l * (1.0 - k) + k);
    return gv * gl;
}

[shader("fragment")]
float4 fragmentMain(MeshVaryings in) : SV_Target
{
    uint2 pixel = uint2(in.clipPos.xy);

    float4 gba = gbufferA.Load(int3(pixel, 0));
    float4 gbb = gbufferB.Load(int3(pixel, 0));
    float4 gbc = gbufferC.Load(int3(pixel, 0));

    float3 albedo    = gbc.rgb;
    float  metallic  = gbc.a;
    float  roughness = clamp(gba.w, 0.04, 1.0);
    float3 n         = normalize(gba.xyz);
    float3 world_pos = gbb.xyz;
    float  depth     = gbb.w;

    // Sky
    if (depth >= 10000.0)
        return float4(0.02, 0.02, 0.04, 1.0);

    float3 v = normalize(frame.cameraPosition.xyz - world_pos);

    // Sample shadow map at half resolution (point sample via Load).
    // shadowMapWidth is the half-res width; half-res height = screenHeight/2.
    uint2 shadowCoord = uint2(
        uint(float(pixel.x) * float(pc.shadowMapWidth) / float(pc.screenWidth)),
        pixel.y / 2);
    float shadow = shadowMap.Load(int3(shadowCoord, 0)).x;

    // Sample GI at half resolution (point sample via Load).
    // giMapWidth is the half-res width; half-res height = screenHeight/2.
    uint2 giCoord = uint2(
        uint(float(pixel.x) * float(pc.giMapWidth) / float(pc.screenWidth)),
        pixel.y / 2);
    float4 gi = giRadiance.Load(int3(giCoord, 0));
    float3 gi_radiance = gi.rgb;

    // --- Direct lighting (PBR) ---
    float3 f0 = lerp(float3(0.04), albedo, metallic);
    float3 light_dir = normalize(frame.lightDirection.xyz);
    float3 h = normalize(v + light_dir);
    float3 radiance = frame.lightColor.rgb * frame.lightDirection.w;

    float ndf = distribution_ggx(n, h, roughness);
    float g = geometry_smith(n, v, light_dir, roughness);
    float3 f = fresnel_schlick(max(dot(h, v), 0.0), f0);

    float3 numerator = ndf * g * f;
    float denominator = 4.0 * max(dot(n, v), 0.0) * max(dot(n, light_dir), 0.0) + 1e-4;
    float3 specular = numerator / denominator;

    float3 kd = (float3(1.0) - f) * (1.0 - metallic);
    float n_dot_l = max(dot(n, light_dir), 0.0);
    float3 direct = (kd * albedo / PI + specular) * radiance * n_dot_l * shadow;

    // --- IBL ambient ---
    float3 f_ibl = fresnel_schlick_roughness(max(dot(n, v), 0.0), f0, roughness);
    float3 kd_ibl = (float3(1.0) - f_ibl) * (1.0 - metallic);
    float3 irradiance = envCube.SampleLevel(n, 4.0).rgb;
    float3 diffuse_ibl = irradiance * albedo;
    float3 ibl = (kd_ibl * diffuse_ibl) * pc.ambient;

    // --- SHARC GI ---
    float3 gi_term = gi_radiance;

    float3 color = direct + ibl + gi_term;

    return float4(color, 1.0);
}
