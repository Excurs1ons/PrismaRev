// Baked probe-volume GI — representation + consumer layer (Slang side).
//
// Slang module: `import gi;` from scene_frag.slang (wired in PR-3). Mirrors the
// Rust algorithm in crates/prism-render/src/gi.rs:
//   * ProbeVolumeInfo  <-> Rust `ProbeVolumeInfo` (byte-for-byte, std140)
//   * EvalSH9Radiance, EvalSH9Irradiance <-> Rust `eval_sh9`, `eval_sh9_irradiance`
//   * SampleProbeVolume* <-> Rust `sample_probe_irradiance`
// The basis ordering + constants MUST stay in lock-step with the Rust side and
// the offline baker (see the table in gi.rs module docs).
//
// REPRESENTATION CHANGE (PR-5): the baker now stores RADIANCE SH (no cosine
// pre-convolution). The probe volume encodes the incident radiance function
// L(omega). Two evaluation paths derive different quantities from the same SH:
//   * Diffuse  → EvalSH9Irradiance(n) applies the Ramamoorthi & Hanrahan
//                cosine-convolution factors A_l = [PI, 2*PI/3, PI/4]
//                returning irradiance E(n) = ∫ L(ω) max(0, n·ω) dω.
//   * Specular → EvalSH9Radiance(r) returns the radiance L(r) reconstructed
//                from SH, which feeds the split-sum specular approximation.
// This eliminates the old PI-scale mismatch between IBL and GI paths and
// makes the specular GI path physically meaningful.
//
// PRODUCER-AGNOSTIC: this module only *reads* a regular grid of order-2 SH
// probes and reconstructs irradiance/radiance. It does not care whether the
// grid was filled by an offline baker or a real-time DDGI pass.
// See docs/DESIGN.md §6.
//
// Probe-volume texture layout (the representation contract):
//   A single Texture3D (RGBA16F) of dimensions (dims.x, dims.y, dims.z * 9).
//   SH coefficient c (0..8) occupies depth slices [c*dims.z, (c+1)*dims.z)
//   ("9 layers", one layer per coefficient). Probe (i,j,k) coefficient c lives at
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

// Ramamoorthi & Hanrahan SH cosine-convolution factors (orthonormal basis).
// Each band l is scaled by A_l where:
//   A_0 = pi
//   A_1 = 2*pi/3
//   A_2 = pi/4
// These convert radiance SH coefficients into irradiance via:
//   E(n) = sum_{l,m} c_lm * A_l * Y_lm(n)
static const float A_L0 = 3.14159265359;       // pi
static const float A_L1 = 2.09439510239;       // 2*pi/3
static const float A_L2 = 0.78539816339;       // pi/4

// Probe-volume grid metadata. Mirrors Rust `ProbeVolumeInfo` (gi.rs).
// std140: three vec4 = 48 bytes, 16-byte aligned.
public struct ProbeVolumeInfo
{
    public float4 origin;   // offset  0: xyz = world pos of probe (0,0,0), w unused
    public float4 spacing;  // offset 16: xyz = world distance between probes, w unused
    public uint4  dims;     // offset 32: xyz = probe count per axis (>=1), w unused
};

// ---- SH evaluation helpers ------------------------------------------------

/// Evaluate order-2 RADIANCE SH: returns L(ω) ≈ Σ c_lm * Y_lm(ω).
/// The SH coefficients are assumed to encode the incident radiance function
/// (no cosine pre-convolution). This is the correct input for the split-sum
/// specular approximation.
public float3 EvalSH9Radiance(float3 n, float3 sh[SH_COEFF_COUNT])
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

/// Evaluate order-2 IRRADIANCE SH: returns E(n) = ∫ L(ω) max(0, n·ω) dω.
/// Applies the Ramamoorthi & Hanrahan cosine-convolution factors A_l to the
/// radiance SH coefficients. The caller divides by PI for the Lambertian
/// BRDF: diffuse = kd * (E(n) / PI) * albedo.
public float3 EvalSH9Irradiance(float3 n, float3 sh[SH_COEFF_COUNT])
{
    // Band 0 (l=0): A_0 = PI — one coefficient
    // Band 1 (l=1): A_1 = 2*PI/3 — three coefficients (y, z, x)
    // Band 2 (l=2): A_2 = PI/4 — five coefficients
    return sh[0] * SH_C0 * A_L0
         + sh[1] * (SH_C1 * n.y) * A_L1
         + sh[2] * (SH_C1 * n.z) * A_L1
         + sh[3] * (SH_C1 * n.x) * A_L1
         + sh[4] * (SH_C2 * n.x * n.y)   * A_L2
         + sh[5] * (SH_C2 * n.y * n.z)   * A_L2
         + sh[6] * (SH_C3 * (3.0 * n.z * n.z - 1.0)) * A_L2
         + sh[7] * (SH_C2 * n.x * n.z)   * A_L2
         + sh[8] * (SH_C4 * (n.x * n.x - n.y * n.y)) * A_L2;
}

// ---- Coordinate helpers ---------------------------------------------------

// Map a world position to fractional probe-grid coordinates.
// coord = (world - origin) / spacing. May lie outside [0, dims-1]; the sampler
// clamps.
public float3 ProbeVolumeWorldToCoord(float3 world_pos, ProbeVolumeInfo info)
{
    return (world_pos - info.origin.xyz) / info.spacing.xyz;
}

// ---- Trilinear SH sampling (internal) -------------------------------------

// Trilinearly blend the 8 corner probes' 9 SH coefficients, then evaluate
// `eval` (one of EvalSH9Radiance or EvalSH9Irradiance) with the given `dir`.
// The `mode` selector chooses the evaluation function:
//   0 = EvalSH9Radiance  (specular / debug)
//   1 = EvalSH9Irradiance (diffuse)
float3 SampleProbeVolumeSH(Texture3D volume, ProbeVolumeInfo info,
                           float3 world_pos, float3 dir, uint mode)
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

    // Select the SH evaluation function based on mode.
    if (mode == 0u)
        return EvalSH9Radiance(dir, sh);
    else
        return EvalSH9Irradiance(dir, sh);
}

// ---- Public sampling API --------------------------------------------------

/// Sample IRRADIANCE from the probe volume. Returns E(n) = ∫ L(ω) max(0, n·ω) dω.
/// The caller multiplies by kd * albedo / PI for the Lambertian diffuse BRDF.
public float3 SampleProbeVolumeIrradiance(Texture3D volume, ProbeVolumeInfo info,
                                          float3 world_pos, float3 normal)
{
    return SampleProbeVolumeSH(volume, info, world_pos, normal, 1u);
}

/// Sample RADIANCE from the probe volume. Returns L(ω) ≈ Σ c_lm * Y_lm(ω),
/// the SH-reconstructed radiance in the given direction. This is the correct
/// input for the split-sum specular approximation (replaces the prefiltered
/// environment map).
public float3 SampleProbeVolumeRadiance(Texture3D volume, ProbeVolumeInfo info,
                                        float3 world_pos, float3 dir)
{
    return SampleProbeVolumeSH(volume, info, world_pos, dir, 0u);
}
