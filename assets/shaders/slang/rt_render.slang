// rt_render.slang — 输出到 RenderTexture 的离屏着色器（Unity 式 RenderTexture 演示）。
//
// 渲染目标与着色器解耦：本 shader 只是往 color attachment 0 输出颜色，
// 它不知道最终目标是一张 RenderTexture 还是 swapchain —— 目标由
// RenderToTexturePass 决定（绑定 RT 的 image view 作为 framebuffer attachment）。
//
// 内容：把 16 位模式按 4x4 方格逐位展开（bitmap pattern）。
//   - UV ∈ [0,1] 分成 4x4 个方格，cell = int(uv * 4)
//   - bit index = row * 4 + col（bit0 = 左上角）
//   - bit = 1 → 白，bit = 0 → 黑
// 16 位数字由 CPU 端每帧随机生成（RenderToTexturePass 内 xorshift），
// 经 push constant 传入。

struct RtRenderPush
{
    uint pattern; // 低 16 位有效：4x4 位图（bit i = 行 i/4、列 i%4 的方格）
};
[[vk::push_constant]] RtRenderPush pc;

struct VsOut
{
    float4 pos : SV_Position;
    float2 uv : TEXCOORD0;
};

// 全屏三角形（3 顶点，无顶点缓冲），UV 从 (0,0) 到 (2,2) 覆盖视口。
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
    int2 cell = int2(i.uv * 4.0);
    int idx = cell.y * 4 + cell.x;
    uint bit = (pc.pattern >> uint(idx)) & 1u;
    return float4(float(bit), float(bit), float(bit), 1.0);
}
