use std::time::Duration;

use serde_json::json;
use sing_portal_agent::tester::{Kind, Tester, TesterConfig, down_mbps};
use sing_portal_core::clash_api::ClashClient;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[test]
fn bandwidth_math() {
    assert_eq!(down_mbps(0, 1.0), 0.0);
    assert_eq!(down_mbps(1_000_000, 0.0), 0.0);
    assert!((down_mbps(1_250_000, 1.0) - 10.0).abs() < f64::EPSILON);
}

#[tokio::test]
async fn measures_download_through_inbound_and_restores_selection() {
    let proxy = StubProxy::start(1024 * 1024).await;

    let clash = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/proxies/s_out"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "now": "previous" })))
        .mount(&clash)
        .await;
    Mock::given(method("PUT"))
        .and(path("/proxies/s_out"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&clash)
        .await;

    let tester = Tester::new(
        TesterConfig {
            proxy_inbound: proxy.url(),
            latency_url: "http://probe.invalid/".to_string(),
            download_url: "http://speed.invalid/payload".to_string(),
            max_bytes: 1024 * 1024,
            timeout: Duration::from_secs(5),
        },
        ClashClient::new(&clash.uri(), Duration::from_secs(5)).unwrap(),
    )
    .unwrap();

    let measurement = tester.measure("node-a", Kind::Bandwidth, None).await.unwrap();

    assert_eq!(measurement.kind, "bandwidth");
    assert_eq!(measurement.tag, "node-a");
    assert_eq!(measurement.bytes, Some(1024 * 1024));
    assert!(measurement.down_mbps.unwrap() > 0.0);
    assert_eq!(measurement.restored, "previous");

    let puts: Vec<_> = clash
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| request.method.as_str() == "PUT")
        .collect();
    assert_eq!(puts.len(), 2, "switch to the target, then restore");
}

struct StubProxy {
    address: std::net::SocketAddr,
}

impl StubProxy {
    async fn start(body_size: usize) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buffer = [0u8; 2048];
                    let _ = socket.read(&mut buffer).await;
                    let body = vec![b'x'; body_size];
                    let head = format!(
                        "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.write_all(&body).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        Self { address }
    }

    fn url(&self) -> String {
        format!("http://{}", self.address)
    }
}
