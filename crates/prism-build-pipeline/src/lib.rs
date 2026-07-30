//! API surface for the Prisma build pipeline.
//!
//! Currently provides:
//! - `bake_gi()` — offline GI probe-volume baker (GPU ray-query, multi-bounce path tracing)

mod bake_gi;

pub use bake_gi::bake_gi;
pub use bake_gi::BakeGiConfig;
