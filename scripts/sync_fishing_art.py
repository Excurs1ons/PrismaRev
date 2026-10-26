#!/usr/bin/env python3
"""Copy reference art into `projects/fishing/assets/sprites/` (gitignored).

The engine demo drives real gameplay logic with the reference game's art so we
can match its scale, layering and timing. Nothing here is committed: the output
directory is gitignored and must be replaced with original art before release.

Usage (repo root):

    python scripts/sync_fishing_art.py

Source: `.local/reference/castnchill/textures/` (see `scripts/unpack_reference.py`).
Fallback: `python scripts/gen_placeholder_sprites.py` for original placeholders.
"""

from __future__ import annotations

import json
import shutil
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / ".local" / "reference" / "castnchill" / "textures"
OUT = ROOT / "projects" / "fishing" / "assets" / "sprites"

# Location whose background stack we borrow for the lake slice.
LOCATION = "picnicpoint"

# (ref base name, output slug, parallax factor)
# Layer index (`+<n>_`) grows with distance, so the factor shrinks.
BACKGROUND = [
    ("+70_far-mountains", "far-mountains", 0.06),
    ("+69_mountain-right-left", "mountain-right-left", 0.08),
    ("+68_mountain-right-far-copy", "mountain-right-far", 0.10),
    ("+66_mountain-right-close", "mountain-right-close", 0.13),
    ("+65_cloud1", "cloud-1", 0.04),
    ("+59_seaweed", "seaweed", 0.45),
    ("+22_mid-2", "mid-ground", 0.55),
    ("+2_dock-back", "dock-back", 0.85),
    ("+0_GAME-GROUND", "ground", 1.00),
    ("+0_plants", "plants", 0.97),
    ("+1_dock", "dock", 0.90),
    ("+13_+1_front-rocks", "front-rocks", 1.05),
]

# Gameplay sprites: (ref name, output name)
PROPS = [
    ("Rowboat", "boat.png"),
    ("TopWater_World", "bobber.png"),
    ("Hook", "hook.png"),
    ("FXSplash_M", "splash.png"),
    ("FXBubbleRipple", "ripple.png"),
    ("NormalRod_A_Game", "rod.png"),
]

# Fish used by the lake slice (L tier = the in-water sprite).
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

TODS = ("dawn", "day", "dusk", "night")


def main() -> int:
    if not SRC.is_dir():
        sys.exit(
            f"reference art not found: {SRC}\n"
            "run `python scripts/unpack_reference.py --game <dir> --out .local/reference/castnchill` first"
        )

    index = json.loads((SRC.parent / "index.json").read_text(encoding="utf-8"))
    # Background layers are location-specific; props and fish live in shared
    # bundles, so keep a global map as well.
    by_name: dict[str, dict] = {}
    global_by_name: dict[str, dict] = {}
    for rec in index:
        if rec["type"] != "Texture2D":
            continue
        global_by_name.setdefault(rec["name"].lower(), rec)
        if LOCATION in rec["source"]:
            by_name.setdefault(rec["name"].lower(), rec)

    (OUT / "bg").mkdir(parents=True, exist_ok=True)
    (OUT / "fish").mkdir(parents=True, exist_ok=True)

    manifest = {"background": [], "props": {}, "fish": {}}
    missing: list[str] = []

    # --- background stack: one file per time of day -----------------------
    for order, (ref, slug, factor) in enumerate(BACKGROUND):
        entry = {"slug": slug, "parallax": factor, "order": order, "variants": {}}
        for tod in TODS:
            rec = by_name.get(f"{ref.lower()}@{tod}")
            if rec is None:
                missing.append(f"{ref}@{tod}")
                continue
            dst = OUT / "bg" / f"{slug}@{tod}.png"
            shutil.copyfile(SRC / f"{rec['path'].split('/')[-1]}", dst)
            entry["variants"][tod] = f"sprites/bg/{slug}@{tod}.png"
        if entry["variants"]:
            manifest["background"].append(entry)

    # --- props ------------------------------------------------------------
    for ref, out_name in PROPS:
        rec = global_by_name.get(ref.lower())
        if rec is None:
            missing.append(ref)
            continue
        shutil.copyfile(SRC / f"{rec['path'].split('/')[-1]}", OUT / out_name)
        manifest["props"][ref] = f"sprites/{out_name}"

    # --- fish -------------------------------------------------------------
    for ref in FISH:
        rec = global_by_name.get(ref.lower())
        if rec is None:
            missing.append(ref)
            continue
        dst = OUT / "fish" / f"{ref}.png"
        shutil.copyfile(SRC / f"{rec['path'].split('/')[-1]}", dst)
        manifest["fish"][ref] = f"sprites/fish/{ref}.png"

    (OUT / "manifest.json").write_text(
        json.dumps(manifest, indent=1, ensure_ascii=False), encoding="utf-8"
    )

    layers = len(manifest["background"])
    print(f"background layers: {layers}")
    print(f"props: {len(manifest['props'])}  fish: {len(manifest['fish'])}")
    if missing:
        print(f"missing ({len(missing)}): {', '.join(missing[:10])}")
    print(f"-> {OUT}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
