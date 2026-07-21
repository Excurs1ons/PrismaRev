// Baked probe-volume GI — representation + consumer layer (Slang side).
//
// Slang module: `import gi;` from scene_frag.slang (wired in PR-3). Mirrors the
// Rust algorithm in crates/prism-render/src/gi.rs:
//   * ProbeVolumeInfo  <-> Rust `ProbeVolumeInfo` (byte-for-byte, std140)
//   * EvalSH9          <-> Rust `eval_sh9`
//   * SampleProbeVolumeIrradiance <-> Rust `sample_probe_irradiance`
// The basis ordering + constants MUST stay in lock-step with the Rust side and
// the offline baker (see the table in gi.rs module docs).
//
// PRODUCER-AGNOSTIC: this module only *reads* a regular grid of order-2 SH
// probes and reconstructs irradiance. It does not care whether the grid was
// filled by an offline baker or a real-time DDGI pass. See docs/DESIGN.md §6.
//
// Probe-volume texture layout (the representation contract):
//   A single Texture3D (RGBA16F) of dimensions (dims.x, dims.y, dims.z * 9).
//   SH coefficient c (0..8) occupies depth slices [c*dims.z, (c+1)*dims.z)
//   ("9 层", one layer per coefficient). Probe (i,j,k) coefficient c lives at
//   texel (i, j, c*dims.z + k); .rgb = coefficient RGB (.a unused; RGBA16F is
//   used because Vulkan has no universal packed RGB16F format). Sampling below
//   uses integer Load + manual trilinear so the filter never bleeds across
//   coefficient slabs.
module gi;

// Order-2 SH coefficient count (bands 0,1,2): 1 + 3 + 5 = 9.
public static const uint SH_COEFF_COUNT = 9u;

// Order-2 real SH basis constants (Ramamoorthi & Hanrahan 2001, orthonormal).
static const float SH_C0 = 0.282095; // 0.5 * sqrt(1/pi)
static const float SH_C1 = 0.488603; // 0.5 * sqrt(3/pi)
static const float SH_C2 = 1.092548; // 0.5 * sqrt(15/pi)
static const float SH_C3 = 0.315392; // 0.25 * sqrt(5/pi)
static const float SH_C4 = 0.546274; // 0.25 * sqrt(15/pi)

// Probe-volume grid metadata. Mirrors Rust `ProbeVolumeInfo` (gi.rs).
// std140: three vec4 = 48 bytes, 16-byte aligned.
public struct ProbeVolumeInfo
{
    public float4 origin;   // offset  0: xyz = world pos of probe (0,0,0), w unused
    public float4 spacing;  // offset 16: xyz = world distance between probes, w unused
    public uint4  dims;     // offset 32: xyz = probe count per axis (>=1), w unused
};

// Evaluate order-2 SH (9 RGB coefficients) for a unit direction `n`.
// `sh[c]` is the c-th coefficient (RGB). Returns reconstructed irradiance.
// Cosine-lobe convolution is assumed pre-applied by the baker (coefficients
// already encode irradiance); this does NOT multiply by albedo/pi -- the caller
// does that. Basis order matches the table in gi.rs.
public float3 EvalSH9(float3 n, float3 sh[SH_COEFF_COUNT])
{
    return sh[0] * SH_C0
         + sh[1] * (SH_C1 * n.y)
         + sh[2] * (SH_C1 * n.z)
         + sh[3] * (SH_C1 * n.x)
         + sh[4] * (SH_C2 * n.x * n.y)
         + sh[5] * (SH_C2 * n.y * n.z)
         + sh[6] * (SH_C3 * (3.0 * n.z * n.z - 1.0))
         + sh[7] * (SH_C2 * n.x * n.z)
         + sh[8] * (SH_C4 * (n.x * n.x - n.y * n.y));
}

// Map a world position to fractional probe-grid coordinates.
// coord = (world - origin) / spacing. May lie outside [0, dims-1]; the sampler
// clamps.
public float3 ProbeVolumeWorldToCoord(float3 world_pos, ProbeVolumeInfo info)
{
    return (world_pos - info.origin.xyz) / info.spacing.xyz;
}

// Sample irradiance from the probe volume at a world position for a surface
// normal. Fetches the 8 corner probes' 9 SH coefficients (integer Load, no
// cross-coefficient bleed), trilinear-blends them, then EvalSH9. Out-of-volume
// points clamp to the boundary probes; dims==1 axes degenerate to a single
// probe. Returns irradiance only -- multiply by albedo/pi at the call site.
public float3 SampleProbeVolumeIrradiance(Texture3D volume, ProbeVolumeInfo info,
                                          float3 world_pos, float3 normal)
{
    float3 coord = ProbeVolumeWorldToCoord(world_pos, info);
    int3 dims = int3(info.dims.xyz);
    // Clamp fractional coord to [0, dims-1] so out-of-volume points snap to the
    // boundary probes.
    float3 c = clamp(coord, float3(0.0), float3(dims) - float3(1.0));
    // Base corner, clamped so base+1 stays in-range (dims>=1 -> base>=0).
    int3 base = clamp(int3(floor(c)), int3(0), max(dims - int3(2), int3(0)));
    float3 t = c - float3(base);

    float3 sh[SH_COEFF_COUNT];
    for (uint coeff = 0u; coeff < SH_COEFF_COUNT; coeff++)
        sh[coeff] = float3(0.0);

    int dz = dims.z;
    for (uint idx = 0u; idx < 8u; idx++)
    {
        int di = int(idx & 1u);
        int dj = int((idx >> 1) & 1u);
        int dk = int((idx >> 2) & 1u);
        float w = (di == 0 ? 1.0 - t.x : t.x)
                * (dj == 0 ? 1.0 - t.y : t.y)
                * (dk == 0 ? 1.0 - t.z : t.z);
        if (w == 0.0)
            continue;
        int3 p = base + int3(di, dj, dk);
        for (uint coeff = 0u; coeff < SH_COEFF_COUNT; coeff++)
        {
            int z = int(coeff) * dz + p.z;
            sh[coeff] += volume.Load(int4(p.x, p.y, z, 0)).rgb * w;
        }
    }
    return EvalSH9(normal, sh);
}
