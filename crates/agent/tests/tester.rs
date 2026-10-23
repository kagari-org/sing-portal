use std::path::{Path, PathBuf};
use std::time::Duration;

use sing_portal_agent::api::BoxApi;
use sing_portal_agent::tester::{Kind, Tester, TesterConfig};
use tempfile::TempDir;

fn script(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join("sing-box");
    std::fs::write(&path, body).unwrap();
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    std::fs::set_permissions(&path, permissions).unwrap();
    path
}

fn tester(script: PathBuf) -> Tester {
    Tester::new(
        TesterConfig {
            timeout: Duration::from_secs(3),
            network_quality_runtime: 2,
            network_quality_config_url: None,
        },
        BoxApi::new(script, "http://127.0.0.1:1", Duration::from_secs(5)),
    )
}

#[tokio::test]
async fn bandwidth_reports_capacities() {
    let dir = TempDir::new().unwrap();
    let script = script(
        &dir.path(),
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
    exit 1
    ;;
esac
"#,
    );

    let measurement = tester(script)
        .measure("node-a", Kind::Bandwidth)
        .await
        .unwrap();
    assert_eq!(measurement.kind, "bandwidth");
    assert_eq!(measurement.down_mbps, Some(88.0));
    assert_eq!(measurement.up_mbps, Some(42.0));
    assert_eq!(measurement.latency_ms, Some(37.0));
    assert_eq!(measurement.responsiveness, Some(600.0));
}

#[tokio::test]
async fn latency_reads_the_url_test_result() {
    let dir = TempDir::new().unwrap();
    let state = dir.path().to_path_buf();
    let script = script(
        &dir.path(),
        &format!(
            r#"#!/bin/sh
case "$1 $2 $3" in
  "api group urltest") touch {state}/tested ;;
  "api outbounds"*)
    if [ -f {state}/tested ]; then
      printf 'node-a\tdirect\t42 ms\n'
    else
      printf 'node-a\tdirect\t-\n'
    fi
    ;;
esac
"#,
            state = state.display()
        ),
    );

    let measurement = tester(script)
        .measure("node-a", Kind::Latency)
        .await
        .unwrap();
    assert_eq!(measurement.kind, "latency");
    assert_eq!(measurement.latency_ms, Some(42.0));
    assert!(measurement.down_mbps.is_none());
}

#[tokio::test]
async fn latency_rejects_tags_outside_the_group() {
    let dir = TempDir::new().unwrap();
    let script = script(
        &dir.path(),
        r#"#!/bin/sh
if [ "$1 $2" = "api outbounds" ]; then
  printf 'other\tdirect\t42 ms\n'
fi
"#,
    );

    let error = tester(script)
        .measure("node-a", Kind::Latency)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("not part of"), "got {error}");
}

#[tokio::test]
async fn latency_reports_a_failed_url_test() {
    let dir = TempDir::new().unwrap();
    let state = dir.path().to_path_buf();
    let script = script(
        &dir.path(),
        &format!(
            r#"#!/bin/sh
case "$1 $2 $3" in
  "api group urltest") touch {state}/tested ;;
  "api outbounds"*)
    if [ -f {state}/tested ]; then
      printf 'node-a\tdirect\t-\nnode-b\tdirect\t10 ms\n'
    else
      printf 'node-a\tdirect\t20 ms\nnode-b\tdirect\t10 ms\n'
    fi
    ;;
esac
"#,
            state = state.display()
        ),
    );

    let error = tester(script)
        .measure("node-a", Kind::Latency)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("url test failed"), "got {error}");
}
