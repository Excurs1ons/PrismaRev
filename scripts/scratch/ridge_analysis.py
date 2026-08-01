#!/usr/bin/env python3
"""检测高度图的山脊结构：脊线像素、脊线连贯性（相邻脊线像素的连通性）。"""
import struct
import sys

def load_raw(path):
    raw = open(path, 'rb').read()
    return struct.unpack('<%df' % (len(raw) // 4), raw)

vals = load_raw(sys.argv[1])
n = len(vals)
w = h = int(n ** 0.5)

# 山脊 = 高于两侧的像素（水平方向比左右都高，或垂直方向比上下都高）
ridge = [[False] * w for _ in range(h)]
for y in range(1, h - 1):
    for x in range(1, w - 1):
        v = vals[y * w + x]
        h_ridge = v > vals[y * w + x - 1] and v > vals[y * w + x + 1]
        v_ridge = v > vals[(y - 1) * w + x] and v > vals[(y + 1) * w + x]
        if h_ridge or v_ridge:
            ridge[y][x] = True

total_ridge = sum(sum(row) for row in ridge)
print(f'ridge pixels: {total_ridge} ({100.0 * total_ridge / n:.1f}%)')

# 连通性：每个脊线像素周围 8 邻域中还有多少脊线像素
# 孤立点（邻域 0 个）= 噪点脊；连贯脊线（邻域 >= 2 个）= 山脉
isolated = 0
connected = 0
for y in range(1, h - 1):
    for x in range(1, w - 1):
        if not ridge[y][x]:
            continue
        cnt = 0
        for dy in (-1, 0, 1):
            for dx in (-1, 0, 1):
                if dx == 0 and dy == 0:
                    continue
                if ridge[y + dy][x + dx]:
                    cnt += 1
        if cnt == 0:
            isolated += 1
        elif cnt >= 2:
            connected += 1

print(f'isolated ridges (noise): {isolated} ({100.0 * isolated / max(total_ridge, 1):.1f}%)')
print(f'connected ridges (mountain): {connected} ({100.0 * connected / max(total_ridge, 1):.1f}%)')
