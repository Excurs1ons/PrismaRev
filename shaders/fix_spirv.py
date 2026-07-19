#!/usr/bin/env python3
"""Strip illegal ArrayStride decorations from Slang-emitted SPIR-V.

Slang (observed on 2025.x) emits an `OpDecorate <runtime-array> ArrayStride 8`
decoration for `UniformConstant` runtime arrays of opaque types
(sampled images / samplers), e.g. `Texture2D[]` / `SamplerState[]` used for
bindless descriptor indexing. SPIR-V validation rejects explicit layout
decorations on runtime arrays of opaque types
(VUID-StandaloneSpirv-None-10684), so `vkCreateShaderModule` fails under the
validation layer.

This rewrites the SPIR-V assembly to drop `ArrayStride` only on runtime arrays
whose element type is an opaque (image or sampler) type. Struct/SSBO runtime
arrays keep their stride, which is legal and required.

Usage: fix_spirv.py <input.spv> <output.spv>
"""

import sys
import subprocess


def main() -> int:
    if len(sys.argv) != 3:
        print(f"usage: {sys.argv[0]} <input.spv> <output.spv>", file=sys.stderr)
        return 2

    in_path, out_path = sys.argv[1], sys.argv[2]
    spirv_dis = "spirv-dis"
    spirv_as = "spirv-as"

    # Disassemble to human-readable SPIR-V assembly.
    asm = subprocess.run(
        [spirv_dis, in_path], capture_output=True, text=True, check=True
    ).stdout

    lines = asm.splitlines()

    def strip_percent(s: str) -> str:
        return s[1:] if s.startswith("%") else s

    # First pass: find runtime-array types whose element is an image or sampler.
    # We collect the *type* ids of runtime arrays, then determine which of those
    # have an opaque element by inspecting the referenced element type.
    runtime_arr_types = {}  # runtime_array_type_id -> element_type_id
    type_kind = {}  # type_id -> 'image' | 'sampler' | other
    for line in lines:
        if "OpTypeImage" in line or "OpTypeSampler" in line:
            # e.g. "%76 = OpTypeImage %float 2D 2 0 0 1 Unknown"
            tid = strip_percent(line.split("=")[0].strip())
            type_kind[tid] = "image" if "OpTypeImage" in line else "sampler"
        if "OpTypeRuntimeArray" in line:
            # e.g. "%_runtimearr_76 = OpTypeRuntimeArray %76"
            parts = line.split("=")
            rid = strip_percent(parts[0].strip())
            elem = strip_percent(
                parts[1].split("OpTypeRuntimeArray", 1)[1].strip().split()[0]
            )
            runtime_arr_types[rid] = elem

    # Runtime arrays of opaque element types must not carry ArrayStride.
    illegal_stride_targets = {
        rid
        for rid, elem in runtime_arr_types.items()
        if type_kind.get(elem) in ("image", "sampler")
    }

    out_lines = []
    for line in lines:
        # Drop: "OpDecorate %<id> ArrayStride <n>" where <id> is such a runtime array.
        if "ArrayStride" in line and "OpDecorate" in line:
            deco_id = strip_percent(line.split("%", 1)[1].split()[0].rstrip(":"))
            if deco_id in illegal_stride_targets:
                continue
        out_lines.append(line)

    subprocess.run(
        [spirv_as, "-", "-o", out_path],
        input="\n".join(out_lines) + "\n",
        text=True,
        check=True,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
