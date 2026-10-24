mod common;

use std::io::{Read, Write};
use std::net::TcpListener as StdListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use sing_portal_agent::api::BoxApi;
use sing_portal_agent::tester::{Kind, Tester, TesterConfig};

fn serve(size: usize, initial: usize, stall: Duration) -> u16 {
    let listener = StdListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let body = Arc::new(vec![7u8; size]);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let body = body.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                let header = format!(
                    "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(&body[..initial.min(body.len())]);
                let _ = stream.flush();
                std::thread::sleep(stall);
                let _ = stream.write_all(&body[initial.min(body.len())..]);
                let _ = stream.flush();
            });
        }
    });
    port
}

fn serve_stall_first_headers(size: usize, stall: Duration) -> u16 {
    let listener = StdListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let body = Arc::new(vec![7u8; size]);
    let first = Arc::new(AtomicBool::new(true));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let body = body.clone();
            let is_first = first.swap(false, Ordering::SeqCst);
            std::thread::spawn(move || {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                if is_first {
                    std::thread::sleep(stall);
                }
                let header = format!(
                    "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(&body);
                let _ = stream.flush();
            });
        }
    });
    port
}

fn test_config(
    api_port: u16,
    inbound_port: u16,
    egress_port: u16,
    health_port: u16,
) -> serde_json::Value {
    serde_json::json!({
        "log": { "level": "warn" },
        "services": [
            { "type": "api", "tag": "s_api", "listen": "127.0.0.1", "listen_port": api_port }
        ],
        "inbounds": [
            { "type": "mixed", "tag": "s_test-in", "listen": "127.0.0.1", "listen_port": inbound_port },
            { "type": "mixed", "tag": "s_egress-in", "listen": "127.0.0.1", "listen_port": egress_port },
            { "type": "mixed", "tag": "s_health-in", "listen": "127.0.0.1", "listen_port": health_port }
        ],
        "outbounds": [
            { "type": "selector", "tag": "s_test", "outbounds": ["d1"], "default": "d1" },
            { "type": "direct", "tag": "d1" }
        ],
        "route": {
            "rules": [
                { "inbound": ["s_health-in"], "outbound": "d1" },
                { "inbound": ["s_egress-in"], "outbound": "s_test" },
                { "inbound": ["s_test-in"], "outbound": "s_test" }
            ],
            "final": "d1"
        }
    })
}

async fn tester(
    api_port: u16,
    inbound_port: u16,
    egress_port: u16,
    health_port: u16,
    http_port: u16,
    timeout: Duration,
) -> Tester {
    let api = BoxApi::new(
        &format!("http://127.0.0.1:{api_port}"),
        Duration::from_secs(5),
    )
    .unwrap();
    Tester::new(
        TesterConfig {
            timeout,
            inbound: format!("127.0.0.1:{inbound_port}").parse().unwrap(),
            egress_inbound: format!("127.0.0.1:{egress_port}").parse().unwrap(),
            health_inbound: format!("127.0.0.1:{health_port}").parse().unwrap(),
            download_url: format!("http://127.0.0.1:{http_port}/data"),
            check_url: format!("http://127.0.0.1:{http_port}/check"),
            check_timeout: Duration::from_secs(5),
            max_bytes: 2 * 1024 * 1024,
        },
        api,
    )
}

#[tokio::test]
async fn measures_download_speed() {
    if common::sing_box().is_none() {
        eprintln!("SING_BOX_BIN is not set, skipping");
        return;
    }
    let http_port = serve(4 * 1024 * 1024, usize::MAX, Duration::ZERO);
    let ports = common::free_ports(4);
    let api_port = ports[0];
    let inbound_port = ports[1];
    let egress_port = ports[2];
    let health_port = ports[3];

    let dir = tempfile::TempDir::new().unwrap();
    let (child, _config_dir, _state_dir) = common::start(
        dir.path(),
        api_port,
        &test_config(api_port, inbound_port, egress_port, health_port),
    )
    .await;
    let tester = tester(
        api_port,
        inbound_port,
        egress_port,
        health_port,
        http_port,
        Duration::from_secs(10),
    )
    .await;

    let measurement = tester.measure("d1", Kind::Bandwidth).await.unwrap();
    assert_eq!(measurement.kind, "bandwidth");
    assert!(measurement.down_mbps.unwrap() > 0.0);
    drop(child);
}

#[tokio::test]
async fn uses_partial_download_on_timeout() {
    if common::sing_box().is_none() {
        eprintln!("SING_BOX_BIN is not set, skipping");
        return;
    }
    // 256 KiB arrive immediately, the rest only after the probe deadline
    let http_port = serve(4 * 1024 * 1024, 256 * 1024, Duration::from_secs(3));
    let ports = common::free_ports(4);
    let api_port = ports[0];
    let inbound_port = ports[1];
    let egress_port = ports[2];
    let health_port = ports[3];

    let dir = tempfile::TempDir::new().unwrap();
    let (child, _config_dir, _state_dir) = common::start(
        dir.path(),
        api_port,
        &test_config(api_port, inbound_port, egress_port, health_port),
    )
    .await;
    let tester = tester(
        api_port,
        inbound_port,
        egress_port,
        health_port,
        http_port,
        Duration::from_millis(800),
    )
    .await;

    let measurement = tester.measure("d1", Kind::Bandwidth).await.unwrap();
    let down = measurement.down_mbps.unwrap();
    assert!(down > 0.0);
    drop(child);
}

#[tokio::test]
async fn download_timeout_releases_the_selector_lock() {
    if common::sing_box().is_none() {
        eprintln!("SING_BOX_BIN is not set, skipping");
        return;
    }
    // The first request never sends headers until after the probe deadline.
    let http_port = serve_stall_first_headers(4 * 1024 * 1024, Duration::from_secs(2));
    let ports = common::free_ports(4);
    let api_port = ports[0];
    let inbound_port = ports[1];
    let egress_port = ports[2];
    let health_port = ports[3];

    let dir = tempfile::TempDir::new().unwrap();
    let (child, _config_dir, _state_dir) = common::start(
        dir.path(),
        api_port,
        &test_config(api_port, inbound_port, egress_port, health_port),
    )
    .await;
    let tester = tester(
        api_port,
        inbound_port,
        egress_port,
        health_port,
        http_port,
        Duration::from_millis(300),
    )
    .await;

    let started = Instant::now();
    let first = tester.measure("d1", Kind::Bandwidth).await;
    assert!(first.is_err(), "a stalled response must time out");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the download timeout must be bounded"
    );

    let second = tester.measure("d1", Kind::Bandwidth).await.unwrap();
    assert!(second.down_mbps.unwrap() > 0.0);
    drop(child);
}

#[tokio::test]
async fn checks_egress_through_the_real_inbound() {
    if common::sing_box().is_none() {
        eprintln!("SING_BOX_BIN is not set, skipping");
        return;
    }
    let http_port = serve(1024, 1024, Duration::ZERO);
    let ports = common::free_ports(4);
    let api_port = ports[0];
    let inbound_port = ports[1];
    let egress_port = ports[2];
    let health_port = ports[3];

    let dir = tempfile::TempDir::new().unwrap();
    let (child, _config_dir, _state_dir) = common::start(
        dir.path(),
        api_port,
        &test_config(api_port, inbound_port, egress_port, health_port),
    )
    .await;
    let tester = tester(
        api_port,
        inbound_port,
        egress_port,
        health_port,
        http_port,
        Duration::from_secs(10),
    )
    .await;

    let measurement = tester.measure("d1", Kind::Egress).await.unwrap();
    assert_eq!(measurement.kind, "egress");
    assert!(measurement.latency_ms.unwrap() >= 0.0);
    drop(child);
}

#[tokio::test]
async fn checks_the_real_egress_health_path() {
    if common::sing_box().is_none() {
        eprintln!("SING_BOX_BIN is not set, skipping");
        return;
    }
    let http_port = serve(1024, 1024, Duration::ZERO);
    let ports = common::free_ports(4);
    let api_port = ports[0];
    let inbound_port = ports[1];
    let egress_port = ports[2];
    let health_port = ports[3];

    let dir = tempfile::TempDir::new().unwrap();
    let (child, _config_dir, _state_dir) = common::start(
        dir.path(),
        api_port,
        &test_config(api_port, inbound_port, egress_port, health_port),
    )
    .await;
    let tester = tester(
        api_port,
        inbound_port,
        egress_port,
        health_port,
        http_port,
        Duration::from_secs(10),
    )
    .await;

    let latency = tester.health_check().await.unwrap();
    assert!(latency >= 0.0);
    drop(child);
}
