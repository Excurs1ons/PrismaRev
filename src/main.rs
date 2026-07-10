//! PrismaRev engine entry point.

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    log::info!("PrismaRev starting");
    prism_engine::App::run()?;
    log::info!("PrismaRev exited cleanly");
    Ok(())
}
