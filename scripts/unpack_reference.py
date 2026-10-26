#!/usr/bin/env python3
"""Unpack the Cast n Chill reference build into `.local/reference/castnchill/`.

The extracted art/audio is **reference material only** — it is gitignored and
must never be shipped or committed. We use it to measure sprite sizes, pivots,
palettes and layout, then redraw our own assets.

Usage (from the repo root):

    python scripts/unpack_reference.py \
        --game "D:/Desktop/Cast n Chill" \
        --out .local/reference/castnchill

Requires `UnityPy` (pip install UnityPy) and Pillow.

Layout written under --out:

    sprites/<name>.png        Sprite objects (atlas rect cropped)
    textures/<name>.png       Texture2D objects that are not sprite sources
    text/<name>.txt|json      TextAsset (localisation, configs, tables)
    other/<name>.<ext>        MonoBehaviour / Font / Material dumps (JSON)
    index.json                one record per object: type, name, source file,
                              pixel size, PPU, pivot, rect
    summary.txt               per-type counts + failures

Bundles live in `castnchill_Data/StreamingAssets/aa/StandaloneWindows64/*.bundle`;
the rest of the content is in `castnchill_Data/*.assets`.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path

try:
    import UnityPy
except ImportError:  # pragma: no cover - guidance only
    sys.exit(
        "UnityPy is required: python -m pip install UnityPy\n"
        "(Pillow is already needed for image export.)"
    )


SAFE_NAME = re.compile(r"[^A-Za-z0-9._-]+")


def safe(name: str, fallback: str) -> str:
    cleaned = SAFE_NAME.sub("_", (name or "").strip()).strip("._")
    return cleaned or fallback


def asset_sources(game_dir: Path) -> list[Path]:
    data = game_dir / "castnchill_Data"
    out: list[Path] = []
    for pattern in ("*.assets", "*.bundle", "*.unity3d"):
        out.extend(sorted(data.glob(pattern)))
    aa = data / "StreamingAssets" / "aa"
    if aa.is_dir():
        out.extend(sorted(aa.rglob("*.bundle")))
    return [p for p in out if p.is_file()]


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--game", required=True, type=Path, help="Cast n Chill install dir")
    ap.add_argument("--out", required=True, type=Path, help="output dir (gitignored)")
    ap.add_argument(
        "--unity-version",
        default="6000.3.26f1",
        help="fallback Unity version for bundles that omit it",
    )
    ap.add_argument(
        "--only",
        default=None,
        help="only unpack containers whose path contains this substring",
    )
    ap.add_argument(
        "--group-by-source",
        action="store_true",
        help="prefix exported files with the source container name so different "
        "levels do not overwrite each other",
    )
    args = ap.parse_args()

    # AssetBundles built by Addressables do not always embed the editor version;
    # without a fallback UnityPy refuses to parse them.
    UnityPy.config.FALLBACK_UNITY_VERSION = args.unity_version

    game: Path = args.game
    out: Path = args.out
    if not (game / "castnchill_Data").is_dir():
        return print(f"not a Unity build: {game}") or 1

    for sub in ("sprites", "textures", "text", "other"):
        (out / sub).mkdir(parents=True, exist_ok=True)

    sources = asset_sources(game)
    if args.only:
        sources = [p for p in sources if args.only in p.relative_to(game).as_posix()]
    print(f"{len(sources)} container(s)")

    records: list[dict] = []
    counts: dict[str, int] = {}
    failures: list[str] = []
    seen: set[tuple[str, str, str]] = set()

    for source in sources:
        rel = source.relative_to(game).as_posix()
        group = safe(source.stem, "container") if args.group_by_source else ""
        prefix = f"{group}/" if group else ""
        print(f"  {rel}")
        try:
            env = UnityPy.load(str(source))
        except Exception as exc:  # noqa: BLE001 - report and continue
            failures.append(f"{rel}: load failed: {exc}")
            continue

        for obj in env.objects:
            kind = obj.type.name
            try:
                data = obj.read()
            except Exception as exc:  # noqa: BLE001
                failures.append(f"{rel}: {kind} read failed: {exc}")
                continue

            name = getattr(data, "m_Name", "") or getattr(data, "name", "")
            record: dict = {"type": kind, "name": name, "source": rel}

            try:
                if kind == "Sprite":
                    image = data.image
                    size = list(image.size)
                    record.update(
                        {
                            "width": size[0],
                            "height": size[1],
                            "pixels_per_unit": getattr(data, "m_PixelsPerUnit", None),
                            "pivot": list(getattr(data, "m_Pivot", []) or []),
                            "rect": list(getattr(data, "m_Rect", {}) or []),
                            "path": f"sprites/{prefix}{safe(name, 'sprite')}.png",
                        }
                    )
                    key = (kind, name, f"{size[0]}x{size[1]}")
                    if key not in seen:
                        seen.add(key)
                        target = out / record["path"]
                        target.parent.mkdir(parents=True, exist_ok=True)
                        image.save(target)

                elif kind == "Texture2D":
                    image = data.image
                    size = list(image.size)
                    record.update(
                        {
                            "width": size[0],
                            "height": size[1],
                            "path": f"textures/{prefix}{safe(name, 'texture')}.png",
                        }
                    )
                    key = (kind, name, f"{size[0]}x{size[1]}")
                    if key not in seen:
                        seen.add(key)
                        target = out / record["path"]
                        target.parent.mkdir(parents=True, exist_ok=True)
                        image.save(target)

                elif kind == "TextAsset":
                    script = getattr(data, "m_Script", "")
                    if isinstance(script, (bytes, bytearray)):
                        text = bytes(script).decode("utf-8", "replace")
                    else:
                        text = str(script)
                    ext = "json" if text.lstrip().startswith(("{", "[")) else "txt"
                    record["path"] = f"text/{safe(name, 'text')}.{ext}"
                    (out / record["path"]).write_text(text, encoding="utf-8")
                    record["chars"] = len(text)

                elif kind == "MonoBehaviour":
                    # Typetree may be unavailable without the game's assemblies;
                    # keep the raw read() dump so names/fields stay searchable.
                    dump = {
                        k: v
                        for k, v in vars(data).items()
                        if not k.startswith("_") and isinstance(v, (str, int, float, bool, list, dict))
                    }
                    record["path"] = f"other/{safe(name, 'mono')}.json"
                    (out / record["path"]).write_text(
                        json.dumps(dump, ensure_ascii=False, indent=1, default=str),
                        encoding="utf-8",
                    )
                else:
                    continue
            except Exception as exc:  # noqa: BLE001
                failures.append(f"{rel}: {kind} '{name}' export failed: {exc}")
                continue

            counts[kind] = counts.get(kind, 0) + 1
            records.append(record)

    records.sort(key=lambda r: (r["type"], r["name"]))
    (out / "index.json").write_text(
        json.dumps(records, ensure_ascii=False, indent=1, default=str), encoding="utf-8"
    )

    lines = [f"containers: {len(sources)}", ""]
    lines += [f"{kind:16} {n}" for kind, n in sorted(counts.items())]
    lines += ["", f"failures: {len(failures)}"]
    lines += [f"  {f}" for f in failures[:200]]
    (out / "summary.txt").write_text("\n".join(lines) + "\n", encoding="utf-8")

    print("\n".join(lines[: 3 + len(counts)]))
    print(f"\nwrote {len(records)} records -> {out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
