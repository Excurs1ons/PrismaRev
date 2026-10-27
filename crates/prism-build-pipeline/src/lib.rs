//! API surface for the Prisma build pipeline.
//!
//! Currently provides:
//! - `bake_gi()` — offline GI probe-volume baker (GPU ray-query, multi-bounce path tracing)
//! - `heightmap` — erosion-based heightmap generator (thermal + hydraulic)
//! - `build_scene_package()` — cook `scenes.toml` entries and pack `scenes.pak`

mod bake_gi;
mod heightmap;
mod scene_pack;

pub use bake_gi::bake_gi;
pub use bake_gi::gi_bake_dispatch_groups;
pub use bake_gi::unpack_baked_probe_volume;
pub use bake_gi::BakeGiConfig;
pub use heightmap::{generate_eroded_heightmap, generate_terrain, ErosionParams, Heightmap};
pub use scene_pack::{build_scene_package, ScenePackSummary};
