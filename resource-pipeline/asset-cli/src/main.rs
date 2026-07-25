//! # asset-cli
//!
//! Command-line interface for the PrismaRev Resource Pipeline.
//!
//! ## Usage
//!
//! ```bash
//! # Initialize a new project
//! asset-cli init
//!
//! # Scan the Assets/ directory
//! asset-cli scan
//!
//! # Import all assets (run importers + cache)
//! asset-cli import
//!
//! # Build a .pak for distribution
//! asset-cli build --output game.pak --compression 3
//!
//! # Validate an existing .pak
//! asset-cli validate game.pak
//!
//! # List registered assets
//! asset-cli list
//!
//! # Inspect a specific asset
//! asset-cli inspect 10001
//! ```

use asset_cooker::{default_cooker_registry, profile, CookPipeline};
use asset_core::AssetType;
use asset_db::{AssetDatabase, ImportCache};
use asset_importer::{default_importer_registry, ImportPipeline};
use asset_package::PackageBuilder;

use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(name = "asset-cli", about = "PrismaRev Resource Pipeline CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Project root (defaults to cwd).
    #[arg(long, global = true)]
    project: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize a new project structure.
    Init {
        /// Target directory (defaults to cwd).
        dir: Option<PathBuf>,
    },

    /// Scan the Assets/ directory and update the database.
    Scan,

    /// Import assets (run importers).
    Import {
        /// Force re-import even if cached.
        #[arg(long, short)]
        force: bool,
    },

    /// Build a .pak package.
    Build {
        /// Output .pak file path.
        #[arg(long, short, default_value = "game.pak")]
        output: PathBuf,

        /// Compression level (0-10, 0=no compression).
        #[arg(long, short = 'l', default_value = "3")]
        compression: u32,

        /// Platform target (reserved for future use).
        #[arg(long)]
        platform: Option<String>,
    },

    /// Validate a .pak file.
    Validate {
        /// Path to the .pak file.
        pak: PathBuf,
    },

    /// List all registered assets.
    List,

    /// Inspect a single asset.
    Inspect {
        /// Asset ID or path.
        id: String,
    },
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn project_root(cli: &Cli) -> PathBuf {
    cli.project
        .clone()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

fn assets_dir(root: &Path) -> PathBuf {
    root.join("Assets")
}

fn library_dir(root: &Path) -> PathBuf {
    root.join("Library")
}

fn db_path(root: &Path) -> PathBuf {
    library_dir(root).join("AssetDatabase.json")
}

fn cache_path(root: &Path) -> PathBuf {
    library_dir(root).join("import_cache.json")
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

fn cmd_init(dir: &Path) -> anyhow::Result<()> {
    let assets = assets_dir(dir);
    let library = library_dir(dir);

    std::fs::create_dir_all(&assets)?;
    std::fs::create_dir_all(&library)?;

    let db = AssetDatabase::new();
    db.save(&db_path(dir))?;

    let cache = ImportCache::new();
    cache.save(&cache_path(dir))?;

    println!("✅  Initialized resource pipeline in {}", dir.display());
    println!("   Assets/   – place source files here");
    println!("   Library/  – database & import cache");
    Ok(())
}

fn cmd_scan(root: &Path, _cli: &Cli) -> anyhow::Result<()> {
    let assets = assets_dir(root);
    if !assets.exists() {
        anyhow::bail!("Assets/ directory not found. Run 'asset-cli init' first.");
    }

    let mut db = AssetDatabase::load(&db_path(root))?;
    let mut scanned = 0u32;

    walk_directory(&assets, &mut |path| {
        let relative = path
            .strip_prefix(&assets)
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();

        // Skip if already known.
        if db.get_by_path(&relative).is_some() {
            return;
        }

        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        let asset_type = AssetType::from_extension(extension);

        let id = db.generate_id();
        let record = asset_db::AssetRecord::new(id, relative, asset_type, "raw");
        db.insert(record).unwrap();
        scanned += 1;
    });

    db.save(&db_path(root))?;
    println!("✅  Scan complete: {} new, {} total assets", scanned, db.len());
    Ok(())
}

fn cmd_import(root: &Path, _force: bool, _cli: &Cli) -> anyhow::Result<()> {
    let assets = assets_dir(root);
    if !assets.exists() {
        anyhow::bail!("Assets/ directory not found. Run 'asset-cli init' first.");
    }

    let mut db = AssetDatabase::load(&db_path(root))?;
    let mut cache = ImportCache::load(&cache_path(root))?;

    let registry = Arc::new(default_importer_registry());
    let pipeline = ImportPipeline::new(registry);

    // First, register all source files in the DB so we have records.
    walk_directory(&assets, &mut |path| {
        let relative = path
            .strip_prefix(&assets)
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();
        if db.get_by_path(&relative).is_none() {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let asset_type = AssetType::from_extension(ext);
            let id = db.generate_id();
            let record = asset_db::AssetRecord::new(id, relative, asset_type, "raw");
            db.insert(record).ok();
        }
    });

    // Now import each file.
    let count = db.len();
    let mut imported = 0u32;
    let mut cached = 0u32;

    // Collect records as vec since import_file borrows db mutably.
    let paths: Vec<String> = db.records().map(|r| r.path.clone()).collect();

    for relative in &paths {
        let source_path = assets.join(&relative);
        if !source_path.exists() {
            continue;
        }
        match pipeline.import_file(&source_path, &mut db, &mut cache, None) {
            Ok(true) => imported += 1,
            Ok(false) => cached += 1,
            Err(e) => eprintln!("  ⚠  {}: {e}", relative),
        }
    }

    db.save(&db_path(root))?;
    cache.save(&cache_path(root))?;

    println!(
        "✅  Import complete: {imported} imported, {cached} cached, {count} total records"
    );
    Ok(())
}

fn cmd_build(
    root: &Path,
    output: &Path,
    compression: u32,
    platform: &Option<String>,
    _cli: &Cli,
) -> anyhow::Result<()> {
    if !db_path(root).exists() {
        anyhow::bail!("No AssetDatabase.json found. Run 'asset-cli scan' first.");
    }

    let db = AssetDatabase::load(&db_path(root))?;
    if db.is_empty() {
        anyhow::bail!("No assets to build. Run 'asset-cli scan' first.");
    }

    // Load imported data: for now, just read source files.
    let assets = assets_dir(root);
    let mut asset_data: std::collections::HashMap<asset_core::AssetId, Vec<u8>> =
        std::collections::HashMap::new();
    for record in db.records() {
        let source_path = assets.join(&record.path);
        if let Ok(data) = std::fs::read(&source_path) {
            asset_data.insert(record.id, data);
        } else {
            eprintln!("  ⚠  cannot read {}", record.path);
        }
    }

    let cooker_reg = default_cooker_registry();
    let pipeline = CookPipeline::new(cooker_reg);

    let compression_level = compression.min(22) as i32;
    let mut builder = PackageBuilder::new();
    builder.set_compression(compression_level);

    let settings = profile::CookSettings {
        platform: platform.clone().unwrap_or_else(|| "desktop".to_owned()),
        ..Default::default()
    };

    let summary = pipeline.cook_all(&db, &asset_data, &mut builder, &settings)?;

    let pak = builder.build()?;
    std::fs::write(output, &pak)?;

    println!(
        "✅  Build complete: {} cooked, {} skipped → {} ({:.1} KB)",
        summary.cooked,
        summary.skipped,
        output.display(),
        pak.len() as f64 / 1024.0,
    );
    Ok(())
}

fn cmd_validate(pak_path: &Path) -> anyhow::Result<()> {
    use asset_package::PackageReader;

    let pak = std::fs::read(pak_path)?;
    // from_bytes validates magic + version + checksum
    let reader = PackageReader::from_bytes(&pak).map_err(|e| anyhow::anyhow!("{e}"))?;

    let magic_str = std::str::from_utf8(&reader.header().magic).unwrap_or("????");
    println!("📦  Magic:    {magic_str}");
    println!("🔢  Version:  {}", reader.header().version);
    println!("📊  Assets:   {}", reader.asset_count());
    println!("✅  Checksum: PASS");
    Ok(())
}

fn cmd_list(root: &Path, _cli: &Cli) -> anyhow::Result<()> {
    if !db_path(root).exists() {
        anyhow::bail!("No AssetDatabase.json found. Run 'asset-cli scan' first.");
    }

    let db = AssetDatabase::load(&db_path(root))?;
    println!("📋  Asset Database ({} records):", db.len());
    for record in db.records() {
        println!(
            "  {:>8}  {:?}  {}",
            record.id, record.asset_type, record.path
        );
    }
    Ok(())
}

fn cmd_inspect(root: &Path, id_or_path: &str) -> anyhow::Result<()> {
    if !db_path(root).exists() {
        anyhow::bail!("No AssetDatabase.json found.");
    }

    let db = AssetDatabase::load(&db_path(root))?;

    // Try to parse as numeric ID first.
    let record = if let Ok(num) = id_or_path.parse::<u64>() {
        db.get(asset_core::AssetId::from_raw(num))
    } else {
        db.get_by_path(id_or_path)
    };

    match record {
        Some(r) => {
            println!("ID:           {}", r.id);
            println!("Path:         {}", r.path);
            println!("Type:         {:?}", r.asset_type);
            println!("Importer:     {}", r.importer_name);
            println!("State:        {:?}", r.state);
            println!("Version:      {}", r.version);
            println!("Source Hash:  {:#x}", r.source_hash);
            println!("Deps:         {} assets", r.dependencies.len());
            for dep in &r.dependencies {
                if let Some(dep_record) = db.get(*dep) {
                    println!("  → {}  {}", dep, dep_record.path);
                } else {
                    println!("  → {}  (missing)", dep);
                }
            }
        }
        None => anyhow::bail!("Asset not found: {id_or_path}"),
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Entrypoint
// ---------------------------------------------------------------------------

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();
    let root = project_root(&cli);

    match &cli.command {
        Commands::Init { dir } => cmd_init(dir.as_deref().unwrap_or(&root)),
        Commands::Scan => cmd_scan(&root, &cli),
        Commands::Import { force } => cmd_import(&root, *force, &cli),
        Commands::Build {
            output,
            compression,
            platform,
        } => cmd_build(&root, output, *compression, platform, &cli),
        Commands::Validate { pak } => cmd_validate(pak),
        Commands::List => cmd_list(&root, &cli),
        Commands::Inspect { id } => cmd_inspect(&root, id),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn walk_directory(dir: &Path, cb: &mut impl FnMut(PathBuf)) {
    if !dir.is_dir() {
        return;
    }
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk_directory(&path, cb);
            } else if path.is_file() {
                cb(path);
            }
        }
    }
}
