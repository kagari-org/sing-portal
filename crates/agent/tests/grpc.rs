mod common;

use std::time::Duration;

use sing_portal_agent::api::BoxApi;
use sing_portal_agent::tester::{Kind, Tester, TesterConfig};
use tempfile::TempDir;

#[tokio::test]
async fn talks_to_the_sing_box_api() {
    if common::sing_box().is_none() {
        eprintln!("SING_BOX_BIN is not set, skipping");
        return;
    }

    let dir = TempDir::new().unwrap();
    let api_port = common::free_port();
    let endpoint = common::local_endpoint().await;
    let config = common::config(api_port, &format!("http://127.0.0.1:{endpoint}/"));
    let (_child, _config_dir, _state_dir) = common::start(dir.path(), api_port, &config).await;

    let api = BoxApi::new(&format!("http://127.0.0.1:{api_port}"), Duration::from_secs(5)).unwrap();

    assert!(!api.version().await.unwrap().is_empty());
    assert_eq!(api.current_selection().await.unwrap(), "s_auto");
    assert!(api.outbounds().await.unwrap().contains_key("d1"));

    api.select("s_out", "d1").await.unwrap();
    assert_eq!(api.current_selection().await.unwrap(), "d1");
}

#[tokio::test]
async fn measures_latency_of_one_outbound() {
    if common::sing_box().is_none() {
        eprintln!("SING_BOX_BIN is not set, skipping");
        return;
    }

    let dir = TempDir::new().unwrap();
    let api_port = common::free_port();
    let endpoint = common::local_endpoint().await;
    let config = common::config(api_port, &format!("http://127.0.0.1:{endpoint}/"));
    let (_child, _config_dir, _state_dir) = common::start(dir.path(), api_port, &config).await;

    let api = BoxApi::new(&format!("http://127.0.0.1:{api_port}"), Duration::from_secs(5)).unwrap();
    let tester = Tester::new(TesterConfig::default(), api);

    let measurement = tester.measure("d1", Kind::Latency).await.unwrap();
    assert_eq!(measurement.kind, "latency");
    assert!(measurement.latency_ms.unwrap() > 0.0);

    let error = tester.measure("missing", Kind::Latency).await.unwrap_err();
    assert!(error.to_string().contains("not known"), "got {error}");
}
