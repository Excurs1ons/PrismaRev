#!/usr/bin/env python3
"""Generate original placeholder art for `projects/fishing/assets/sprites/`.

The sprite directory is gitignored: it is produced either by
`scripts/sync_fishing_art.py` (reference art, local only) or by this script
(original placeholders, safe to commit-adjacent). Both write the same filenames
so the scene file never changes.

Usage (repo root):  python scripts/gen_placeholder_sprites.py
"""

from __future__ import annotations

import json
from pathlib import Path

import math
from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "projects" / "fishing" / "assets" / "sprites"

# Must match `BACKGROUND` in scripts/sync_fishing_art.py.
BACKGROUND = [
    ("far-mountains", 0.06, (150, 176, 196)),
    ("mountain-right-left", 0.08, (132, 160, 182)),
    ("mountain-right-far", 0.10, (118, 146, 170)),
    ("mountain-right-close", 0.13, (104, 132, 156)),
    ("cloud-1", 0.04, (232, 238, 244)),
    ("seaweed", 0.45, (58, 92, 74)),
    ("mid-ground", 0.55, (74, 104, 86)),
    ("dock-back", 0.85, (96, 78, 62)),
    ("ground", 1.00, (38, 72, 96)),
    ("plants", 0.97, (52, 92, 70)),
    ("dock", 0.90, (122, 96, 72)),
    ("front-rocks", 1.05, (70, 66, 62)),
]
TODS = ("dawn", "day", "dusk", "night")
TOD_TINT = {
    "dawn": (1.00, 0.86, 0.80),
    "day": (1.00, 1.00, 1.00),
    "dusk": (0.92, 0.78, 0.86),
    "night": (0.46, 0.54, 0.78),
}
PROPS = {
    "boat.png": (64, 16, (128, 84, 52)),
    "bobber.png": (16, 32, (214, 72, 64)),
    "hook.png": (5, 6, (196, 196, 200)),
    "splash.png": (960, 16, (220, 236, 246)),
    "ripple.png": (128, 16, (200, 226, 240)),
    "rod.png": (64, 64, (150, 120, 88)),
}
FISH = [
    "Bluegill-Sunfish_L",
    "BrookTrout_L",
    "BrownTrout_L",
    "CommonCarp_L",
    "Crappie_L",
    "LargemouthBass_L",
    "NorthernPike_L",
    "YellowPerch_L",
    "Walleye_L",
    "Lake-Trout_L",
]


def tinted(color, tod):
    t = TOD_TINT[tod]
    return tuple(min(255, int(c * k)) for c, k in zip(color, t))


def layer(size, color, seed):
    im = Image.new("RGBA", size, (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    w, h = size
    # A soft ridge line so parallax motion is visible.
    points = []
    for i in range(0, w + 1, max(1, w // 40)):
        y = int(h * (0.45 + 0.18 * ((i // max(1, w // 8) + seed) % 3) / 2))
        points.append((i, y))
    points += [(w, h), (0, h)]
    d.polygon(points, fill=color + (255,))
    return im


def water_tile(size=(512, 256)):
    """Opaque water body: sky-tinted at the horizon, deepening downwards.

    The top row of the texture is the water line, so the quad hides every
    background layer's hard bottom crop instead of letting the sky bleed
    through (a translucent band over a bright sky reads as grey haze).
    """
    w, h = size
    im = Image.new("RGBA", size, (0, 0, 0, 255))
    px = im.load()
    # sRGB stops: horizon (reflects the sky) -> mid lake -> deep water.
    stops = [(0.0, (92, 122, 138)), (0.18, (58, 92, 116)), (0.55, (34, 62, 88)), (1.0, (18, 36, 58))]

    def sample(t):
        for i in range(len(stops) - 1):
            t0, c0 = stops[i]
            t1, c1 = stops[i + 1]
            if t0 <= t <= t1:
                f = (t - t0) / (t1 - t0)
                return tuple(c0[k] + (c1[k] - c0[k]) * f for k in range(3))
        return stops[-1][1]

    for y in range(h):
        t = y / (h - 1)
        r, g, b = sample(t)
        # Ripples get shorter and lower-contrast with distance (nearer water
        # sits at the bottom of the texture).
        amp = 3.0 + 7.0 * t
        for x in range(w):
            wave = math.sin((x * 0.06) + math.sin(y * 0.9) * 2.0)
            ripple = amp * wave
            px[x, y] = (
                max(0, min(255, int(r + ripple * 0.6))),
                max(0, min(255, int(g + ripple * 0.8))),
                max(0, min(255, int(b + ripple))),
                255,
            )
    return im


def main() -> int:
    (OUT / "bg").mkdir(parents=True, exist_ok=True)
    (OUT / "fish").mkdir(parents=True, exist_ok=True)
    manifest = {"background": [], "props": {}, "fish": {}}

    for order, (slug, factor, color) in enumerate(BACKGROUND):
        variants = {}
        for tod in TODS:
            im = layer((1600, 340), tinted(color, tod), order)
            rel = f"sprites/bg/{slug}@{tod}.png"
            im.save(OUT / "bg" / f"{slug}@{tod}.png")
            variants[tod] = rel
        manifest["background"].append(
            {"slug": slug, "parallax": factor, "order": order, "variants": variants}
        )

    for name, (w, h, color) in PROPS.items():
        if name == "water.png":
            im = water_tile((w, h))
            im.save(OUT / name)
            manifest["props"][name] = f"sprites/{name}"
            continue
        im = Image.new("RGBA", (w, h), (0, 0, 0, 0))
        d = ImageDraw.Draw(im)
        d.rounded_rectangle([1, 1, w - 2, h - 2], radius=max(1, h // 4), fill=color + (255,))
        im.save(OUT / name)
        manifest["props"][name] = f"sprites/{name}"

    for name in FISH:
        im = Image.new("RGBA", (20, 20), (0, 0, 0, 0))
        d = ImageDraw.Draw(im)
        d.ellipse([2, 6, 16, 14], fill=(150, 170, 150, 255))
        d.polygon([(2, 10), (-2 + 4, 5), (2, 15)], fill=(120, 140, 120, 255))
        im.save(OUT / "fish" / f"{name}.png")
        manifest["fish"][name] = f"sprites/fish/{name}.png"

    (OUT / "manifest.json").write_text(
        json.dumps(manifest, indent=1, ensure_ascii=False), encoding="utf-8"
    )
    print(f"placeholders -> {OUT}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
