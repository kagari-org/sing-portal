use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;
use sing_portal_agent::tester::{Kind, Tester, TesterConfig, mbps, parse_network_quality};
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

fn state_dir(dir: &TempDir) -> PathBuf {
    let path = dir.path().join("state");
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(
        path.join("outbounds.json"),
        serde_json::to_vec(&json!([{ "type": "vless", "tag": "node-a", "server": "example.com" }]))
            .unwrap(),
    )
    .unwrap();
    path
}

fn tester(dir: &TempDir, script: &Path, clash: &MockServer) -> Tester {
    Tester::new(
        TesterConfig {
            sing_box: script.to_path_buf(),
            state_dir: state_dir(dir),
            max_bytes: 1024 * 1024,
            timeout: Duration::from_secs(5),
            network_quality_runtime: 2,
            ..Default::default()
        },
        ClashClient::new(&clash.uri(), Duration::from_secs(5)).unwrap(),
    )
    .unwrap()
}

#[test]
fn bandwidth_math() {
    assert_eq!(mbps(0, 1.0), 0.0);
    assert_eq!(mbps(1_000_000, 0.0), 0.0);
    assert!((mbps(1_250_000, 1.0) - 10.0).abs() < f64::EPSILON);
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
async fn throughput_reads_reported_bytes() {
    let dir = TempDir::new().unwrap();
    let script = script(&dir, "#!/bin/sh\nhead -c 524288 /dev/zero\n");
    let clash = MockServer::start().await;
    let tester = tester(&dir, &script, &clash);

    let measurement = tester
        .measure("node-a", Kind::Throughput, None)
        .await
        .unwrap();

    assert_eq!(measurement.kind, "throughput");
    assert_eq!(measurement.bytes, Some(524288));
    assert!(measurement.down_mbps.unwrap() > 0.0);
    assert!(measurement.latency_ms.is_none());
}

#[tokio::test]
async fn network_quality_reports_capacities() {
    let dir = TempDir::new().unwrap();
    let script = script(
        &dir,
        "#!/bin/sh\ncat <<'EOF'\nIdle Latency: 37 ms\n\
Download Capacity: 88.0 Mbps Accuracy: High\n\
Upload Capacity: 42.0 Mbps Accuracy: High\n\
Download Responsiveness: 600 RPM Accuracy: Medium\nEOF\n",
    );
    let clash = MockServer::start().await;
    let tester = tester(&dir, &script, &clash);

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
async fn network_quality_prefers_the_sing_box_api() {
    let dir = TempDir::new().unwrap();
    let script = script(
        &dir,
        r#"#!/bin/sh
case "$1 $2" in
  "api networkquality")
    cat <<EOF
Download Capacity: 64.0 Mbps
Upload Capacity: 32.0 Mbps
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
            state_dir: state_dir(&dir),
            network_quality_runtime: 2,
            api_url: Some("http://127.0.0.1:19091".into()),
            ..Default::default()
        },
        ClashClient::new(&clash.uri(), Duration::from_secs(5)).unwrap(),
    )
    .unwrap();

    let measurement = tester
        .measure("node-a", Kind::Bandwidth, None)
        .await
        .unwrap();
    assert_eq!(measurement.down_mbps, Some(64.0));
    assert_eq!(measurement.up_mbps, Some(32.0));
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

    let tester = tester(&dir, &script, &clash);
    let measurement = tester.measure("node-a", Kind::Latency, None).await.unwrap();

    assert_eq!(measurement.kind, "latency");
    assert_eq!(measurement.latency_ms, Some(37.0));
    assert!(measurement.down_mbps.is_none());
}
