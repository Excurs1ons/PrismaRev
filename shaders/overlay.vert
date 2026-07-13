#version 450
layout(location = 0) in vec2 inPos;   // clip space, already in [-1, 1]
layout(location = 1) in vec2 inUV;    // atlas uv
layout(location = 2) in vec4 inColor; // rgba
layout(location = 0) out vec2 vUV;
layout(location = 1) out vec4 vColor;
void main() {
    vUV = inUV;
    vColor = inColor;
    gl_Position = vec4(inPos, 0.0, 1.0);
}
