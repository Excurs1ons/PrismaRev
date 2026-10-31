// 3D particle billboards — camera-facing quads with a soft radial falloff.
//
// Drawn inside ForwardPass' HDR render pass, after the opaque mesh list and
// before the 2D sprite pass. Unlike `sprites.slang` this pipeline keeps the
// depth test (write off): particles are world-space, so geometry in front of
// them must occlude them. Blending is straight alpha
// (SRC_ALPHA / ONE_MINUS_SRC_ALPHA), matching the ParticlePass pipeline.
//
// Self-contained (no bindless table): the color comes from the instance, the
// alpha falloff is computed from the quad's UV, so a particle needs no texture.
//
// Push constants: ParticlePush (96 bytes) = view-projection + camera basis.
//   The camera right/up vectors come from the transpose of the view rotation
//   (GraphFrame::inv_view_rot columns), so billboards face the camera without
//   a per-instance rotation on the CPU.
//
// Per-instance vertex data (one 32-byte record per particle, instance rate):
//   loc 0 : center (xyz) + size (w, world units)
//   loc 1 : linear RGBA color
//
// Compile:
//   slangc particles.slang -profile spirv_1_4 -target spirv \
//          -entry vertexMain -stage vertex -fvk-use-entrypoint-name \
//          -o particles.vert.spv
//   slangc particles.slang -profile spirv_1_4 -target spirv \
//          -entry fragmentMain -stage fragment -fvk-use-entrypoint-name \
//          -o particles.frag.spv

struct ParticlePush
{
    float4x4 viewProj;
    float4 camRight;   // xyz = camera right in world space
    float4 camUp;      // xyz = camera up in world space
};
[[vk::push_constant]]
ConstantBuffer<ParticlePush> pc;

struct ParticleVertexIn
{
    [[vk::location(0)]] float4 centerSize;  // xyz = center, w = size
    [[vk::location(1)]] float4 color;
};

struct ParticleVaryings
{
    float4 clipPos : SV_Position;
    float2 quad    : TEXCOORD0;   // -0.5 .. 0.5
    float4 color   : COLOR0;
};

// Unit quad: two triangles, counter-clockwise.
static const float2 QUAD_POS[6] = {
    float2(-0.5, -0.5),
    float2( 0.5, -0.5),
    float2( 0.5,  0.5),
    float2(-0.5, -0.5),
    float2( 0.5,  0.5),
    float2(-0.5,  0.5),
};

[shader("vertex")]
ParticleVaryings vertexMain(ParticleVertexIn in, uint vertexId : SV_VertexID)
{
    float2 corner = QUAD_POS[vertexId];
    float size = in.centerSize.w;

    // Billboard: expand along the camera basis so the quad always faces the
    // viewer. `camRight` / `camUp` are unit vectors from the view matrix.
    float3 world = in.centerSize.xyz
        + pc.camRight.xyz * (corner.x * size)
        + pc.camUp.xyz * (corner.y * size);

    ParticleVaryings out;
    out.clipPos = mul(pc.viewProj, float4(world, 1.0));
    out.quad = corner;
    out.color = in.color;
    return out;
}

[shader("fragment")]
float4 fragmentMain(ParticleVaryings in) : SV_Target0
{
    // Soft round blob: full opacity in the middle, fading to 0 at the quad
    // edge (`|quad|` reaches 0.5 at the edge, so scale by 4 first).
    float r2 = saturate(dot(in.quad, in.quad) * 4.0);
    float falloff = saturate(1.0 - r2);
    falloff *= falloff;
    return float4(in.color.rgb, in.color.a * falloff);
}
