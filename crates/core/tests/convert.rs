use serde_json::{Value, json};
use sing_portal_core::canonical::{normalize, revision, selection_revision, slugify, unique_slug};
use sing_portal_core::detect::{Format, detect, parse_subscription};
use sing_portal_core::model::{TAG_SEPARATOR, outbound_tag};
use sing_portal_core::render::{AUTO_TAG, DIRECT_TAG, SELECTOR_TAG, config};
use sing_portal_core::uri;

const SINGBOX: &str = include_str!("../../../fixtures/sub-singbox.json");
const CLASH: &str = include_str!("../../../fixtures/sub-clash.yaml");
const URIS: &str = include_str!("../../../fixtures/sub-uri.txt");
const URIS_B64: &str = include_str!("../../../fixtures/sub-uri.base64.txt");

fn by_tag<'a>(outbounds: &'a [Value], tag: &str) -> &'a Value {
    outbounds
        .iter()
        .find(|outbound| outbound_tag(outbound) == Some(tag))
        .unwrap_or_else(|| panic!("missing tag {tag}"))
}

#[test]
fn detects_formats() {
    assert_eq!(detect(SINGBOX.as_bytes()).unwrap(), Format::SingBoxJson);
    assert_eq!(detect(CLASH.as_bytes()).unwrap(), Format::ClashYaml);
    assert_eq!(detect(URIS.as_bytes()).unwrap(), Format::Uri);
    assert_eq!(detect(URIS_B64.as_bytes()).unwrap(), Format::UriBase64);
}

#[test]
fn rejects_html_and_empty() {
    assert!(detect(b"<!doctype html><html></html>").is_err());
    assert!(detect(b"   \n").is_err());
    assert!(detect(b"\x00\x01\x02").is_err());
}

#[test]
fn sing_box_json_is_parsed() {
    let parsed = parse_subscription(SINGBOX.as_bytes()).unwrap();
    assert_eq!(parsed.format, Format::SingBoxJson);
    assert_eq!(parsed.outbounds.len(), 4);

    let normalized = normalize(parsed.outbounds, "jc").unwrap();
    assert_eq!(normalized.len(), 2);
    let tags: Vec<&str> = normalized
        .iter()
        .filter_map(outbound_tag)
        .collect();
    assert_eq!(tags, vec!["jc · JP 02", "jc · US 01"]);
    assert_eq!(
        by_tag(&normalized, "jc · US 01")["flow"],
        Value::from("xtls-rprx-vision")
    );
}

#[test]
fn clash_yaml_is_parsed() {
    let parsed = parse_subscription(CLASH.as_bytes()).unwrap();
    assert_eq!(parsed.format, Format::ClashYaml);
    assert_eq!(parsed.outbounds.len(), 4);
    assert_eq!(parsed.warnings.len(), 1);
    assert!(parsed.warnings[0].contains("wireguard"));

    let outbounds = normalize(parsed.outbounds, "cl").unwrap();

    let ss = by_tag(&outbounds, "cl · SS-NODE");
    assert_eq!(ss["type"], "shadowsocks");
    assert_eq!(ss["method"], "aes-128-gcm");
    assert_eq!(ss["plugin"], "obfs");
    assert_eq!(ss["plugin_opts"], "obfs=http;obfs-host=bing.com");

    let vmess = by_tag(&outbounds, "cl · VMESS-WS");
    assert_eq!(vmess["transport"]["type"], "ws");
    assert_eq!(vmess["transport"]["path"], "/ray");
    assert_eq!(vmess["transport"]["headers"]["Host"], "cdn.example.com");
    assert_eq!(vmess["tls"]["server_name"], "cdn.example.com");

    let trojan = by_tag(&outbounds, "cl · TROJAN-H2");
    assert_eq!(trojan["transport"]["type"], "http");
    assert_eq!(trojan["transport"]["path"], "/tunnel");
    assert_eq!(trojan["transport"]["host"], json!(["trojan.example.com"]));
    assert_eq!(trojan["tls"]["insecure"], true);
    assert_eq!(trojan["tls"]["alpn"], json!(["h2", "http/1.1"]));

    let vless = by_tag(&outbounds, "cl · VLESS-REALITY");
    assert_eq!(vless["transport"]["type"], "grpc");
    assert_eq!(vless["transport"]["service_name"], "grpcsvc");
    assert_eq!(vless["tls"]["reality"]["public_key"], "PUBKEY");
    assert_eq!(vless["tls"]["reality"]["short_id"], "abcd");
    assert_eq!(vless["tls"]["utls"]["fingerprint"], "chrome");
}

#[test]
fn uri_list_is_parsed() {
    let parsed = parse_subscription(URIS.as_bytes()).unwrap();
    assert_eq!(parsed.format, Format::Uri);
    assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
    assert_eq!(parsed.outbounds.len(), 10);

    let vmess = by_tag(&parsed.outbounds, "VMESS WS");
    assert_eq!(vmess["type"], "vmess");
    assert_eq!(vmess["uuid"], "b831381d-6324-4d53-ad4f-8cda48b30811");
    assert_eq!(vmess["security"], "auto");
    assert_eq!(vmess["alter_id"], 0);
    assert_eq!(vmess["server_port"], 443);
    assert_eq!(vmess["transport"]["type"], "ws");
    assert_eq!(vmess["transport"]["path"], "/ray");
    assert_eq!(vmess["transport"]["headers"]["Host"], "cdn.example.com");
    assert_eq!(vmess["tls"]["server_name"], "cdn.example.com");
    assert_eq!(vmess["tls"]["alpn"], json!(["h2", "http/1.1"]));
    assert_eq!(vmess["tls"]["utls"]["fingerprint"], "chrome");

    let vless = by_tag(&parsed.outbounds, "VLESS REALITY");
    assert_eq!(vless["type"], "vless");
    assert_eq!(vless["flow"], "xtls-rprx-vision");
    assert_eq!(vless["transport"]["type"], "grpc");
    assert_eq!(vless["transport"]["service_name"], "grpcsvc");
    assert_eq!(vless["tls"]["reality"]["public_key"], "PUBKEY");
    assert_eq!(vless["tls"]["reality"]["short_id"], "abcd");
    assert_eq!(vless["tls"]["server_name"], "vless.example.com");

    let ss = by_tag(&parsed.outbounds, "SS OBFS");
    assert_eq!(ss["type"], "shadowsocks");
    assert_eq!(ss["method"], "aes-256-gcm");
    assert_eq!(ss["password"], "password");
    assert_eq!(ss["plugin"], "obfs-local");
    assert_eq!(ss["plugin_opts"], "obfs=http;obfs-host=example.com");

    let legacy = by_tag(&parsed.outbounds, "SS LEGACY");
    assert_eq!(legacy["server"], "ss2.example.com");
    assert_eq!(legacy["server_port"], 8389);
    assert_eq!(legacy["password"], "password");

    let trojan = by_tag(&parsed.outbounds, "TROJAN WS");
    assert_eq!(trojan["password"], "pa$$word");
    assert_eq!(trojan["transport"]["type"], "ws");
    assert_eq!(trojan["transport"]["path"], "/tj");
    assert_eq!(trojan["tls"]["insecure"], true);

    let hy2 = by_tag(&parsed.outbounds, "HY2");
    assert_eq!(hy2["type"], "hysteria2");
    assert_eq!(hy2["obfs"]["type"], "salamander");
    assert_eq!(hy2["obfs"]["password"], "obfspw");
    assert_eq!(hy2["tls"]["insecure"], true);

    let tuic = by_tag(&parsed.outbounds, "TUIC");
    assert_eq!(tuic["type"], "tuic");
    assert_eq!(tuic["password"], "tuicpass");
    assert_eq!(tuic["congestion_control"], "bbr");
    assert_eq!(tuic["udp_relay_mode"], "native");
    assert_eq!(tuic["tls"]["alpn"], json!(["h3"]));

    let anytls = by_tag(&parsed.outbounds, "ANYTLS");
    assert_eq!(anytls["type"], "anytls");
    assert_eq!(anytls["password"], "anypass");

    let socks = by_tag(&parsed.outbounds, "SOCKS");
    assert_eq!(socks["type"], "socks");
    assert_eq!(socks["version"], "5");
    assert_eq!(socks["username"], "sockuser");

    let http = by_tag(&parsed.outbounds, "HTTP");
    assert_eq!(http["type"], "http");
    assert_eq!(http["username"], "httpuser");
    assert!(http.get("tls").is_none());
}

#[test]
fn base64_uri_list_is_parsed() {
    let parsed = parse_subscription(URIS_B64.as_bytes()).unwrap();
    assert_eq!(parsed.format, Format::UriBase64);
    assert_eq!(parsed.outbounds.len(), 3);
}

#[test]
fn unsupported_schemes_are_reported() {
    let error =
        uri::parse_lines("naive+https://user@example.com:443#NAIVE\nvmess://not-base64\n").unwrap_err();
    let message = error.to_string();
    assert!(message.contains("naive"), "{message}");
    assert!(message.contains("vmess"), "{message}");
}

#[test]
fn normalize_prefixes_dedupes_and_sorts() {
    let outbounds = vec![
        json!({"type":"vless","tag":"B","server":"b.example.com","server_port":443,"uuid":"u"}),
        json!({"type":"selector","tag":"group","outbounds":["B"]}),
        json!({"type":"vless","tag":"B","server":"b2.example.com","server_port":443,"uuid":"u"}),
        json!({"tag":"no-type"}),
    ];

    let normalized = normalize(outbounds, "sub").unwrap();
    let tags: Vec<String> = normalized.iter().filter_map(outbound_tag).map(ToString::to_string).collect();
    assert_eq!(
        tags,
        vec![
            format!("sub{TAG_SEPARATOR}B"),
            format!("sub{TAG_SEPARATOR}B#2"),
        ]
    );
    assert_eq!(normalized[1]["server"], "b2.example.com");
}

#[test]
fn normalize_rejects_empty_and_oversized() {
    assert!(normalize(vec![json!({"type":"direct","tag":"direct"})], "sub").is_err());

    let oversized: Vec<Value> = (0..1001)
        .map(|index| {
            json!({"type":"vless","tag":format!("n{index}"),"server":"h","server_port":443,"uuid":"u"})
        })
        .collect();
    assert!(normalize(oversized, "sub").is_err());
}

#[test]
fn revision_is_stable_and_order_independent() {
    let first = json!({"type":"vless","tag":"a","server":"h1","server_port":443,"uuid":"u"});
    let second = json!({"type":"vless","tag":"b","server":"h2","server_port":443,"uuid":"u"});

    let forward = revision(&[first.clone(), second.clone()]);
    let backward = revision(&[second.clone(), first.clone()]);
    assert_eq!(forward, backward);
    assert!(forward.starts_with("sha256:"));
    assert_eq!(revision(&[]), "none");

    let changed = json!({"type":"vless","tag":"a","server":"h9","server_port":443,"uuid":"u"});
    assert_ne!(forward, revision(&[changed, second]));
}

#[test]
fn selection_revision_depends_on_node_and_selection() {
    let a = selection_revision("home", "s_auto");
    assert_eq!(a, selection_revision("home", "s_auto"));
    assert_ne!(a, selection_revision("cola", "s_auto"));
    assert_ne!(a, selection_revision("home", "s_direct"));
}

#[test]
fn slugify_and_unique_slug() {
    assert_eq!(slugify("Hong Kong 01"), "hong-kong-01");
    assert_eq!(slugify("Provider A"), "provider-a");
    assert_eq!(slugify("!!!"), "sub");
    assert_eq!(slugify("a-very-long-subscription-name"), "a-very-long-subs");

    let taken = vec!["sub".to_string(), "sub-2".to_string()];
    assert_eq!(unique_slug("other", &taken), "other");
    assert_eq!(unique_slug("sub", &taken), "sub-3");
}

#[test]
fn render_builds_the_outbounds_config() {
    let outbounds = vec![
        json!({"type":"vless","tag":"n1","server":"h","server_port":443,"uuid":"u"}),
        json!({"type":"vless","tag":"n2","server":"h","server_port":443,"uuid":"u"}),
    ];

    let rendered = config(&outbounds);
    let rendered_outbounds = rendered["outbounds"].as_array().unwrap();
    assert_eq!(rendered_outbounds.len(), 5);
    assert_eq!(rendered_outbounds[0]["type"], "selector");
    assert_eq!(rendered_outbounds[0]["tag"], SELECTOR_TAG);
    assert_eq!(rendered_outbounds[0]["default"], AUTO_TAG);
    assert_eq!(
        rendered_outbounds[0]["outbounds"],
        json!([AUTO_TAG, DIRECT_TAG, "n1", "n2"])
    );
    assert_eq!(rendered_outbounds[1]["type"], "urltest");
    assert_eq!(rendered_outbounds[1]["tag"], AUTO_TAG);
    assert_eq!(rendered_outbounds[2]["tag"], DIRECT_TAG);
}

#[test]
fn render_without_nodes_falls_back_to_direct() {
    let rendered = config(&[]);
    let outbounds = rendered["outbounds"].as_array().unwrap();
    assert_eq!(outbounds.len(), 2);
    assert_eq!(outbounds[0]["default"], DIRECT_TAG);
    assert_eq!(outbounds[0]["outbounds"], json!([DIRECT_TAG]));
    assert_eq!(outbounds[1]["tag"], DIRECT_TAG);
}
