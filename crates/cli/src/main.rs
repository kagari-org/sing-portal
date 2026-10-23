use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use sing_portal_agent::AgentConfig;
use sing_portal_server::{AppState, ServerConfig, parse_duration};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "sing-portal",
    version,
    about = "sing-box subscription portal and node agent"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Serve(ServeArgs),
    Agent(AgentArgs),
}

#[derive(Args)]
struct ServeArgs {
    #[arg(long, env = "SING_PORTAL_LISTEN", default_value = "127.0.0.1:3015")]
    listen: SocketAddr,

    #[arg(long, env = "SING_PORTAL_DB", default_value = "portal.db")]
    db: PathBuf,

    #[arg(long, env = "SING_PORTAL_SING_BOX")]
    sing_box: Option<PathBuf>,

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

    #[arg(long, env = "SING_PORTAL_AUTO_INTERVAL", default_value = "10m")]
    auto_interval: String,

    #[arg(long, env = "SING_PORTAL_AUTO_CANDIDATES", default_value_t = 5)]
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

#[derive(Args)]
struct AgentArgs {
    #[arg(long = "portal", env = "SING_PORTAL_URL")]
    portal: String,

    #[arg(long = "name", env = "SING_PORTAL_NODE")]
    node: String,

    #[arg(long, env = "SING_PORTAL_TEMPLATE")]
    template: PathBuf,

    #[arg(
        long,
        env = "SING_PORTAL_CONFIG",
        default_value = "/run/sing-box/config.json"
    )]
    config: PathBuf,

    #[arg(
        long,
        env = "SING_PORTAL_STATE_DIR",
        default_value = "/var/lib/sing-box"
    )]
    state_dir: PathBuf,

    #[arg(long, env = "SING_PORTAL_SING_BOX", default_value = "sing-box")]
    sing_box: String,

    #[arg(
        long,
        env = "SING_PORTAL_RESTART_CMD",
        default_value = "systemctl restart sing-box"
    )]
    restart_cmd: String,

    #[arg(long, env = "SING_PORTAL_INTERVAL", default_value = "15s")]
    interval: String,

    #[arg(long, env = "SING_PORTAL_HEALTH_TIMEOUT", default_value = "10s")]
    health_timeout: String,

    #[arg(long)]
    once: bool,

    #[arg(long = "best-effort")]
    best_effort: bool,

    #[arg(long, env = "SING_PORTAL_TEST_TIMEOUT", default_value = "20s")]
    test_timeout: String,

    #[arg(
        long,
        env = "SING_PORTAL_TEST_NQ_RUNTIME",
        default_value_t = 8
    )]
    test_nq_runtime: u64,

    #[arg(long, env = "SING_PORTAL_API_URL")]
    api_url: String,

    #[arg(long, env = "SING_PORTAL_TEST_NQ_CONFIG_URL")]
    test_nq_config_url: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing();

    match cli.command {
        Command::Serve(args) => serve(args).await,
        Command::Agent(args) => run_agent(args).await,
    }
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
        sing_box: args.sing_box,
        fetch_proxy: args.fetch_proxy,
        refresh_default: parse_duration(&args.refresh_default).map_err(anyhow::Error::msg)?,
        fetch_timeout: parse_duration(&args.fetch_timeout).map_err(anyhow::Error::msg)?,
        fetch_max_bytes: args.fetch_max_bytes,
        static_dir: args.static_dir,
        embed_web: true,
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

async fn run_agent(args: AgentArgs) -> Result<()> {
    let config = AgentConfig {
        portal_url: args.portal,
        node_name: args.node,
        template_path: args.template,
        config_path: args.config,
        state_dir: args.state_dir.clone(),
        sing_box_bin: args.sing_box.clone(),
        api_url: args.api_url,
        restart_cmd: if args.restart_cmd.is_empty() {
            None
        } else {
            Some(args.restart_cmd)
        },
        interval: parse_duration(&args.interval).map_err(anyhow::Error::msg)?,
        once: args.once,
        best_effort: args.best_effort,
        health_timeout: parse_duration(&args.health_timeout).map_err(anyhow::Error::msg)?,
        tester: sing_portal_agent::tester::TesterConfig {
            timeout: parse_duration(&args.test_timeout).map_err(anyhow::Error::msg)?,
            network_quality_runtime: args.test_nq_runtime,
            network_quality_config_url: args.test_nq_config_url,
        },
    };

    let agent = sing_portal_agent::Agent::new(config).context("cannot create agent")?;
    agent.run().await.context("agent failed")?;
    Ok(())
}
