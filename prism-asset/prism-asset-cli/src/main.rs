//! # prism-asset-cli
//!
//! Command-line 接口 for the PrismaRev 资源 管线
//!
//! ## 用法
//!
//! ```bash
//! # Initialize a new project
//! prism-asset-cli init
//!
//! # Scan the Assets/ directory
//! prism-asset-cli scan
//!
//! # 导入 all assets (run importers + cache)
//! prism-asset-cli 导入
//!
//! # 构建 a .pak for distribution
//! prism-asset-cli 构建 --output game.pak --compression 3
//!
//! # Validate an existing .pak
//! prism-asset-cli validate game.pak
//!
//! # 列表 registered assets
//! prism-asset-cli 列表
//!
//! # Inspect a specific 资源
//! prism-asset-cli inspect 10001
//! ```

use prism_asset_cooker::{default_cooker_registry, profile, CookPipeline};
use prism_asset_core::AssetType;
use prism_asset_db::{AssetDatabase, ImportCache};
use prism_asset_importer::{default_importer_registry, ImportPipeline};
use prism_asset_package::PackageBuilder;

use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use unicode_width::UnicodeWidthStr;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// 格式 a byte count for human-readable display.
fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    const TB: f64 = GB * 1024.0;

    let b = bytes as f64;
    if b >= TB {
        format!("{:.1} TB", b / TB)
    } else if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

/// Pad a 字符串 to a 目标 terminal display 宽度 accounting for CJK
/// characters (which occupy 2 columns). If `s` is wider than 宽度 no
/// 填充 is added.
fn pad_display(s: &str, width: usize) -> String {
    let sw = UnicodeWidthStr::width(s);
    if sw >= width {
        s.to_owned()
    } else {
        let pad = " ".repeat(width - sw);
        format!("{s}{pad}")
    }
}

/// Print a bilingual key-value pair with the `/` aligned.
/// `key_en_width` is the display 宽度 to pad the EN text to, so that
/// ` / ` starts at the same 列 across all rows.
fn kv(key_en: &str, key_cn: &str, value: &str, key_en_width: usize) {
    let label = format!("{} / {}", pad_display(key_en, key_en_width), key_cn);
    println!("  {}  {}", label, value);
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(name = "prism-asset-cli", about = "PrismaRev Resource Pipeline CLI\n资源管线命令行工具")]
#[command(args_conflicts_with_subcommands = true)]
struct Cli {
    /// .pak file to inspect / validate (shorthand for `validate`).
    /// 直接传入 .pak 路径来验证（`validate` 的快捷方式）
    #[arg(index = 1)]
    pak: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Commands>,

    /// Project root (defaults to cwd).
    /// 项目根目录（默认为当前目录）
    #[arg(long, global = true)]
    project: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize a new project structure. 初始化新项目
    Init {
        /// Target directory (defaults to cwd). 目标目录（默认当前目录）
        dir: Option<PathBuf>,
    },

    /// Scan the Assets/ directory and 更新 the database.
    /// 扫描 Assets/ 目录并更新数据库
    Scan,

    /// Import assets (run importers). 导入资源（执行导入器）
    Import {
        /// Force re-import even if cached. 强制重新导入（跳过缓存）
        #[arg(long, short)]
        force: bool,
    },

    /// Build a .pak package. 构建 .pak 资源包
    Build {
        /// Output .pak file path. 输出 .pak 文件路径
        #[arg(long, short, default_value = "game.pak")]
        output: PathBuf,

        /// Compression level (0-10, 0=no compression). 压缩级别（0-10，0=不压缩）
        #[arg(long, short = 'l', default_value = "3")]
        compression: u32,

        /// Platform target (reserved for future use). 目标平台（预留）
        #[arg(long)]
        platform: Option<String>,
    },

    /// Validate a .pak file. 验证 .pak 文件
    Validate {
        /// Path to the .pak file. .pak 文件路径
        pak: PathBuf,
    },

    /// List all registered assets. 列出所有已注册资源
    List,

    /// Inspect a single asset. 查看单个资源详情
    Inspect {
        /// Asset ID or path. 资源 ID 或路径
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

    println!("✅  Resource pipeline initialized / 资源管线已初始化: {}", dir.display());
    println!("   Assets/    –  Place source files here / 放置源文件");
    println!("   Library/   –  Database & import cache / 数据库和导入缓存");
    Ok(())
}

fn cmd_scan(root: &Path, _cli: &Cli) -> anyhow::Result<()> {
    let assets = assets_dir(root);
    if !assets.exists() {
        anyhow::bail!("Assets/ directory not found / 目录不存在. Run 'prism-asset-cli init' first.");
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
        let record = prism_asset_db::AssetRecord::new(id, relative, asset_type, "raw");
        db.insert(record).unwrap();
        scanned += 1;
    });

    db.save(&db_path(root))?;
    println!("✅  Scan complete / 扫描完成: {} new/新增, {} total/总计", scanned, db.len());
    Ok(())
}

fn cmd_import(root: &Path, _force: bool, _cli: &Cli) -> anyhow::Result<()> {
    let assets = assets_dir(root);
    if !assets.exists() {
        anyhow::bail!("Assets/ directory not found / 目录不存在. Run 'prism-asset-cli init' first.");
    }

    let mut db = AssetDatabase::load(&db_path(root))?;
    let mut cache = ImportCache::load(&cache_path(root))?;

    let registry = Arc::new(default_importer_registry());
    let pipeline = ImportPipeline::new(registry);

    // 第一个 register all 源 files in the DB so we have records.
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
            let record = prism_asset_db::AssetRecord::new(id, relative, asset_type, "raw");
            db.insert(record).ok();
        }
    });

    // Now 导入 each file.
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
            Ok(r) if r.was_imported => imported += 1,
            Ok(_) => cached += 1,
            Err(e) => eprintln!("  ⚠  {}: {e}", relative),
        }
    }

    db.save(&db_path(root))?;
    cache.save(&cache_path(root))?;

    println!(
        "✅  Import complete / 导入完成: {imported} imported/导入, {cached} cached/缓存, {count} total/总计"
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
        anyhow::bail!("No AssetDatabase.json found / 未找到数据库. Run 'prism-asset-cli scan' first.");
    }

    let db = AssetDatabase::load(&db_path(root))?;
    if db.is_empty() {
        anyhow::bail!("No assets to build / 没有可构建的资源. Run 'prism-asset-cli scan' first.");
    }

    // 加载 imported data: for now, just 读取 源 files.
    let assets = assets_dir(root);
    let mut asset_data: std::collections::HashMap<prism_asset_core::AssetId, Vec<u8>> =
        std::collections::HashMap::new();
    for record in db.records() {
        let source_path = assets.join(&record.path);
        if let Ok(data) = std::fs::read(&source_path) {
            asset_data.insert(record.id, data);
        } else {
            eprintln!("  ⚠  cannot read / 无法读取 {}", record.path);
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

    // ── 写入 a human-readable .meta.json alongside the .pak ──────────
    //
    // This file is purely for inspection (validate / pak-info).  It
    // pairs the AssetDatabase records (which have paths, types,
    // importer names) with the per-asset sizes from the actual .pak.
    // The 运行时 never reads this file.
    let meta_path = output.with_extension("pak.meta.json");
    if let Ok(reader) = prism_asset_package::PackageReader::from_bytes(&pak) {
        let mut assets = Vec::new();
        for record in db.records() {
            let size = reader
                .find_record(record.id)
                .map(|r| (r.size, r.compressed_size))
                .unwrap_or((0, 0));
            let compressed = size.1 > 0;
            assets.push(serde_json::json!({
                "id": format!("{:#x}", record.id.into_raw()),
                "path": record.path,
                "type": record.asset_type.label(),
                "importer": record.importer_name,
                "size": size.0,
                "compressed_size": if compressed {
                    serde_json::Value::Number(size.1.into())
                } else {
                    serde_json::Value::Null
                },
                "compression_ratio": if compressed && size.0 > 0 {
                    let r = size.1 as f64 / size.0 as f64;
                    serde_json::Value::Number(serde_json::Number::from_f64(
                        (r * 100.0).round() / 100.0
                    ).unwrap_or(serde_json::Number::from(0)))
                } else {
                    serde_json::Value::Null
                },
            }));
        }
        let manifest = serde_json::json!({
            "pak": output.file_name().map(|s| s.to_string_lossy()).unwrap_or_default(),
            "format": std::str::from_utf8(&reader.header().magic).unwrap_or("?"),
            "version": reader.header().version,
            "asset_count": reader.asset_count(),
            "total_size": pak.len(),
            "assets": assets,
        });
        let meta_json =
            serde_json::to_string_pretty(&manifest).map_err(|e| anyhow::anyhow!("{e}"))?;
        std::fs::write(&meta_path, &meta_json)?;
        println!("   📋  Manifest / 清单:  {}", meta_path.display());
    } else {
        eprintln!("   ⚠  Could not read back .pak for manifest / 无法回读 .pak");
    }

    println!(
        "✅  Build complete / 构建完成: {} cooked/已烹饪, {} skipped/跳过 → {} ({})",
        summary.cooked,
        summary.skipped,
        output.display(),
        format_bytes(pak.len() as u64),
    );
    Ok(())
}

fn cmd_validate(pak_path: &Path) -> anyhow::Result<()> {
    use prism_asset_package::PackageReader;

    let pak = std::fs::read(pak_path)?;
    let reader = PackageReader::from_bytes(&pak).map_err(|e| anyhow::anyhow!("{e}"))?;

    let magic_str = std::str::from_utf8(&reader.header().magic).unwrap_or("????");
    kv("📦  Magic", "标识", magic_str, 12);
    kv("🔢  Version", "版本", &format!("{}", reader.header().version), 12);
    kv("📊  Assets", "资源", &format!("{}", reader.asset_count()), 12);
    kv(
        "📏  Size",
        "大小",
        &format!("{} ({} bytes)", format_bytes(pak.len() as u64), pak.len()),
        12,
    );
    kv("✅  Checksum", "校验", "PASS", 12);

    // Optionally enrich with the .pak.meta.json if it 存在 alongside.
    let meta_path = pak_path.with_extension("pak.meta.json");
    let meta_assets: std::collections::HashMap<String, serde_json::Value> = (|| {
        let text = std::fs::read_to_string(&meta_path).ok()?;
        let root: serde_json::Value = serde_json::from_str(&text).ok()?;
        let arr = root.get("assets")?.as_array()?;
        let mut map = std::collections::HashMap::new();
        for entry in arr {
            if let Some(id) = entry.get("id").and_then(|i| i.as_str()) {
                map.insert(id.to_owned(), entry.clone());
            }
        }
        Some(map)
    })()
    .unwrap_or_default();

    println!();
    println!("   Assets / 资源清单:");
    println!();
    // 列 widths in display columns.
    const COL_ID: usize = 12;
    const COL_TYPE: usize = 12;
    const COL_SIZE: usize = 12;
    const COL_CSIZE: usize = 12;
    const COL_RATIO: usize = 12;
	    // Header 行 — bilingual, EN padded to 5 so `/` aligns.
	    let hdr = |en: &str, cn: &str, w: usize| pad_display(&format!("{} / {}", pad_display(en, 5), cn), w);
	    println!(
	        "  {}  {}  {}  {}  {}  {}",
	        hdr("ID", "标识", COL_ID),
	        hdr("Type", "类型", COL_TYPE),
	        hdr("Size", "大小", COL_SIZE),
	        hdr("Comp", "压缩", COL_CSIZE),
	        hdr("Ratio", "比率", COL_RATIO),
	        "Name / 名称",
	    );
    // Separator — all columns same 宽度
    println!(
        "  {}  {}  {}  {}  {}  {}",
        "-".repeat(COL_ID),
        "-".repeat(COL_TYPE),
        "-".repeat(COL_SIZE),
        "-".repeat(COL_CSIZE),
        "-".repeat(COL_RATIO),
        "-".repeat(COL_ID),
    );
    for record in reader.records() {
        let id_hex = format!("{:#x}", record.id);
        let meta = meta_assets.get(&id_hex);

        // Best-effort 标签 from metadata, or fall 后 to 类型 name + raw ID.
        let label: String = meta
            .and_then(|m| m.get("path").and_then(|p| p.as_str()))
            .map(|p| {
                Path::new(p)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(p)
                    .to_owned()
            })
            .unwrap_or_else(|| {
                let type_name = AssetType::from_u32(record.type_id).label();
                format!("({} asset {id_hex})", pad_display(type_name, 7))
            });
        let type_label = meta
            .and_then(|m| m.get("type").and_then(|t| t.as_str()))
            .unwrap_or_else(|| AssetType::from_u32(record.type_id).label());

        let ratio_str = if record.compressed_size > 0 && record.size > 0 {
            let ratio = record.compressed_size as f64 / record.size as f64;
            let pct = (ratio * 100.0).round();
            // Only show 压缩 比率 when it actually saved 空间 (< 95%).
            if pct < 95.0 {
                format!("{pct:.0}%")
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        let formatted_size = format_bytes(record.size);
        let formatted_compressed = if record.compressed_size > 0 {
            format_bytes(record.compressed_size)
        } else {
            "-".to_owned()
        };

        // All columns left-aligned using Unicode-aware 填充
        println!(
            "  {}  {}  {}  {}  {}  {}",
            pad_display(&id_hex, COL_ID),
            pad_display(type_label, COL_TYPE),
            pad_display(&formatted_size, COL_SIZE),
            pad_display(&formatted_compressed, COL_CSIZE),
            pad_display(&ratio_str, COL_RATIO),
            label,
        );
    }
    // Hint about metadata when the .meta.json was absent.
    if meta_assets.is_empty() {
        println!();
        println!("💡  Tip / 提示:");
        println!("     Rebuild with current prism-asset-cli to generate .pak.meta.json");
        println!("     重新构建以生成可读的资产清单");
    }
    Ok(())
}

fn cmd_list(root: &Path, _cli: &Cli) -> anyhow::Result<()> {
    if !db_path(root).exists() {
        anyhow::bail!("No AssetDatabase.json found / 未找到数据库. Run 'prism-asset-cli scan' first.");
    }

    let db = AssetDatabase::load(&db_path(root))?;
    println!("📋  Assets / 资源 ({} records / 条):", db.len());
    for record in db.records() {
        let type_label = format!("{:?}", record.asset_type);
        println!(
            "  {:>10}  {}  {}",
            record.id,
            pad_display(&type_label, 10),
            record.path,
        );
    }
    Ok(())
}

fn cmd_inspect(root: &Path, id_or_path: &str) -> anyhow::Result<()> {
    if !db_path(root).exists() {
        anyhow::bail!("No AssetDatabase.json found / 未找到数据库.");
    }

    let db = AssetDatabase::load(&db_path(root))?;

    // Try to parse as numeric ID 第一个
    let record = if let Ok(num) = id_or_path.parse::<u64>() {
        db.get(prism_asset_core::AssetId::from_raw(num))
    } else {
        db.get_by_path(id_or_path)
    };

    match record {
        Some(r) => {
            println!("── Asset / 资源 ──────────────────────────────────");
            kv("ID", "编号", &r.id.to_string(), 11);
            kv("Path", "路径", &r.path, 11);
            kv("Type", "类型", &format!("{:?}", r.asset_type), 11);
            kv("Importer", "导入器", &r.importer_name, 11);
            kv("State", "状态", &format!("{:?}", r.state), 11);
            kv("Version", "版本", &r.version.to_string(), 11);
            kv("Source Hash", "哈希", &format!("{:#x}", r.source_hash), 11);
            kv("Deps", "依赖", &format!("{} assets / {} 个资源", r.dependencies.len(), r.dependencies.len()), 11);
            for dep in &r.dependencies {
                if let Some(dep_record) = db.get(*dep) {
                    println!("  → {}  {}", dep, dep_record.path);
                } else {
                    println!("  → {}  (missing / 缺失)", dep);
                }
            }
        }
        None => anyhow::bail!("Asset not found / 未找到资源: {id_or_path}"),
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
        Some(Commands::Init { dir }) => cmd_init(dir.as_deref().unwrap_or(&root)),
        Some(Commands::Scan) => cmd_scan(&root, &cli),
        Some(Commands::Import { force }) => cmd_import(&root, *force, &cli),
        Some(Commands::Build {
            output,
            compression,
            platform,
        }) => cmd_build(&root, output, *compression, platform, &cli),
        Some(Commands::Validate { pak }) => cmd_validate(pak),
        Some(Commands::List) => cmd_list(&root, &cli),
        Some(Commands::Inspect { id }) => cmd_inspect(&root, id),
        None => {
            if let Some(pak) = &cli.pak {
                cmd_validate(pak)
            } else {
                // No subcommand and no .pak → show help.
                use clap::CommandFactory;
                Cli::command().print_help()?;
                println!();
                Ok(())
            }
        }
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
