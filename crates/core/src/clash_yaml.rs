use serde_json::{Map, Value, json};
use serde_yaml_ng::Value as Yaml;

use crate::error::Error;

pub fn parse(content: &str) -> Result<(Vec<Value>, Vec<String>), Error> {
    let document: Yaml =
        serde_yaml_ng::from_str(content).map_err(|err| Error::Yaml(err.to_string()))?;
    let proxies = document
        .get("proxies")
        .and_then(Yaml::as_sequence)
        .ok_or_else(|| Error::Yaml("no proxies key".into()))?;

    let mut outbounds = Vec::new();
    let mut warnings = Vec::new();

    for proxy in proxies {
        match convert(proxy) {
            Ok(Some(outbound)) => outbounds.push(outbound),
            Ok(None) => {}
            Err(err) => warnings.push(err.to_string()),
        }
    }

    if outbounds.is_empty() {
        return Err(Error::Yaml("no supported proxy in proxies list".into()));
    }

    Ok((outbounds, warnings))
}

fn convert(proxy: &Yaml) -> Result<Option<Value>, Error> {
    let proxy = proxy
        .as_mapping()
        .ok_or_else(|| Error::Yaml("proxy entry is not a mapping".into()))?;

    let name = string(proxy, "name").ok_or_else(|| Error::Yaml("proxy has no name".into()))?;
    let server =
        string(proxy, "server").ok_or_else(|| Error::Yaml("proxy has no server".into()))?;
    let port = integer(proxy, "port").ok_or_else(|| Error::Yaml("proxy has no port".into()))?;
    let kind = match string(proxy, "type") {
        Some(kind) => kind,
        None => return Err(Error::Yaml("proxy has no type".into())),
    };

    let mut map = Map::new();
    map.insert("tag".into(), json!(name));
    map.insert("server".into(), json!(server));
    map.insert("server_port".into(), json!(port));

    match kind {
        "ss" => {
            map.insert("type".into(), json!("shadowsocks"));
            map.insert(
                "method".into(),
                json!(string(proxy, "cipher").unwrap_or("aes-128-gcm")),
            );
            map.insert(
                "password".into(),
                json!(string(proxy, "password").unwrap_or_default()),
            );
            apply_ss_plugin(proxy, &mut map);
        }
        "vmess" => {
            map.insert("type".into(), json!("vmess"));
            map.insert(
                "uuid".into(),
                json!(string(proxy, "uuid").unwrap_or_default()),
            );
            map.insert(
                "alter_id".into(),
                json!(integer(proxy, "alterId").unwrap_or(0)),
            );
            map.insert(
                "security".into(),
                json!(string(proxy, "cipher").unwrap_or("auto")),
            );
            if let Some(transport) = transport(proxy) {
                map.insert("transport".into(), transport);
            }
            if truthy(proxy, "tls") || reality(proxy) {
                map.insert("tls".into(), tls(proxy, server));
            }
        }
        "vless" => {
            map.insert("type".into(), json!("vless"));
            map.insert(
                "uuid".into(),
                json!(string(proxy, "uuid").unwrap_or_default()),
            );
            if let Some(flow) = string(proxy, "flow").filter(|flow| !flow.is_empty()) {
                map.insert("flow".into(), json!(flow));
            }
            if let Some(transport) = transport(proxy) {
                map.insert("transport".into(), transport);
            }
            if truthy(proxy, "tls") || reality(proxy) {
                map.insert("tls".into(), tls(proxy, server));
            }
        }
        "trojan" => {
            map.insert("type".into(), json!("trojan"));
            map.insert(
                "password".into(),
                json!(string(proxy, "password").unwrap_or_default()),
            );
            if let Some(transport) = transport(proxy) {
                map.insert("transport".into(), transport);
            }
            map.insert("tls".into(), tls(proxy, server));
        }
        "hysteria2" | "hy2" => {
            map.insert("type".into(), json!("hysteria2"));
            map.insert(
                "password".into(),
                json!(string(proxy, "password")
                    .or_else(|| string(proxy, "auth"))
                    .unwrap_or_default()),
            );
            if let (Some(obfs), Some(password)) = (
                string(proxy, "obfs").filter(|obfs| *obfs != "none"),
                string(proxy, "obfs-password"),
            ) {
                map.insert(
                    "obfs".into(),
                    json!({ "type": obfs, "password": password }),
                );
            }
            map.insert("tls".into(), tls(proxy, server));
        }
        "tuic" => {
            map.insert("type".into(), json!("tuic"));
            map.insert(
                "uuid".into(),
                json!(string(proxy, "uuid").unwrap_or_default()),
            );
            map.insert(
                "password".into(),
                json!(string(proxy, "password").unwrap_or_default()),
            );
            if let Some(congestion) =
                string(proxy, "congestion-controller").or_else(|| string(proxy, "congestion_control"))
            {
                map.insert("congestion_control".into(), json!(congestion));
            }
            if let Some(mode) =
                string(proxy, "udp-relay-mode").or_else(|| string(proxy, "udp_relay_mode"))
            {
                map.insert("udp_relay_mode".into(), json!(mode));
            }
            map.insert("tls".into(), tls(proxy, server));
        }
        "anytls" => {
            map.insert("type".into(), json!("anytls"));
            map.insert(
                "password".into(),
                json!(string(proxy, "password").unwrap_or_default()),
            );
            map.insert("tls".into(), tls(proxy, server));
        }
        "socks5" | "socks" => {
            map.insert("type".into(), json!("socks"));
            map.insert("version".into(), json!("5"));
            if let Some(username) = string(proxy, "username") {
                map.insert("username".into(), json!(username));
                map.insert(
                    "password".into(),
                    json!(string(proxy, "password").unwrap_or_default()),
                );
            }
        }
        "http" | "https" => {
            map.insert("type".into(), json!("http"));
            if let Some(username) = string(proxy, "username") {
                map.insert("username".into(), json!(username));
                map.insert(
                    "password".into(),
                    json!(string(proxy, "password").unwrap_or_default()),
                );
            }
            if kind == "https" || truthy(proxy, "tls") {
                map.insert("tls".into(), tls(proxy, server));
            }
        }
        other => return Err(Error::Yaml(format!("unsupported proxy type: {other}"))),
    };

    Ok(Some(Value::Object(map)))
}


fn apply_ss_plugin(proxy: &serde_yaml_ng::Mapping, map: &mut Map<String, Value>) {
    let Some(plugin) = string(proxy, "plugin") else {
        return;
    };
    let options = proxy
        .get(Yaml::String("plugin-opts".into()))
        .and_then(Yaml::as_mapping);

    let mut parts = Vec::new();
    if let Some(options) = options {
        if let Some(mode) = string(options, "mode") {
            parts.push(format!("obfs={mode}"));
        }
        if let Some(host) = string(options, "host") {
            parts.push(format!("obfs-host={host}"));
        }
    }

    map.insert("plugin".into(), json!(plugin));
    if !parts.is_empty() {
        map.insert("plugin_opts".into(), json!(parts.join(";")));
    }
}

fn transport(proxy: &serde_yaml_ng::Mapping) -> Option<Value> {
    match string(proxy, "network")? {
        "ws" => {
            let opts = proxy
                .get(Yaml::String("ws-opts".into()))
                .and_then(Yaml::as_mapping);
            let mut map = Map::new();
            map.insert("type".into(), json!("ws"));
            let path = opts
                .and_then(|opts| string(opts, "path"))
                .unwrap_or("/");
            map.insert("path".into(), json!(path));
            let host = opts
                .and_then(|opts| opts.get(Yaml::String("headers".into())))
                .and_then(Yaml::as_mapping)
                .and_then(|headers| string(headers, "Host").or_else(|| string(headers, "host")));
            if let Some(host) = host {
                map.insert("headers".into(), json!({ "Host": host }));
            }
            Some(Value::Object(map))
        }
        "grpc" => {
            let opts = proxy
                .get(Yaml::String("grpc-opts".into()))
                .and_then(Yaml::as_mapping);
            let service_name = opts
                .and_then(|opts| string(opts, "grpc-service-name"))
                .unwrap_or_default();
            Some(json!({ "type": "grpc", "service_name": service_name }))
        }
        "h2" => {
            let opts = proxy
                .get(Yaml::String("h2-opts".into()))
                .and_then(Yaml::as_mapping);
            let mut map = Map::new();
            map.insert("type".into(), json!("http"));
            if let Some(opts) = opts {
                map.insert("path".into(), json!(string(opts, "path").unwrap_or("/")));
                if let Some(hosts) = string_list(opts, "host") {
                    map.insert("host".into(), json!(hosts));
                }
            }
            Some(Value::Object(map))
        }
        _ => None,
    }
}

fn reality(proxy: &serde_yaml_ng::Mapping) -> bool {
    proxy
        .get(Yaml::String("reality-opts".into()))
        .and_then(Yaml::as_mapping)
        .is_some()
}

fn tls(proxy: &serde_yaml_ng::Mapping, server: &str) -> Value {
    let mut map = Map::new();
    map.insert("enabled".into(), json!(true));

    let server_name = string(proxy, "servername")
        .or_else(|| string(proxy, "sni"))
        .unwrap_or(server);
    if !server_name.is_empty() {
        map.insert("server_name".into(), json!(server_name));
    }
    if truthy(proxy, "skip-cert-verify") {
        map.insert("insecure".into(), json!(true));
    }
    if let Some(alpn) = string_list(proxy, "alpn") {
        map.insert("alpn".into(), json!(alpn));
    }
    if let Some(fingerprint) = string(proxy, "client-fingerprint") {
        map.insert(
            "utls".into(),
            json!({ "enabled": true, "fingerprint": fingerprint }),
        );
    }
    if let Some(reality) = proxy
        .get(Yaml::String("reality-opts".into()))
        .and_then(Yaml::as_mapping)
    {
        map.insert(
            "reality".into(),
            json!({
                "enabled": true,
                "public_key": string(reality, "public-key").unwrap_or_default(),
                "short_id": string(reality, "short-id").unwrap_or_default(),
            }),
        );
    }

    Value::Object(map)
}

fn get<'a>(map: &'a serde_yaml_ng::Mapping, key: &str) -> Option<&'a Yaml> {
    map.get(Yaml::String(key.to_string()))
}

fn string<'a>(map: &'a serde_yaml_ng::Mapping, key: &str) -> Option<&'a str> {
    get(map, key).and_then(Yaml::as_str)
}

fn integer(map: &serde_yaml_ng::Mapping, key: &str) -> Option<u64> {
    get(map, key).and_then(|value| {
        value
            .as_u64()
            .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
    })
}

fn truthy(map: &serde_yaml_ng::Mapping, key: &str) -> bool {
    get(map, key)
        .map(|value| value.as_bool().unwrap_or(false))
        .unwrap_or(false)
}

fn string_list(map: &serde_yaml_ng::Mapping, key: &str) -> Option<Vec<String>> {
    get(map, key).and_then(|value| match value {
        Yaml::Sequence(items) => Some(
            items
                .iter()
                .filter_map(Yaml::as_str)
                .map(ToString::to_string)
                .collect(),
        ),
        Yaml::String(single) => Some(vec![single.clone()]),
        _ => None,
    })
}
