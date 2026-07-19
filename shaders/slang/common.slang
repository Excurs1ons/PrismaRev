// Shared types for the PrismaRev forward pipeline.
//
// Slang module: `import common;` from other .slang files.
//
// Layout note: these structs are laid out to match the existing GLSL std140
// (FrameUBO) / std430-ish push-constant blocks byte-for-byte, so the Rust-side
// `#[repr(C)]` mirrors (FrameUBOData / PbrPushConstants) stay valid and the
// reflection-generated bindings agree with the hand-written ones.
module common;

// Per-frame uniform data. Mirrors Rust `FrameUBOData` (set 0, binding 0).
// std140: mat4(64) + vec4(16)*3 + mat4(64) + mat4(64) = 240 bytes.
public struct FrameUBO
{
    public float4x4 viewProj;        // offset   0
    public float4   cameraPosition;  // offset  64  (xyz = camera pos, w = point light count)
    public float4   lightDirection;  // offset  80  (xyz = dir TO light, w = intensity)
    public float4   lightColor;      // offset  96  (rgb = color, w = ambient factor)
    public float4x4 view;            // offset 112  (world -> view)
    public float4x4 lightViewProj;   // offset 176  (light-space VP for shadow map)
};

// Vertex stream matching mesh.vert input locations 0..4.
public struct MeshVertexIn
{
    public float3 position  : POSITION;   // location 0
    public float3 normal    : NORMAL;     // location 1
    public float3 color     : COLOR0;     // location 2
    public float2 uv        : TEXCOORD0;  // location 3
    public float3 tangent   : TANGENT;    // location 4
};

// Interpolants shared by mesh.frag (Blinn-Phong) and pbr.frag (PBR).
// Output/input locations MUST stay in sync across the vertex + both fragment
// stages: 0 color, 1 world normal, 2 world pos, 3 uv, 4 tangent.
public struct MeshVaryings
{
    public float4 clipPos      : SV_Position;
    public float3 color        : COLOR0;     // location 0
    public float3 worldNormal  : NORMAL;     // location 1
    public float3 worldPos     : TEXCOORD1;  // location 2
    public float2 uv           : TEXCOORD0;  // location 3
    public float3 tangent      : TANGENT;    // location 4
};
