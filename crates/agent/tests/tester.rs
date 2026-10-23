use std::path::PathBuf;
use std::time::Duration;

use serde_json::json;
use sing_portal_agent::tester::{Kind, Tester, TesterConfig, parse_network_quality};
use sing_portal_core::clash_api::ClashClient;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn script(dir: &TempDir, body: &str) -> PathBuf {
    let path = dir.path().join("sing-box");
    std::fs::write(&path, body).unwrap();
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    std::fs::set_permissions(&path, permissions).unwrap();
    path
}

fn tester(dir: &TempDir, script: PathBuf, api_url: Option<&str>) -> Tester {
    Tester::new(
        TesterConfig {
            sing_box: script,
            api_url: api_url.map(ToString::to_string),
            network_quality_runtime: 2,
            ..Default::default()
        },
        ClashClient::new("http://127.0.0.1:1", Duration::from_secs(2)).unwrap(),
    )
    .unwrap()
}

#[test]
fn parses_network_quality_summary() {
    let output = "==== NETWORK QUALITY TEST ====\rIdle Latency: 114 ms\r\n\
                  Download Capacity: 12.5 Mbps Accuracy: Medium\r\n\
                  Download Capacity: 88.0 Mbps Accuracy: High\r\n\
                  Upload Capacity: 42.0 Mbps Accuracy: High\r\n\
                  Download Responsiveness: 600 RPM Accuracy: Medium\r\n\
                  Upload Responsiveness: 520 RPM Accuracy: Low\r\n";
    let report = parse_network_quality(output);
    assert_eq!(report.idle_latency_ms, Some(114.0));
    assert_eq!(report.download_mbps, Some(88.0), "last reading wins");
    assert_eq!(report.upload_mbps, Some(42.0));
    assert_eq!(report.download_rpm, Some(600.0));
    assert_eq!(report.upload_rpm, Some(520.0));
}

#[tokio::test]
async fn bandwidth_fails_without_the_api() {
    let dir = TempDir::new().unwrap();
    let script = script(&dir, "#!/bin/sh\nexit 0\n");
    let tester = tester(&dir, script, None);

    let error = tester
        .measure("node-a", Kind::Bandwidth, None)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("api is not configured"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn bandwidth_runs_inside_the_daemon() {
    let dir = TempDir::new().unwrap();
    let script = script(
        &dir,
        r#"#!/bin/sh
case "$1 $2" in
  "api networkquality")
    cat <<EOF
Idle Latency: 37 ms
Download Capacity: 88.0 Mbps
Upload Capacity: 42.0 Mbps
Download Responsiveness: 600 RPM
EOF
    ;;
  *)
    echo "unexpected args: $@" >&2
    exit 1
    ;;
esac
"#,
    );

    let clash = MockServer::start().await;
    let tester = Tester::new(
        TesterConfig {
            sing_box: script,
            api_url: Some(clash.uri()),
            network_quality_runtime: 2,
            ..Default::default()
        },
        ClashClient::new(&clash.uri(), Duration::from_secs(2)).unwrap(),
    )
    .unwrap();

    let measurement = tester
        .measure("node-a", Kind::Bandwidth, None)
        .await
        .unwrap();
    assert_eq!(measurement.kind, "bandwidth");
    assert_eq!(measurement.down_mbps, Some(88.0));
    assert_eq!(measurement.up_mbps, Some(42.0));
    assert_eq!(measurement.latency_ms, Some(37.0));
    assert_eq!(measurement.responsiveness, Some(600.0));
}

#[tokio::test]
async fn latency_uses_the_clash_delay_api() {
    let dir = TempDir::new().unwrap();
    let script = script(&dir, "#!/bin/sh\nexit 0\n");
    let clash = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/proxies/node-a/delay"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "delay": 37 })))
        .mount(&clash)
        .await;

    let tester = Tester::new(
        TesterConfig {
            sing_box: script,
            ..Default::default()
        },
        ClashClient::new(&clash.uri(), Duration::from_secs(2)).unwrap(),
    )
    .unwrap();

    let measurement = tester.measure("node-a", Kind::Latency, None).await.unwrap();
    assert_eq!(measurement.kind, "latency");
    assert_eq!(measurement.latency_ms, Some(37.0));
    assert!(measurement.down_mbps.is_none());
}
