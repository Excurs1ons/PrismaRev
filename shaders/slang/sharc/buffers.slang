// SHARC buffer ABI — defines the voxel element types and buffer bindings.
//
// This file replaces Truvis's `api/pass/realtime_rt.slangi` (which carries
// the full RT pipeline's 32+ bindings). Here we only declare the 3 SHARC
// buffers + the SharcParameters struct that the algorithm files consume.
//
// Buffer layout (all zero-initialized; hash key 0 = empty slot):
// - hash_entries:   RWStructuredBuffer<uint64>  (1 × capacity)
// - accumulation:   RWStructuredBuffer<SharcAccumulationData>  (1 × capacity)
// - resolved:       RWStructuredBuffer<SharcPackedData>  (1 × capacity)

#pragma once

#ifdef __SLANG__

/// Accumulation buffer element: per-frame atomic-accumulated radiance
/// (xyz, quantized to u32 via radianceScale) + sample count (w).
struct SharcAccumulationData
{
    uint4 data;
};

/// Resolved buffer element: cross-frame accumulated radiance (fp16 rgb +
/// accumulatedSampleNum) + frame/stale metadata (2× u32).
struct SharcPackedData
{
    float16_t4 radianceData;
    uint       sampleData;
    uint       sampleDataExt;
};

#endif

/// SHARC mode constants (mirrors Rust `RenderSettings.gi_mode`).
static const uint SHARC_MODE_OFF    = 0;
static const uint SHARC_MODE_UPDATE = 1;
static const uint SHARC_MODE_ON     = 2;

/// SHARC sub-pass phase (same pipeline, different dispatch).
static const uint SHARC_PHASE_NONE    = 0;
static const uint SHARC_PHASE_UPDATE  = 1;
static const uint SHARC_PHASE_RESOLVE = 2;
