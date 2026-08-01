#!/usr/bin/env python3
"""渲染 512×512 高度图 raw 为 PNG 预览图（海洋深蓝→浅蓝，陆地绿→棕→雪顶）。"""
import struct
import sys
from PIL import Image

raw_path = sys.argv[1] if len(sys.argv) > 1 else 'assets/heightmaps/terrain_512.raw'
out_path = sys.argv[2] if len(sys.argv) > 2 else 'assets/heightmaps/terrain_512_preview.png'

raw = open(raw_path, 'rb').read()
vals = struct.unpack('<%df' % (len(raw) // 4), raw)
w = h = int(len(vals) ** 0.5)

img = Image.new('RGB', (w, h))
px = img.load()
for y in range(h):
    for x in range(w):
        v = vals[y * w + x]
        if v < 0:
            t = max(0.0, min(1.0, -v / 11000.0))
            r, g, b = int(8 + 30 * t), int(18 + 50 * t), int(70 + 140 * t)
        else:
            t = max(0.0, min(1.0, v / 8850.0))
            if t < 0.45:
                r, g, b = int(34 + 170 * (t / 0.45)), int(139 + 60 * (t / 0.45)), int(34 + 10 * (t / 0.45))
            elif t < 0.75:
                tt = (t - 0.45) / 0.30
                r, g, b = int(204 - 70 * tt), int(199 - 80 * tt), int(44 - 20 * tt)
            else:
                tt = (t - 0.75) / 0.25
                r, g, b = int(134 + 121 * tt), int(119 + 136 * tt), int(24 + 231 * tt)
        px[x, y] = (r, g, b)

img = img.resize((1024, 1024), Image.BILINEAR)
img.save(out_path)
print(f'PNG saved: {out_path} ({img.size[0]}x{img.size[1]})')
