---
name: prism-asset-cli
description: Use when building, importing, or validating game assets with the prism-asset pipeline. Also when checking asset pipeline health or debugging .pak issues.
---

# prism-asset-cli

`prism-asset-cli` is the offline resource pipeline CLI for PrismaRev. It processes source assets (textures, meshes, JSON, raw) through Import → Cook → Package stages into `.pak` archives that the engine runtime consumes.

## Build & Run

```bash
# From the project root (or from prism-asset/ directory)
cd prism-asset
cargo build -p prism-asset-cli
cargo run -p prism-asset-cli -- <command> [options]
```

The binary is `prism-asset-cli` (`.exe` on Windows).

## Subcommands

| Command | Description |
|---------|-------------|
| `init` | Create `Assets/` + `Library/` directory structure. First command in a new project. |
| `scan` | Scan `Assets/` directory, detect file types by extension, write asset records to database. |
| `import` | Run importers on all scanned assets. Incremental — skips cached files unless `--force`. |
| `build -o game.pak` | Cook all assets (generate mips for textures, optimize meshes) and package into `.pak`. |
| `validate <file.pak>` | Verify magic, version, checksum of a `.pak` file. Shows human-readable asset list if `.pak.meta.json` exists alongside. |
| `list` | List all registered assets from the database (IDs, types, paths). |
| `inspect <id>` | Show detailed info for a single asset (dependencies, hash, version, state). |

**Shorthand**: pass `.pak` path directly as positional arg (equivalent to `validate`).

### Global Options

| Option | Description |
|--------|-------------|
| `--project <dir>` | Project root directory (default: current working directory) |

## Common Workflows

### Full Pipeline (new assets)

```bash
cd path/to/project
prism-asset-cli init
prism-asset-cli scan
prism-asset-cli import
prism-asset-cli build --output game.pak
prism-asset-cli validate game.pak
```

### Quick Validation

```bash
prism-asset-cli game.pak                    # shorthand validate
prism-asset-cli validate game.pak           # explicit validate
```

### Re-Import (force)

```bash
prism-asset-cli import --force              # skip cache, re-import everything
```

After re-importing, re-build with `prism-asset-cli build -o game.pak`.

### Inspect Asset Details

```bash
prism-asset-cli inspect 10001               # by ID (from list output)
prism-asset-cli inspect textures/stone.png  # by path (relative to Assets/)
```

## Asset Pipeline Overview

```
Source (Assets/) → [Import] → Intermediate (RTXI/RMXI) → [Cook] → Runtime (RTEX/RMES) → [Package] → .pak
```

- **Import**: Decodes source files (PNG, glTF, JSON) into intermediate CPU formats (RTXI for textures, RMXI for meshes)
- **Cook**: Converts intermediates to runtime-optimized formats (RTEX with mip chains, RMES with interleaved vertices)
- **Package**: Bundles cooked assets into a single `.pak` file with zstd compression and xxh3 checksums
- **Runtime**: `ResourceManager` (in `prism-asset-runtime`) loads `.pak` files at runtime, lazy-loading assets by `Handle<T>` with memory budget control

## .pak Metadata

When building, `prism-asset-cli build` generates a `.pak.meta.json` file alongside the `.pak`. This human-readable JSON contains asset names, types, sizes, and compression ratios. The `validate` command reads it to show asset names instead of raw IDs.

## Tips

- Always run **`import` before `build`** — build needs cooked data from the import stage.
- The `.pak.meta.json` is for development inspection only. The runtime never reads it.
- `scan` only discovers files — it doesn't decode them. Run `import` to actually process content.
- Import uses xxh3 content hashing for incremental builds. Changed files are re-imported automatically.
- Compression level 0 skips compression entirely (fastest builds, largest .pak). Level 3 is recommended for development. Levels 6-10 for release builds.
- Building a project without a database (`AssetDatabase.json`) will fail — always start with `init`.
