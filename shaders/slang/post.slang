// Post-processing pass — ACES tone mapping + gamma → swapchain output.
//
// Full-screen fragment shader. Reads the HDR color from the LightingPass,
// applies ACES filmic tone mapping, converts to SDR, and outputs to the
// swapchain (B8G8R8A8_UNORM).
//
//   slangc post.slang -profile spirv_1_5 -target spirv \
//          -entry fragmentMain -stage fragment -o post.frag.spv
import common;

// set 0, binding 0 — HDR input (R16G16B16A16_SFLOAT)
[[vk::binding(0, 0)]]
Texture2D<float4> hdrInput;

// set 0, binding 1 — sampler for HDR input
[[vk::binding(1, 0)]]
SamplerState hdrSampler;

struct PostPushConstants
{
    uint  screenWidth;
    uint  screenHeight;
    float exposure;
    float _padding;
};
[[vk::push_constant]]
PostPushConstants pc;

float3 aces(float3 x)
{
    const float a = 2.51;
    const float b = 0.03;
    const float c = 2.43;
    const float d = 0.59;
    const float e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), 0.0, 1.0);
}

[shader("fragment")]
float4 fragmentMain(MeshVaryings in) : SV_Target
{
    float2 uv = in.uv;
    float3 hdr = hdrInput.Sample(hdrSampler, uv).rgb * pc.exposure;
    float3 sdr = aces(hdr);
    // Gamma correction
    sdr = pow(sdr, float3(1.0 / 2.2));
    return float4(sdr, 1.0);
}
