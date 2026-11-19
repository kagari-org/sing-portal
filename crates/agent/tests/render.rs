use std::time::Duration;

use serde_json::{Value, json};
use sing_portal_agent::AgentConfig;
use sing_portal_agent::apply::{render_bytes, template_fingerprint};
use tempfile::TempDir;

fn config(template: &Value) -> AgentConfig {
    AgentConfig {
        portal_url: String::new(),
        node_name: "home".into(),
        token: None,
        template: serde_json::to_string(template).unwrap(),
        api_url: "http://127.0.0.1:1".into(),
        latency_url: "https://cp.cloudflare.com/generate_204".into(),
        interval: Duration::from_secs(15),
        once: true,
        best_effort: false,
        health_timeout: Duration::from_secs(1),
        egress_health_interval: Duration::ZERO,
        egress_health_failures: 2,
        tester: sing_portal_agent::tester::TesterConfig::default(),
    }
}

#[test]
fn renders_test_and_egress_probe_rules() {
    let template = json!({
        "log": { "level": "warn" },
        "dns": {
            "servers": [
                { "tag": "s_cf", "type": "https", "server": "1.0.0.1", "detour": "s_out" },
                { "tag": "s_local", "type": "udp", "server": "223.5.5.5" }
            ],
            "final": "s_cf"
        },
        "route": {
            "final": "s_out",
            "default_domain_resolver": "s_local",
            "rules": [ { "action": "sniff" } ]
        }
    });
    let agent = config(&template);
    let outbounds = json!([
        { "type": "socks", "tag": "node-a", "server": "127.0.0.1", "server_port": 1 }
    ]);

    let rendered: Value =
        serde_json::from_slice(&render_bytes(&agent, outbounds.as_array().unwrap()).unwrap())
            .unwrap();

    let inbound_tags: Vec<&str> = rendered["inbounds"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|inbound| inbound.get("tag").and_then(Value::as_str))
        .collect();
    assert!(inbound_tags.contains(&"s_test-in"));
    assert!(inbound_tags.contains(&"s_egress-in"));
    assert!(inbound_tags.contains(&"s_health-in"));

    let dns_servers = rendered["dns"]["servers"].as_array().unwrap();
    let probe_dns = dns_servers
        .iter()
        .find(|server| server["tag"] == "s_probe-dns")
        .expect("probe dns server");
    assert_eq!(probe_dns["type"], "https");
    assert_eq!(probe_dns["server"], "1.0.0.1");
    assert_eq!(probe_dns["detour"], "s_test");

    let rules = rendered["route"]["rules"].as_array().unwrap();
    assert_eq!(rules[0]["action"], "resolve");
    assert_eq!(rules[0]["server"], "s_local");
    assert_eq!(rules[0]["inbound"][0], "s_test-in");
    assert_eq!(rules[1]["action"], "resolve");
    assert_eq!(rules[1]["server"], "s_probe-dns");
    assert_eq!(rules[1]["inbound"][0], "s_egress-in");
    assert_eq!(rules[2]["action"], "resolve");
    assert!(rules[2].get("server").is_none());
    assert_eq!(rules[2]["inbound"][0], "s_health-in");
    assert_eq!(rules[3]["outbound"], "s_test");
    assert_eq!(rules[3]["inbound"][0], "s_test-in");
    assert_eq!(rules[4]["outbound"], "s_test");
    assert_eq!(rules[4]["inbound"][0], "s_egress-in");
    assert_eq!(rules[5]["outbound"], "s_out");
    assert_eq!(rules[5]["inbound"][0], "s_health-in");

    let selectors: Vec<&str> = rendered["outbounds"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|outbound| outbound.get("tag").and_then(Value::as_str))
        .collect();
    assert!(selectors.contains(&"s_test"));

    if let Some(bin) = std::env::var_os("SING_BOX_BIN").filter(|path| !path.is_empty()) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("rendered.json");
        std::fs::write(&path, serde_json::to_vec(&rendered).unwrap()).unwrap();
        let output = std::process::Command::new(bin)
            .arg("check")
            .arg("-c")
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "sing-box rejected the rendered config: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn template_fingerprint_tracks_template_content() {
    let agent = config(&json!({ "log": { "level": "warn" } }));

    let first = template_fingerprint(&agent);
    assert_eq!(first, template_fingerprint(&agent));

    let mut changed = agent.clone();
    changed.template = r#"{"log":{"level":"info"}}"#.into();
    assert_ne!(first, template_fingerprint(&changed));
}
