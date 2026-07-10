#version 450
layout(location = 0) in vec3 fragColor;
layout(location = 1) in vec3 worldNormal;
layout(location = 2) in vec3 worldPosition;

layout(location = 0) out vec4 outColor;

layout(binding = 0) uniform FrameUBO {
    mat4 viewProj;
    vec4 cameraPosition;
    vec4 lightDirection;   // w = intensity
    vec4 lightColor;       // w = ambient factor
} frame;

const float SHININESS = 32.0;

void main() {
    vec3 N = normalize(worldNormal);
    vec3 L = normalize(frame.lightDirection.xyz);
    vec3 V = normalize(frame.cameraPosition.xyz - worldPosition);
    vec3 H = normalize(L + V);

    float intensity = frame.lightDirection.w;
    float ambient_factor = frame.lightColor.w;

    // Ambient
    vec3 ambient = ambient_factor * fragColor;

    // Diffuse (Lambert)
    float NdotL = max(dot(N, L), 0.0);
    vec3 diffuse = NdotL * frame.lightColor.rgb * fragColor * intensity;

    // Specular (Blinn-Phong)
    float NdotH = max(dot(N, H), 0.0);
    vec3 specular = pow(NdotH, SHININESS) * frame.lightColor.rgb * intensity;

    outColor = vec4(ambient + diffuse + specular, 1.0);
}
