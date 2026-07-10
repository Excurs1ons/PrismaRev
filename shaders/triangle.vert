#version 450

// Vertex input
layout(location = 0) in vec3 inPosition;
layout(location = 1) in vec3 inColor;

// Camera UBO (binding 0, updated per frame)
layout(binding = 0) uniform CameraUBO {
    mat4 viewProj;
} camera;

// Per-entity transform via push constants
layout(push_constant) uniform PushConstants {
    mat4 model;
} push;

// Output to fragment shader
layout(location = 0) out vec3 fragColor;

void main() {
    gl_Position = camera.viewProj * push.model * vec4(inPosition, 1.0);
    fragColor = inColor;
}
