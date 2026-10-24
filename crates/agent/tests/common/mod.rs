#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::process::{Child, Command};

pub fn sing_box() -> Option<PathBuf> {
    std::env::var_os("SING_BOX_BIN")
        .map(PathBuf::from)
        .filter(|path| path.exists())
}

pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Allocate several distinct ports at once; separate `free_port` calls can
/// return the same number because the listener is dropped immediately.
pub fn free_ports(count: usize) -> Vec<u16> {
    let listeners: Vec<std::net::TcpListener> = (0..count)
        .map(|_| std::net::TcpListener::bind("127.0.0.1:0").unwrap())
        .collect();
    listeners
        .iter()
        .map(|listener| listener.local_addr().unwrap().port())
        .collect()
}

pub async fn wait_port(port: u16) {
    for _ in 0..200 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("sing-box did not start listening on {port}");
}

pub fn config(api_port: u16, test_url: &str) -> Value {
    json!({
        "log": { "level": "warn" },
        "services": [
            { "type": "api", "tag": "s_api", "listen": "127.0.0.1", "listen_port": api_port }
        ],
        "outbounds": [
            { "type": "selector", "tag": "s_out", "outbounds": ["s_auto", "d1"], "default": "s_auto" },
            { "type": "urltest", "tag": "s_auto", "outbounds": ["d1"], "url": test_url, "interval": "10m" },
            { "type": "direct", "tag": "d1" }
        ],
        "route": { "final": "s_out" }
    })
}

pub async fn start(root: &Path, api_port: u16, config: &Value) -> (Child, PathBuf, PathBuf) {
    let config_dir = root.join("config");
    let state_dir = root.join("state");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::write(
        config_dir.join("config.json"),
        serde_json::to_vec(config).unwrap(),
    )
    .unwrap();

    let log = std::fs::File::create(root.join("sing-box.log")).unwrap();
    let child = Command::new(sing_box().expect("SING_BOX_BIN"))
        .arg("run")
        .arg("-D")
        .arg(&state_dir)
        .arg("-C")
        .arg(&config_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::from(log))
        .kill_on_drop(true)
        .spawn()
        .unwrap();

    wait_port(api_port).await;

    // make sure the api service really answers before the test can reload
    let api = sing_portal_agent::api::BoxApi::new(
        &format!("http://127.0.0.1:{api_port}"),
        Duration::from_secs(5),
    )
    .unwrap();
    for _ in 0..100 {
        if api.version().await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    (child, config_dir, state_dir)
}

pub async fn local_endpoint() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buffer = [0u8; 1024];
                let _ = socket.read(&mut buffer).await;
                let _ = socket
                    .write_all(
                        b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await;
            });
        }
    });
    port
}
