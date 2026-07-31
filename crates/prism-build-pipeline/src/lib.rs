//! API surface for the Prisma build pipeline.
//!
//! Currently provides:
//! - `bake_gi()` — offline GI probe-volume baker (GPU ray-query, multi-bounce path tracing)
//! - `heightmap` — erosion-based heightmap generator (thermal + hydraulic)

mod bake_gi;
mod heightmap;

pub use bake_gi::bake_gi;
pub use bake_gi::BakeGiConfig;
pub use heightmap::{generate_eroded_heightmap, generate_terrain, ErosionParams, Heightmap};
