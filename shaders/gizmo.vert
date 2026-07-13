#version 450
layout(location = 0) in vec3 inPosition;
layout(location = 1) in vec3 inColor;

layout(push_constant) uniform PushConstants {
    mat4 viewProj;
} push;

layout(location = 0) out vec3 fragColor;

void main() {
    fragColor = inColor;
    gl_Position = push.viewProj * vec4(inPosition, 1.0);
}
