//! CLI entry point for `prism-build-pipeline`.
//!
//! Subcommands:
//!   bake-gi    — offline GI probe-volume baker (GPU ray-query, multi-bounce path tracing)
//!   cook       — cook assets (delegates to prism-asset)
//!   pack       — build .pak files (delegates to prism-asset)

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "prism-build-pipeline", about = "Prisma build pipeline CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Offline GI probe-volume baker (GPU ray-query, multi-bounce path tracing)
    BakeGi {
        /// Output .bin path (default: assets/gi/probe_volume.bin)
        output: Option<PathBuf>,
        /// Path to .pak resource package (default: assets/packed/scene.pak)
        pak: Option<PathBuf>,
        /// Path to .rscn cooked scene file (default: assets/scenes/default.rscn)
        rscn: Option<PathBuf>,
        /// Rays per probe [default: 64]
        #[arg(long, default_value_t = 64)]
        rays: u32,
        /// Max bounces [default: 3]
        #[arg(long, default_value_t = 3)]
        bounces: u32,
    },
    /// Cook a scene/asset file into cooked format
    Cook {
        /// Input file path
        input: PathBuf,
        /// Output directory (default: assets/cooked)
        #[arg(long, default_value = "assets/cooked")]
        output: PathBuf,
    },
    /// Build a .pak resource package
    Pack {
        /// Input directory containing cooked assets
        input: PathBuf,
        /// Output .pak path (default: assets/packed/game.pak)
        #[arg(long, default_value = "assets/packed/game.pak")]
        output: PathBuf,
    },
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let cli = Cli::parse();

    match cli.command {
        Command::BakeGi {
            output,
            pak,
            rscn,
            rays,
            bounces,
        } => {
            let cfg = prism_build_pipeline::BakeGiConfig {
                output_path: output.unwrap_or_else(|| PathBuf::from("assets/gi/probe_volume.bin")),
                pak_path: pak.unwrap_or_else(|| PathBuf::from("assets/packed/scene.pak")),
                rscn_path: rscn.unwrap_or_else(|| PathBuf::from("assets/scenes/default.rscn")),
                num_rays: rays,
                max_bounce: bounces,
                ..Default::default()
            };
            prism_build_pipeline::bake_gi(&cfg)
        }
        Command::Cook { input, output } => {
            log::info!("cook: {input:?} -> {output:?}");
            // TODO: delegate to prism-asset-cooker via prism-build-pipeline
            anyhow::bail!("cook not yet implemented; use prism-asset-cli directly")
        }
        Command::Pack { input, output } => {
            log::info!("pack: {input:?} -> {output:?}");
            // TODO: delegate to prism-asset-package via prism-build-pipeline
            anyhow::bail!("pack not yet implemented; use prism-asset-cli directly")
        }
    }
}
