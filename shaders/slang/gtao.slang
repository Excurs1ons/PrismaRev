// GTAO (Ground-Truth Ambient Occlusion) pass - half-resolution screen-space AO.
//
// Runs as a fullscreen-triangle fragment pass AFTER the ScenePass (which writes
// the D32_SFLOAT depth + the R16G16B16A16 view-space normal MRT target). The
// pass reads depth (+ optional normal) and writes a single-channel R8_UNORM AO
// texture. The scene shader samples *last frame's* AO output (1-frame latency)
// to attenuate the IBL diffuse + specular terms - this avoids the
// chicken-and-egg problem of AO needing scene output while IBL is computed
// inside the scene pass.
//
// Sets (must match `GtaoPass::ensure_pipeline`):
//   set 0, binding 0 - scene depth (Texture2D, D32_SFLOAT)
//   set 0, binding 1 - linear sampler for depth
//   set 1, binding 0 - scene view-space normal (Texture2D, R16G16B16A16_SFLOAT)
//                      (only sampled when `pc.mode == 0`)
//   set 1, binding 1 - linear sampler for normal
//
// Push constants: GtaoPush (96 bytes):
//   inv_proj    - column-major clip->view (for position reconstruction)
//   viewport    - AO texture resolution (half-res)
//   radius      - view-space occlusion radius (world units)
//   mode        - 0 = sample normal MRT, 1 = reconstruct from depth derivatives
//   _pad0       - explicit alignment padding
//
// Coordinate conventions (see README §Coordinate Conventions and AGENTS.md):
//   - Right-handed; camera looks down -Z; NDC y: -1 = top, +1 = bottom.
//   - Vulkan y-flip is baked into proj[1][1] = -inv_tan(fovy/2).
//   - Depth range [0,1]; clip = proj * view * model.
//
// Compile:
//   slangc gtao.slang -profile spirv_1_5 -target spirv \
//          -entry vertexMain -stage vertex   -fvk-use-entrypoint-name -o gtao.vert.spv
//   slangc gtao.slang -profile spirv_1_5 -target spirv \
//          -entry fragmentMain -stage fragment -fvk-use-entrypoint-name -o gtao.frag.spv

// ---- set 0: scene depth + sampler -----------------------------------
[[vk::binding(0, 0)]]
Texture2D<float> depthTex;
[[vk::binding(1, 0)]]
SamplerState depthSampler;

// ---- set 1: view-space normal MRT + sampler -------------------------
// R16G16B16A16_SFLOAT; xyz = view-space normal (unit), w unused.
// Only bound/sampled when pc.mode == 0; set 1 is always declared in the
// pipeline layout so the descriptor set is valid either way.
[[vk::binding(0, 1)]]
Texture2D<float4> normalTex;
[[vk::binding(1, 1)]]
SamplerState normalSampler;

// ---- push constants -------------------------------------------------
// 64 (inv_proj) + 8 (viewport) + 4 (radius) + 4 (mode) + 4 (pad) = 84 bytes.
// Matches the Rust `GtaoPushConstants` (`#[repr(C)]`, tight packing) exactly.
// Vulkan push-constant ranges don't require 16-byte multiples; the range
// `size` just has to match the bytes pushed (84 here).
public struct GtaoPush
{
    public float4x4 inv_proj;   // clip -> view (for position reconstruction)
    public float2   viewport;   // AO texture size (half-res), px
    public float    radius;     // view-space sample radius (world units)
    public uint     mode;       // 0 = normal MRT, 1 = depth-reconstruct
    public uint     _pad0;
};
[[vk::push_constant]]
GtaoPush pc;

static const float PI = 3.14159265359;

// Number of search directions (slices). 4 is the standard GTAO default and
// balances quality vs cost.
static const uint SLICE_COUNT = 4u;
// Samples per direction (per side). 4 gives acceptable quality at half-res.
static const uint SAMPLES_PER_SIDE = 4u;

// Fullscreen triangle: 3 verts covering NDC [-1,1]x[-1,1]. Same pattern as
// post.slang (no vertex buffer, driven by SV_VertexID).
[shader("vertex")]
public void vertexMain(
    uint vid : SV_VertexID,
    out float4 clipPos : SV_Position,
    out float2 uv : TEXCOORD0
)
{
    float2 ndc = float2(
        (vid == 0) ? -1.0 : (vid == 1) ? 3.0 : -1.0,
        (vid == 0) ? -1.0 : (vid == 1) ? -1.0 : 3.0
    );
    clipPos = float4(ndc, 0.0, 1.0);
    // Vulkan NDC y = -1 is the top; UV.y = 0 at the top. Match the framebuffer
    // top-left origin so depth/normal texels line up with rasterized pixels.
    uv = ndc * 0.5 + 0.5;
}

/// Reconstruct view-space position from a screen UV + depth buffer value.
/// `clip` = (uv*2-1, depth, 1); view = inv_proj * clip; perspective divide.
/// Engine convention: camera looks down -Z, so view-space z is negative.
float3 reconstruct_view_pos(float2 uv, float depth)
{
    float4 clip = float4(float2(uv.x * 2.0 - 1.0, uv.y * 2.0 - 1.0), depth, 1.0);
    float4 view = mul(pc.inv_proj, clip);
    return view.xyz / view.w;
}

/// Sample the scene depth at a given screen UV. Returns the raw [0,1] depth.
float sample_depth(float2 uv)
{
    return depthTex.SampleLevel(depthSampler, uv, 0.0).r;
}

/// Fetch the view-space normal for the center pixel.
///   mode 0: sample the normal MRT (R16G16B16A16_SFLOAT, xyz already unit).
///   mode 1: reconstruct from depth derivatives (mobile-friendly, no MRT read).
float3 load_normal(float2 uv, float2 texel, float3 center, float depth)
{
    if (pc.mode == 0u)
    {
        float3 n = normalTex.SampleLevel(normalSampler, uv, 0.0).xyz;
        // Renormalize - the MRT stores a normalized value but filtering at
        // half-res may shrink the length slightly.
        return normalize(n);
    }
    // Depth-derivative reconstruction: build two tangent view-space edge
    // vectors from the center + neighbour depths, then cross them.
    float zx = sample_depth(uv + float2(texel.x, 0.0));
    float zy = sample_depth(uv + float2(0.0, texel.y));
    float3 px = reconstruct_view_pos(uv + float2(texel.x, 0.0), zx);
    float3 py = reconstruct_view_pos(uv + float2(0.0, texel.y), zy);
    // Cross product order picks the normal facing the camera (+Z side in view
    // space, since the camera looks down -Z and surface normals face the viewer).
    float3 n = normalize(cross(px - center, py - center));
    if (n.z < 0.0)
        n = -n;
    return n;
}

/// Search the horizon along one direction of a slice. Steps in screen UV space
/// along `dir_uv`, sampling depth and reconstructing view-space positions.
/// Returns the horizon angle's sine (clamped to the tangent plane) - the larger
/// the sine, the higher the occluder above the surface = more occlusion.
///
/// `center` = center pixel's view-space position.
/// `normal` = surface normal at the center pixel.
/// `axis_uv` = screen-space direction (unit) along which to search.
/// `sign` = +1 or -1 (search both sides of the slice).
float search_horizon(float3 center, float3 normal, float2 uv, float2 axis_uv, float sign)
{
    // The horizon is the largest angle gamma between the view ray to a sample
    // and the horizon plane (the plane through `center` parallel to the screen).
    // We track cos(gamma) where gamma is measured from the +Z view direction:
    // a sample higher than the center (closer to camera, larger z) raises the
    // horizon and increases occlusion. cos(gamma) = (sample.z - center.z) / dist.
    float max_sine = 0.0;
    float2 texel_step = axis_uv * sign / pc.viewport;
    for (uint i = 1u; i <= SAMPLES_PER_SIDE; i++)
    {
        // Screen-space step grows with the iteration (perspective-correct-ish):
        // we step farther in screen space for farther samples so view-space
        // spacing is roughly uniform.
        float t = float(i);
        float2 sample_uv = uv + texel_step * t * (1.0 + t * 0.5);
        sample_uv = clamp(sample_uv, float2(0.0), float2(1.0));
        float sd = sample_depth(sample_uv);
        float3 sp = reconstruct_view_pos(sample_uv, sd);
        float3 v = sp - center;
        // Distance in the xy plane of view space (the horizon plane).
        float len_xy = length(v.xy);
        if (len_xy < 1e-4)
            continue;
        // sin of the angle above the horizon plane. Positive = sample is above
        // the center (occluder); negative = below (no occlusion from this one).
        float sin_h = v.z / len_xy;
        // Note: view-space z is negative (camera looks down -Z), and a sample
        // closer to the camera has a LESS negative z (larger). So an occluder
        // above the surface has v.z > 0 (sample.z less negative than center.z).
        max_sine = max(max_sine, sin_h);
    }
    return max_sine;
}

/// Integrate the GTAO visibility for one slice given the two horizon sines
/// (left and right). Returns 1.0 = unoccluded, 0.0 = fully occluded.
/// Uses the cosine-weighted hemisphere approximation from the GTAO paper:
/// the slice's contribution is the integral of cos(theta) between the two
/// horizons, weighted by the normal's projection onto the slice direction.
float integrate_slice(float sin_h1, float sin_h2, float3 normal, float3 slice_dir)
{
    // Clamp horizons to the upper hemisphere (above the tangent plane).
    float h1 = max(sin_h1, 0.0);
    float h2 = max(sin_h2, 0.0);
    // Convert sine horizons to cosine (the integrand is cos(theta)).
    float cos_h1 = sqrt(saturate(1.0 - h1 * h1));
    float cos_h2 = sqrt(saturate(1.0 - h2 * h2));
    // Approximate cosine-weighted integral: (sin(h) - cos(h)*n_dot_s) range.
    // Use the closed form 1 - cos(h) for the unprojected hemisphere, then
    // weight by the normal's slice projection.
    float n_dot_s = dot(normal, slice_dir);
    // Visibility = 1 - occluded_fraction. Occluded fraction ~ (1 - cos(h_avg))
    // scaled by the normal's alignment with the slice direction.
    float h_avg = (h1 + h2) * 0.5;
    float cos_avg = (cos_h1 + cos_h2) * 0.5;
    float occlusion = (1.0 - cos_avg) * abs(n_dot_s);
    return saturate(1.0 - occlusion);
}

[shader("fragment")]
public void fragmentMain(
    float4 clipPos : SV_Position,
    float2 uv : TEXCOORD0,
    out float4 outColor : SV_Target0
)
{
    float2 texel = 1.0 / pc.viewport;

    float depth = sample_depth(uv);
    // Sky pixels (depth == 1.0, the cleared value) have no occlusion - emit 1.0
    // so the scene's IBL term is unmodified there.
    if (depth >= 1.0)
    {
        outColor = float4(1.0, 1.0, 1.0, 1.0);
        return;
    }

    float3 center = reconstruct_view_pos(uv, depth);
    float3 normal = load_normal(uv, texel, center, depth);

    // Build a tangent basis (t, b) perpendicular to `normal` so we can rotate
    // slice directions in the tangent plane.
    float3 up = abs(normal.z) < 0.999 ? float3(0.0, 0.0, 1.0) : float3(1.0, 0.0, 0.0);
    float3 t = normalize(cross(up, normal));
    float3 b = cross(normal, t);

    float visibility = 0.0;
    for (uint s = 0u; s < SLICE_COUNT; s++)
    {
        // Slice direction angle in the tangent plane, evenly spaced in [0, pi).
        float angle = (float(s) + 0.5) * PI / float(SLICE_COUNT);
        float3 slice_dir = cos(angle) * t + sin(angle) * b;

        // Project the view-space slice direction into screen UV space. A cheap
        // approximation: drop the z component and normalize (works because
        // perspective foreshortening mostly affects z, not the screen-space
        // direction).
        float2 axis_uv = normalize(slice_dir.xy);

        float sin_h1 = search_horizon(center, normal, uv, axis_uv, -1.0);
        float sin_h2 = search_horizon(center, normal, uv, axis_uv, 1.0);

        visibility += integrate_slice(sin_h1, sin_h2, normal, slice_dir);
    }
    visibility /= float(SLICE_COUNT);

    outColor = float4(visibility, visibility, visibility, 1.0);
}
