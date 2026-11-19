use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, Parser};
use sing_portal_server::{AppState, ServerConfig, parse_duration};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "sing-portal-server",
    version,
    about = "sing-box subscription portal"
)]
struct Cli {
    #[command(flatten)]
    args: ServeArgs,
}

#[derive(Args)]
struct ServeArgs {
    #[arg(long, env = "SING_PORTAL_LISTEN", default_value = "127.0.0.1:3015")]
    listen: SocketAddr,

    #[arg(long, env = "SING_PORTAL_DB", default_value = "portal.db")]
    db: PathBuf,

    #[arg(long, env = "SING_PORTAL_FETCH_PROXY")]
    fetch_proxy: Option<String>,

    #[arg(long, env = "SING_PORTAL_REFRESH_DEFAULT", default_value = "6h")]
    refresh_default: String,

    #[arg(long, env = "SING_PORTAL_FETCH_TIMEOUT", default_value = "20s")]
    fetch_timeout: String,

    #[arg(
        long,
        env = "SING_PORTAL_FETCH_MAX_BYTES",
        default_value_t = 8 * 1024 * 1024
    )]
    fetch_max_bytes: usize,

    #[arg(long, env = "SING_PORTAL_STATIC_DIR")]
    static_dir: Option<PathBuf>,

    #[arg(long, env = "SING_PORTAL_AGENT_LISTEN")]
    agent_listen: Option<SocketAddr>,

    #[arg(long, env = "SING_PORTAL_AGENT_TOKEN")]
    agent_token: Option<String>,

    #[arg(long, env = "SING_PORTAL_AUTO_INTERVAL", default_value = "2h")]
    auto_interval: String,

    #[arg(long, env = "SING_PORTAL_AUTO_CANDIDATES", default_value_t = 6)]
    auto_candidates: usize,

    #[arg(long, env = "SING_PORTAL_AUTO_MARGIN", default_value_t = 0.15)]
    auto_margin: f64,

    #[arg(long, env = "SING_PORTAL_AUTO_LAT_REF", default_value_t = 60.0)]
    auto_lat_ref: f64,

    #[arg(long, env = "SING_PORTAL_AUTO_BW_REF", default_value_t = 20.0)]
    auto_bw_ref: f64,

    #[arg(long, env = "SING_PORTAL_AUTO_BW_FLOOR", default_value_t = 0.5)]
    auto_bw_floor: f64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing();
    serve(cli.args).await
}

fn init_tracing() {
    let filter = EnvFilter::try_from_env("SING_PORTAL_LOG")
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

async fn serve(args: ServeArgs) -> Result<()> {
    let config = ServerConfig {
        listen: args.listen,
        db_path: args.db,
        fetch_proxy: args.fetch_proxy,
        refresh_default: parse_duration(&args.refresh_default).map_err(anyhow::Error::msg)?,
        fetch_timeout: parse_duration(&args.fetch_timeout).map_err(anyhow::Error::msg)?,
        fetch_max_bytes: args.fetch_max_bytes,
        static_dir: args.static_dir,
        embed_web: true,
        agent_listen: args.agent_listen,
        agent_token: args.agent_token,
        auto_interval: parse_duration(&args.auto_interval).map_err(anyhow::Error::msg)?,
        auto_candidates: args.auto_candidates,
        auto_margin: args.auto_margin,
        score: sing_portal_core::score::ScoreConfig {
            latency_ref_ms: args.auto_lat_ref,
            bandwidth_ref_mbps: args.auto_bw_ref,
            bandwidth_floor: args.auto_bw_floor,
        },
    };

    let state = AppState::new(config)
        .await
        .context("cannot open portal database")?;

    let scheduler = tokio::spawn(sing_portal_server::scheduler::run(state.clone()));
    let auto = tokio::spawn(sing_portal_server::auto::run(state.clone()));
    let result = sing_portal_server::serve(state).await;
    auto.abort();
    scheduler.abort();
    result.context("portal server failed")?;
    Ok(())
}
