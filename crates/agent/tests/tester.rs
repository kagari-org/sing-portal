mod common;

use std::io::{Read, Write};
use std::net::TcpListener as StdListener;
use std::sync::Arc;
use std::time::Duration;

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

fn test_config(api_port: u16, inbound_port: u16) -> serde_json::Value {
    serde_json::json!({
        "log": { "level": "warn" },
        "services": [
            { "type": "api", "tag": "s_api", "listen": "127.0.0.1", "listen_port": api_port }
        ],
        "inbounds": [
            { "type": "mixed", "tag": "s_test-in", "listen": "127.0.0.1", "listen_port": inbound_port }
        ],
        "outbounds": [
            { "type": "selector", "tag": "s_test", "outbounds": ["d1"], "default": "d1" },
            { "type": "direct", "tag": "d1" }
        ],
        "route": {
            "rules": [ { "inbound": ["s_test-in"], "outbound": "s_test" } ],
            "final": "d1"
        }
    })
}

async fn tester(api_port: u16, inbound_port: u16, http_port: u16, timeout: Duration) -> Tester {
    let api = BoxApi::new(
        &format!("http://127.0.0.1:{api_port}"),
        Duration::from_secs(5),
    )
    .unwrap();
    Tester::new(
        TesterConfig {
            timeout,
            inbound: format!("127.0.0.1:{inbound_port}").parse().unwrap(),
            download_url: format!("http://127.0.0.1:{http_port}/data"),
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
    let api_port = common::free_port();
    let inbound_port = common::free_port();
    let http_port = serve(4 * 1024 * 1024, usize::MAX, Duration::ZERO);

    let dir = tempfile::TempDir::new().unwrap();
    let (child, _config_dir, _state_dir) =
        common::start(dir.path(), api_port, &test_config(api_port, inbound_port)).await;
    let tester = tester(api_port, inbound_port, http_port, Duration::from_secs(10)).await;

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
    let api_port = common::free_port();
    let inbound_port = common::free_port();
    // 256 KiB arrive immediately, the rest only after the probe deadline
    let http_port = serve(4 * 1024 * 1024, 256 * 1024, Duration::from_secs(3));

    let dir = tempfile::TempDir::new().unwrap();
    let (child, _config_dir, _state_dir) =
        common::start(dir.path(), api_port, &test_config(api_port, inbound_port)).await;
    let tester = tester(api_port, inbound_port, http_port, Duration::from_millis(800)).await;

    let measurement = tester.measure("d1", Kind::Bandwidth).await.unwrap();
    let down = measurement.down_mbps.unwrap();
    assert!(down > 0.0);
    drop(child);
}
