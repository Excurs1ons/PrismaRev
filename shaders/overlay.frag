#version 450
layout(binding = 0) uniform sampler2D fontTex;
layout(location = 0) in vec2 vUV;
layout(location = 1) in vec4 vColor;
layout(location = 0) out vec4 outColor;
void main() {
    float a = texture(fontTex, vUV).a;
    outColor = vec4(vColor.rgb, vColor.a * a);
}
