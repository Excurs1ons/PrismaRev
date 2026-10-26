// 2D sprite pass — instanced quads sampled through the bindless texture table.
//
// Drawn inside ForwardPass' HDR render pass (after the 3D mesh draws, before
// the debug gizmo), so sprites land in the same linear color target that
// PostPass tonemaps. `tonemap_mode = 2` (passthrough) keeps the pixel-art
// palette intact.
//
// Sets (must match `SpritePass::ensure_pipeline`):
//   set 0, binding 0 - bindless SamplerState[] (globalSamplers, shared table)
//   set 0, binding 1 - bindless Texture2D[] (bindlessSrvs, shared table)
// The bindless layout is bound at set 0 here (it is bound at set 1 in the
// scene pipeline; the layout object is index-agnostic).
//
// Push constants: SpritePush (64 bytes) = world-view-projection.
//
// Per-instance vertex data (one 112-byte record per sprite, instance rate):
//   loc 0..3 : model matrix columns (float4 each)
//   loc 4    : uv rect (u0, v0, u1, v1)
//   loc 5    : color (linear RGBA, multiplied with the sampled texel)
//   loc 6    : bindless texture index
//
// Compile:
//   slangc sprites.slang -profile spirv_1_4 -target spirv \
//          -entry vertexMain -stage vertex -fvk-use-entrypoint-name \
//          -o sprites.vert.spv
//   slangc sprites.slang -profile spirv_1_4 -target spirv \
//          -entry fragmentMain -stage fragment -fvk-use-entrypoint-name \
//          -o sprites.frag.spv

[[vk::binding(0, 0)]]
SamplerState globalSamplers[];

[[vk::binding(1, 0)]]
Texture2D bindlessSrvs[];

// Sampler slot order is fixed by `bindless::SamplerType` (see
// `RenderTextureManager`): 0 = linear wrap, 1 = linear clamp,
// 2 = nearest, 3 = shadow.
static const uint SAMPLER_NEAREST = 2u;

struct SpritePush
{
    float4x4 viewProj;
};
[[vk::push_constant]]
ConstantBuffer<SpritePush> pc;

struct SpriteVertexIn
{
    [[vk::location(0)]] float4 m0;
    [[vk::location(1)]] float4 m1;
    [[vk::location(2)]] float4 m2;
    [[vk::location(3)]] float4 m3;
    [[vk::location(4)]] float4 uvRect;
    [[vk::location(5)]] float4 color;
    [[vk::location(6)]] uint   textureIndex;
};

struct SpriteVaryings
{
    float4 clipPos : SV_Position;
    float2 uv      : TEXCOORD0;
    float4 color   : COLOR0;
    uint   textureIndex : TEXCOORD1;
};

// Unit quad: two triangles, counter-clockwise in a +Y-up world.
static const float2 QUAD_POS[6] = {
    float2(-0.5, -0.5),
    float2( 0.5, -0.5),
    float2( 0.5,  0.5),
    float2(-0.5, -0.5),
    float2( 0.5,  0.5),
    float2(-0.5,  0.5),
};

[shader("vertex")]
SpriteVaryings vertexMain(SpriteVertexIn in, uint vertexId : SV_VertexID)
{
    float2 corner = QUAD_POS[vertexId];
    float4x4 model = float4x4(in.m0, in.m1, in.m2, in.m3);
    float4 worldPos = mul(model, float4(corner, 0.0, 1.0));

    // Texture rows arrive top-to-bottom, world +Y points up: map the quad's
    // top edge to the sprite rect's v0 so images are not flipped.
    float2 t = float2(corner.x + 0.5, 0.5 - corner.y);
    float2 uv = lerp(in.uvRect.xy, in.uvRect.zw, t);

    SpriteVaryings out;
    out.clipPos = mul(pc.viewProj, worldPos);
    out.uv = uv;
    out.color = in.color;
    out.textureIndex = in.textureIndex;
    return out;
}

[shader("fragment")]
float4 fragmentMain(SpriteVaryings in) : SV_Target0
{
    // Nearest sampling keeps pixel art crisp; the rect is pre-clamped on the
    // CPU so the sprite sheet frame never bleeds into its neighbours.
    float4 texel = bindlessSrvs[NonUniformResourceIndex(in.textureIndex)]
        .SampleLevel(globalSamplers[NonUniformResourceIndex(SAMPLER_NEAREST)], in.uv, 0.0);
    float4 color = texel * in.color;
    // Straight alpha: the pipeline blends SRC_ALPHA / ONE_MINUS_SRC_ALPHA.
    return color;
}
