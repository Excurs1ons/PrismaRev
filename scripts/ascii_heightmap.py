#!/usr/bin/env python3
"""把高度图渲染成 ASCII 灰度轮廓（大尺度，看山脉形态）。"""
import struct
import sys

def load_raw(path):
    raw = open(path, 'rb').read()
    return struct.unpack('<%df' % (len(raw) // 4), raw)

vals = load_raw(sys.argv[1])
n = len(vals)
w = h = int(n ** 0.5)

# 降采样到 64x64
step = w // 64
chars = ' .:-=+*#%@'
for y in range(0, h, step):
    line = ''
    for x in range(0, w, step):
        v = vals[y * w + x]
        # 海平面以下用 '~' 系，以上用地形字符
        if v < 0:
            t = max(0.0, min(1.0, -v / 11000.0))
            idx = min(3, int(t * 3))
            line += '~' * (idx + 1)
        else:
            t = max(0.0, min(1.0, v / 8850.0))
            line += chars[min(len(chars) - 1, int(t * (len(chars) - 1)))]
    print(line)
