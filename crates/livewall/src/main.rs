//! livewall wallpaper daemon (`livewall`).

mod app;
mod ipc_server;
mod media;
mod metrics;
mod power;
mod wayland;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::EnvFilter;
use livewall_core::load_config;

use crate::app::App;

#[derive(Parser, Debug)]
#[command(name = "livewall", about = "Efficient Wayland video/image wallpaper daemon")]
struct Args {
    /// Override config path
    #[arg(long)]
    config: Option<PathBuf>,
    /// Initial wallpaper for all outputs
    #[arg(long)]
    path: Option<PathBuf>,
    /// Hardware decode preference
    #[arg(long, default_value = "auto")]
    hwdec: String,
    /// Refuse software decode fallback
    #[arg(long)]
    efficiency_mode: bool,
    /// Log verbose Wayland / decode details
    #[arg(long, short)]
    verbose: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let filter = if args.verbose {
        EnvFilter::new("livewall=debug,info")
    } else {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("livewall=info,warn"))
    };
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let mut cfg = if let Some(p) = &args.config {
        let text = std::fs::read_to_string(p)?;
        toml::from_str(&text)?
    } else {
        load_config().unwrap_or_default()
    };

    if let Some(pref) = livewall_core::HwDecPreference::parse(&args.hwdec) {
        cfg.hwdec = pref;
    }
    if args.efficiency_mode {
        cfg.efficiency_mode = true;
    }
    if let Some(path) = args.path {
        cfg.default_path = Some(path);
    }

    tracing::info!(
        hwdec = cfg.hwdec.as_str(),
        efficiency = cfg.efficiency_mode,
        "starting livewall"
    );

    let cfg = Arc::new(std::sync::RwLock::new(cfg));
    App::run(cfg)
}
