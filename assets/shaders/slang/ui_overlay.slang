// UI overlay — screen-space colored quads drawn on top of the final scene.
//   slangc ui_overlay.slang -profile spirv_1_5 -target spirv \
//          -entry vertexMain -entry fragmentMain -o ../ui_overlay.vert.spv -o ../ui_overlay.frag.spv

struct UiVertexIn
{
    float2 position : POSITION; // location 0 — NDC position
    float4 color    : COLOR0;   // location 1 — RGBA color (premultiplied)
};

struct UiVaryings
{
    float4 clipPos : SV_Position;
    float4 color   : COLOR0;    // location 0 — interpolated color
};

[shader("vertex")]
UiVaryings vertexMain(UiVertexIn in)
{
    UiVaryings out;
    out.clipPos = float4(in.position, 0.0, 1.0);
    out.color   = in.color;
    return out;
}

[shader("fragment")]
float4 fragmentMain(UiVaryings in) : SV_Target0
{
    return in.color;
}
