#version 450
// PBR fragment shader (Cook-Torrance) with image-based lighting sampled from a
// cubemap environment (set=1) converted from the user's equirectangular HDR at
// load time. Sampling by direction avoids the equirect pole/seam flicker.
//
// Input locations match mesh.vert: 0 = color, 1 = world normal, 2 = world pos,
// 3 = uv, 4 = tangent.

precision highp float;

layout(location = 0) in vec3 inColor;
layout(location = 1) in vec3 inNormal;
layout(location = 2) in vec3 inWorldPos;
layout(location = 3) in vec2 inUV;
layout(location = 4) in vec3 inTangent;

layout(location = 0) out vec4 outColor;

// Set 0: per-frame UBO (shared with the Blinn-Phong path).
layout(set = 0, binding = 0) uniform FrameUBO {
    mat4 viewProj;
    vec4 cameraPosition;   // xyz = camera pos
    vec4 lightDirection;   // xyz = dir TO light, w = intensity
    vec4 lightColor;       // rgb = color, w = ambient factor
    mat4 view;             // world -> view (for view-space debug normals)
};

// Set 1: image-based lighting environment (cubemap).
layout(set = 1, binding = 0) uniform samplerCube envCube;

// Push constants: model matrix + material params + debug selectors.
layout(push_constant) uniform PushConstants {
    mat4 model;
    vec4 albedoMetallic; // rgb = albedo, a = metallic
    float roughness;
    uint debug_mode;     // 0 Final,1 Albedo,2 Specular,3 Reflection,4 Ambient,5 Normal
    uint normal_space;   // 0 World, 1 View, 2 Tangent
} pc;

const float PI = 3.14159265359;
const float IBL_MAX_LOD = 8.0;

// Diffuse irradiance: a blurred sample of the environment.
vec3 sample_irradiance(vec3 n) {
    return textureLod(envCube, n, 4.0).rgb;
}

// Prefiltered (roughness-weighted) reflection sample.
vec3 sample_specular(vec3 r, float roughness) {
    return textureLod(envCube, r, roughness * IBL_MAX_LOD).rgb;
}

vec3 fresnel_schlick(float cos_theta, vec3 f0) {
    return f0 + (1.0 - f0) * pow(clamp(1.0 - cos_theta, 0.0, 1.0), 5.0);
}

vec3 fresnel_schlick_roughness(float cos_theta, vec3 f0, float roughness) {
    return f0 + (max(vec3(1.0 - roughness), f0) - f0) * pow(clamp(1.0 - cos_theta, 0.0, 1.0), 5.0);
}

float distribution_ggx(vec3 n, vec3 h, float roughness) {
    float a = roughness * roughness;
    float a2 = a * a;
    float n_dot_h = max(dot(n, h), 0.0);
    float n_dot_h2 = n_dot_h * n_dot_h;
    float denom = n_dot_h2 * (a2 - 1.0) + 1.0;
    return a2 / max(PI * denom * denom, 1e-5);
}

float geometry_schlick_ggx(float n_dot_v, float roughness) {
    float r = roughness + 1.0;
    float k = (r * r) / 8.0;
    return n_dot_v / (n_dot_v * (1.0 - k) + k);
}

float geometry_smith(vec3 n, vec3 v, vec3 l, float roughness) {
    float n_dot_v = max(dot(n, v), 0.0);
    float n_dot_l = max(dot(n, l), 0.0);
    return geometry_schlick_ggx(n_dot_v, roughness) * geometry_schlick_ggx(n_dot_l, roughness);
}

vec3 aces(vec3 x) {
    const float a = 2.51, b = 0.03, c = 2.43, d = 0.59, e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), 0.0, 1.0);
}

// Map a normal to a viewable RGB (0..1) for the Normal debug mode.
vec3 debug_normal(vec3 n) {
    if (pc.normal_space == 1u) {
        n = (view * vec4(n, 0.0)).xyz;
    } else if (pc.normal_space == 2u) {
        n = inTangent;
    }
    n = normalize(n);
    return n * 0.5 + 0.5;
}

void main() {
    vec3 albedo = pc.albedoMetallic.rgb;
    float metallic = pc.albedoMetallic.a;
    float roughness = clamp(pc.roughness, 0.04, 1.0);

    vec3 n = normalize(inNormal);
    vec3 v = normalize(cameraPosition.xyz - inWorldPos);
    vec3 r = reflect(-v, n);

    vec3 f0 = mix(vec3(0.04), albedo, metallic);

    // --- Direct lighting (one directional light, world space) ---
    vec3 light_dir = normalize(lightDirection.xyz);
    vec3 h = normalize(v + light_dir);
    float n_dot_l = max(dot(n, light_dir), 0.0);

    vec3 radiance = lightColor.rgb * lightDirection.w;
    float ndf = distribution_ggx(n, h, roughness);
    float g = geometry_smith(n, v, light_dir, roughness);
    vec3 f = fresnel_schlick(max(dot(h, v), 0.0), f0);

    vec3 numerator = ndf * g * f;
    float denominator = 4.0 * max(dot(n, v), 0.0) * n_dot_l + 1e-4;
    vec3 specular = numerator / denominator;

    vec3 kd = (vec3(1.0) - f) * (1.0 - metallic);
    vec3 direct = (kd * albedo / PI + specular) * radiance * n_dot_l;

    // --- Image-based lighting ---
    vec3 f_ibl = fresnel_schlick_roughness(max(dot(n, v), 0.0), f0, roughness);
    vec3 kd_ibl = (vec3(1.0) - f_ibl) * (1.0 - metallic);

    vec3 irradiance = sample_irradiance(n);
    vec3 diffuse_ibl = irradiance * albedo;

    vec3 prefiltered = sample_specular(r, roughness);
    // Cheap split-sum approximation of the BRDF scale/offset term.
    vec2 brdf = vec2(1.0 - roughness, roughness); // scale ~ (1-rough), bias ~ rough
    vec3 specular_ibl = prefiltered * (f_ibl * brdf.x + brdf.y);

    float ambient = lightColor.w;
    vec3 ibl = (kd_ibl * diffuse_ibl + specular_ibl) * ambient;

    vec3 color = direct + ibl;

    // --- Debug visualization override ---
    vec3 debug_out;
    if (pc.debug_mode == 1u) {
        debug_out = albedo;
    } else if (pc.debug_mode == 2u) {
        debug_out = specular;
    } else if (pc.debug_mode == 3u) {
        debug_out = prefiltered;
    } else if (pc.debug_mode == 4u) {
        debug_out = ibl;
    } else if (pc.debug_mode == 5u) {
        debug_out = debug_normal(n);
    } else {
        debug_out = aces(color);
    }

    outColor = vec4(debug_out, 1.0);
}
