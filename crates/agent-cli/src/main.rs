mod host;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Args, Parser};
use host::DesktopHost;
use sing_portal_agent::{Agent, AgentConfig};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "sing-portal-agent",
    version,
    about = "keeps a sing-box instance in sync with a sing-portal machine"
)]
struct Cli {
    #[command(flatten)]
    args: AgentArgs,
}

#[derive(Args)]
struct AgentArgs {
    #[arg(long = "portal", env = "SING_PORTAL_URL")]
    portal: String,

    #[arg(long = "name", env = "SING_PORTAL_NODE")]
    node: String,

    #[arg(long, env = "SING_PORTAL_TOKEN")]
    token: Option<String>,

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

    #[arg(
        long,
        env = "SING_PORTAL_LATENCY_URL",
        default_value = sing_portal_core::model::DEFAULT_TEST_URL
    )]
    latency_url: String,

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

    /// Skip probes an automatic evaluation asks for; manual tests still run.
    #[arg(long, env = "SING_PORTAL_MANUAL_TESTS_ONLY")]
    manual_tests_only: bool,

    #[arg(long, env = "SING_PORTAL_TEST_TIMEOUT", default_value = "10s")]
    test_timeout: String,

    #[arg(
        long,
        env = "SING_PORTAL_TEST_INBOUND",
        default_value = "127.0.0.1:7899"
    )]
    test_inbound: SocketAddr,

    #[arg(
        long,
        env = "SING_PORTAL_TEST_EGRESS_INBOUND",
        default_value = "127.0.0.1:7900"
    )]
    test_egress_inbound: SocketAddr,

    #[arg(
        long,
        env = "SING_PORTAL_TEST_HEALTH_INBOUND",
        default_value = "127.0.0.1:7901"
    )]
    test_health_inbound: SocketAddr,

    #[arg(
        long,
        env = "SING_PORTAL_EGRESS_HEALTH_INTERVAL",
        default_value = "30s"
    )]
    egress_health_interval: String,

    #[arg(long, env = "SING_PORTAL_EGRESS_HEALTH_FAILURES", default_value_t = 2)]
    egress_health_failures: u32,

    #[arg(
        long,
        env = "SING_PORTAL_TEST_DOWNLOAD_URL",
        default_value = "https://speed.cloudflare.com/__down?bytes=26214400"
    )]
    test_download_url: String,

    #[arg(
        long,
        env = "SING_PORTAL_TEST_CHECK_URL",
        default_value = "https://www.cloudflare.com/cdn-cgi/trace"
    )]
    test_check_url: String,

    #[arg(long, env = "SING_PORTAL_TEST_CHECK_TIMEOUT", default_value = "8s")]
    test_check_timeout: String,

    #[arg(
        long,
        env = "SING_PORTAL_TEST_MAX_BYTES",
        default_value_t = 2 * 1024 * 1024
    )]
    test_max_bytes: u64,

    #[arg(long, env = "SING_PORTAL_API_URL")]
    api_url: String,

    #[arg(long, env = "SING_PORTAL_API_SECRET")]
    api_secret: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing();
    run_agent(cli.args).await
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

async fn run_agent(args: AgentArgs) -> Result<()> {
    let template = tokio::fs::read_to_string(&args.template)
        .await
        .with_context(|| format!("cannot read template {}", args.template.display()))?;

    let host = Arc::new(DesktopHost::new(
        args.config,
        args.state_dir,
        if args.restart_cmd.is_empty() {
            None
        } else {
            Some(args.restart_cmd)
        },
    ));

    let config = AgentConfig {
        portal_url: args.portal,
        node_name: args.node,
        token: args.token,
        template,
        api_url: args.api_url,
        api_secret: args.api_secret,
        latency_url: args.latency_url,
        interval: parse_duration(&args.interval).map_err(anyhow::Error::msg)?,
        once: args.once,
        best_effort: args.best_effort,
        auto_measure: !args.manual_tests_only,
        health_timeout: parse_duration(&args.health_timeout).map_err(anyhow::Error::msg)?,
        egress_health_interval: parse_duration(&args.egress_health_interval)
            .map_err(anyhow::Error::msg)?,
        egress_health_failures: args.egress_health_failures,
        tester: sing_portal_agent::tester::TesterConfig {
            timeout: parse_duration(&args.test_timeout).map_err(anyhow::Error::msg)?,
            inbound: args.test_inbound,
            egress_inbound: args.test_egress_inbound,
            health_inbound: args.test_health_inbound,
            download_url: args.test_download_url,
            check_url: args.test_check_url,
            check_timeout: parse_duration(&args.test_check_timeout).map_err(anyhow::Error::msg)?,
            max_bytes: args.test_max_bytes,
        },
    };

    let agent = Agent::new(config, host).context("cannot create agent")?;
    agent.run().await.context("agent failed")?;
    Ok(())
}

fn parse_duration(value: &str) -> Result<std::time::Duration, String> {
    sing_portal_core::parse_duration(value)
}
