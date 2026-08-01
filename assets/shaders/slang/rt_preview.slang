// rt_preview.slang — 消费端示例：全屏采样 RenderTexture（bindless）。
//
// 展示 Unity 式 RT 消费：RenderTexture 在创建时注册进 bindless SRV 表，
// 拿到一个 TextureHandle；本 shader 用 push constant 接收该句柄，
// 从 bindlessSrvs[] 直接采样 —— 与采样任何普通纹理无差别。

// ---- set 1, binding 0 - bindless SAMPLER array (4 modes) ------------
SamplerState globalSamplers[];

// ---- set 1, binding 1 - bindless SAMPLED_IMAGE array -----------------
Texture2D bindlessSrvs[];

struct RtPreviewPush
{
    uint textureHandle; // RenderTexture 的 bindless 槽位
};
[[vk::push_constant]] RtPreviewPush pc;

struct VsOut
{
    float4 pos : SV_Position;
    float2 uv : TEXCOORD0;
};

[shader("vertex")]
VsOut vertexMain(uint vid : SV_VertexID)
{
    VsOut o;
    float2 uv = float2(float((vid << 1) & 2), float(vid & 2));
    o.pos = float4(uv * 2.0 - 1.0, 0.0, 1.0);
    o.uv = uv;
    return o;
}

[shader("fragment")]
float4 fragmentMain(VsOut i) : SV_Target
{
    // 线性采样 + wrap（SamplerType::LinearWrap = 槽 0），RT 内容放大到全屏。
    return bindlessSrvs[pc.textureHandle].SampleLevel(globalSamplers[0], i.uv, 0.0);
}
