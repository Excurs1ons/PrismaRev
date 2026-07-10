#version 450
layout(location = 0) in vec3 inPosition;
layout(location = 1) in vec3 inNormal;
layout(location = 2) in vec3 inColor;

layout(binding = 0) uniform FrameUBO {
    mat4 viewProj;
    vec4 cameraPosition;
    vec4 lightDirection;  // w = intensity
    vec4 lightColor;      // w = ambient factor
} frame;

layout(push_constant) uniform PushConstants {
    mat4 model;
} push;

layout(location = 0) out vec3 fragColor;
layout(location = 1) out vec3 worldNormal;
layout(location = 2) out vec3 worldPosition;

void main() {
    vec4 worldPos = push.model * vec4(inPosition, 1.0);
    worldPosition = worldPos.xyz;
    // Normal transform: use mat3(push.model) — assumes uniform scale
    worldNormal = normalize(mat3(push.model) * inNormal);
    fragColor = inColor;
    gl_Position = frame.viewProj * worldPos;
}
