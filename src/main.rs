//! PrismaRev engine entry point.

use std::io::{IsTerminal, Write};

fn main() -> anyhow::Result<()> {
    init_logger();
    log::info!("PrismaRev starting");
    prism_engine::App::run()?;
    log::info!("PrismaRev exited cleanly");
    Ok(())
}

/// Initialize logging. When a console is attached logs go to stderr; when
/// launched by double-click (no console) logs are written to `prismarev.log`
/// next to the executable so they don't block on a missing stderr handle.
fn init_logger() {
    let use_file = !std::io::stderr().is_terminal();
    let target = if use_file {
        let path = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("prismarev.log")))
            .unwrap_or_else(|| std::path::PathBuf::from("prismarev.log"));
        match std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
        {
            Ok(f) => env_logger::Target::Pipe(Box::new(f)),
            Err(_) => env_logger::Target::Stderr,
        }
    } else {
        env_logger::Target::Stderr
    };

    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .target(target)
        .format(|buf, record| {
            writeln!(
                buf,
                "[{} {:5} {}] {}",
                buf.timestamp_millis(),
                record.level(),
                record.target(),
                record.args()
            )
        })
        .try_init();
}
