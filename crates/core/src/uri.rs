use base64::Engine;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
use percent_encoding::percent_decode_str;
use serde_json::{Map, Value, json};
use url::Url;

use crate::error::Error;

pub fn decode_base64(input: &str) -> Option<String> {
    let compact: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    if compact.is_empty() {
        return None;
    }
    for engine in [
        &STANDARD,
        &STANDARD_NO_PAD,
        &URL_SAFE,
        &URL_SAFE_NO_PAD,
    ] {
        if let Ok(bytes) = engine.decode(compact.as_bytes()) {
            if let Ok(text) = String::from_utf8(bytes) {
                return Some(text);
            }
        }
    }
    None
}

fn percent_decode(input: &str) -> String {
    percent_decode_str(input).decode_utf8_lossy().into_owned()
}

pub fn parse_lines(text: &str) -> Result<(Vec<Value>, Vec<String>), Error> {
    let mut outbounds = Vec::new();
    let mut warnings = Vec::new();

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }
        match parse_one(line) {
            Ok(outbound) => outbounds.push(outbound),
            Err(err) => warnings.push(err.to_string()),
        }
    }

    if outbounds.is_empty() {
        let detail = if warnings.is_empty() {
            "no URI found".to_string()
        } else {
            warnings.join("; ")
        };
        return Err(Error::Uri(detail));
    }

    Ok((outbounds, warnings))
}

pub fn parse_one(line: &str) -> Result<Value, Error> {
    let scheme = line
        .split_once("://")
        .map(|(scheme, _)| scheme.to_ascii_lowercase())
        .ok_or_else(|| Error::Uri("missing scheme".into()))?;

    match scheme.as_str() {
        "vmess" => parse_vmess(line),
        "vless" => parse_vless(line),
        "ss" => parse_ss(line),
        "trojan" => parse_trojan(line),
        "hysteria2" | "hy2" => parse_hysteria2(line),
        "tuic" => parse_tuic(line),
        "anytls" => parse_anytls(line),
        "socks" | "socks5" => parse_socks(line),
        "http" | "https" => parse_http(line),
        other => Err(Error::UnsupportedScheme(other.to_string())),
    }
}

#[derive(Debug, Default)]
struct Params(Vec<(String, String)>);

impl Params {
    fn empty() -> Self {
        Self(Vec::new())
    }

    fn from_url(url: &Url) -> Self {
        Self(
            url.query_pairs()
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect(),
        )
    }

    fn from_query(query: &str) -> Self {
        let query = query.trim_start_matches('?');
        let mut pairs = Vec::new();
        for pair in query.split('&') {
            if pair.is_empty() {
                continue;
            }
            let (key, value) = match pair.split_once('=') {
                Some((key, value)) => (key, value),
                None => (pair, ""),
            };
            pairs.push((percent_decode(key), percent_decode(value)));
        }
        Self(pairs)
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    fn get_any(&self, keys: &[&str]) -> Option<&str> {
        keys.iter().find_map(|key| self.get(key))
    }

    fn flag(&self, keys: &[&str]) -> bool {
        matches!(self.get_any(keys), Some("1" | "true" | "True" | "yes"))
    }
}

fn host_of(url: &Url) -> Result<String, Error> {
    url.host_str()
        .filter(|host| !host.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| Error::Uri("missing host".into()))
}

fn port_of(url: &Url) -> Result<u16, Error> {
    url.port()
        .ok_or_else(|| Error::Uri("missing port".into()))
}

fn tag_of(url: &Url) -> String {
    match url.fragment() {
        Some(fragment) if !fragment.is_empty() => percent_decode(fragment),
        _ => format!("{}:{}", url.host_str().unwrap_or_default(), url.port().unwrap_or(0)),
    }
}

fn tag_from_fragment(fragment: Option<&str>, default: String) -> String {
    match fragment {
        Some(fragment) if !fragment.is_empty() => percent_decode(fragment),
        _ => default,
    }
}

fn transport_from(params: &Params, default_host: &str) -> Option<Value> {
    let kind = params.get("type").unwrap_or("tcp");
    match kind {
        "ws" => {
            let mut map = Map::new();
            map.insert("type".into(), json!("ws"));
            map.insert(
                "path".into(),
                json!(params.get("path").unwrap_or("/")),
            );
            let host = params.get("host").unwrap_or(default_host);
            if !host.is_empty() {
                map.insert("headers".into(), json!({ "Host": host }));
            }
            Some(Value::Object(map))
        }
        "grpc" => {
            let mut map = Map::new();
            map.insert("type".into(), json!("grpc"));
            map.insert(
                "service_name".into(),
                json!(params
                    .get_any(&["serviceName", "service_name"])
                    .unwrap_or_default()),
            );
            Some(Value::Object(map))
        }
        "http" | "h2" => {
            let mut map = Map::new();
            map.insert("type".into(), json!("http"));
            map.insert("path".into(), json!(params.get("path").unwrap_or("/")));
            let host = params.get("host").unwrap_or(default_host);
            if !host.is_empty() {
                map.insert("host".into(), json!([host]));
            }
            Some(Value::Object(map))
        }
        _ => None,
    }
}

fn tls_from(params: &Params, default_server_name: &str, forced: bool) -> Option<Value> {
    let security = params
        .get("security")
        .unwrap_or(if forced { "tls" } else { "none" });

    match security {
        "tls" => {
            let mut map = Map::new();
            map.insert("enabled".into(), json!(true));
            let server_name = params
                .get_any(&["sni", "peer", "server_name"])
                .unwrap_or(default_server_name);
            if !server_name.is_empty() {
                map.insert("server_name".into(), json!(server_name));
            }
            if let Some(alpn) = params.get("alpn") {
                map.insert("alpn".into(), json!(split_list(alpn)));
            }
            if params.flag(&["insecure", "allowInsecure", "allow_insecure"]) {
                map.insert("insecure".into(), json!(true));
            }
            if let Some(fingerprint) = params.get("fp") {
                map.insert(
                    "utls".into(),
                    json!({ "enabled": true, "fingerprint": fingerprint }),
                );
            }
            Some(Value::Object(map))
        }
        "reality" => {
            let mut map = Map::new();
            map.insert("enabled".into(), json!(true));
            if let Some(server_name) = params.get_any(&["sni", "server_name"]) {
                map.insert("server_name".into(), json!(server_name));
            }
            map.insert(
                "utls".into(),
                json!({
                    "enabled": true,
                    "fingerprint": params.get("fp").unwrap_or("chrome"),
                }),
            );
            let mut reality = Map::new();
            reality.insert("enabled".into(), json!(true));
            if let Some(public_key) = params.get_any(&["pbk", "public_key"]) {
                reality.insert("public_key".into(), json!(public_key));
            }
            if let Some(short_id) = params.get_any(&["sid", "short_id"]) {
                reality.insert("short_id".into(), json!(short_id));
            }
            map.insert("reality".into(), Value::Object(reality));
            Some(Value::Object(map))
        }
        _ => None,
    }
}

fn split_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

fn parse_vmess(line: &str) -> Result<Value, Error> {
    let body = line.trim_start_matches("vmess://");
    let payload = body.split('#').next().unwrap_or(body);
    let decoded = decode_base64(payload)
        .ok_or_else(|| Error::Uri("vmess payload is not valid base64".into()))?;
    let doc: Value =
        serde_json::from_str(&decoded).map_err(|_| Error::Uri("vmess payload is not JSON".into()))?;

    let server = doc
        .get("add")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::Uri("vmess payload has no server".into()))?;
    let port = doc
        .get("port")
        .and_then(as_u64)
        .ok_or_else(|| Error::Uri("vmess payload has no port".into()))?;
    let uuid = doc
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::Uri("vmess payload has no id".into()))?;

    let tag = doc
        .get("ps")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| format!("{server}:{port}"));

    let mut map = Map::new();
    map.insert("type".into(), json!("vmess"));
    map.insert("tag".into(), json!(tag));
    map.insert("server".into(), json!(server));
    map.insert("server_port".into(), json!(port));
    map.insert("uuid".into(), json!(uuid));
    map.insert(
        "security".into(),
        json!(doc.get("scy").and_then(Value::as_str).unwrap_or("auto")),
    );
    map.insert(
        "alter_id".into(),
        json!(doc.get("aid").and_then(as_u64).unwrap_or(0)),
    );

    let network = doc.get("net").and_then(Value::as_str).unwrap_or("tcp");
    let host = doc.get("host").and_then(Value::as_str).unwrap_or_default();
    let path = doc.get("path").and_then(Value::as_str).unwrap_or_default();

    let transport = match network {
        "ws" => Some(json!({
            "type": "ws",
            "path": if path.is_empty() { "/" } else { path },
            "headers": { "Host": host },
        })),
        "grpc" => Some(json!({
            "type": "grpc",
            "service_name": path,
        })),
        "h2" | "http" => Some(json!({
            "type": "http",
            "path": if path.is_empty() { "/" } else { path },
            "host": if host.is_empty() { Vec::new() } else { vec![host.to_string()] },
        })),
        _ => None,
    };
    if let Some(transport) = transport {
        map.insert("transport".into(), transport);
    }

    if doc.get("tls").and_then(Value::as_str) == Some("tls") {
        let mut tls = Map::new();
        tls.insert("enabled".into(), json!(true));
        let server_name = doc
            .get("sni")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or(host);
        if !server_name.is_empty() {
            tls.insert("server_name".into(), json!(server_name));
        }
        if let Some(alpn) = doc.get("alpn").and_then(Value::as_str) {
            let list = split_list(alpn);
            if !list.is_empty() {
                tls.insert("alpn".into(), json!(list));
            }
        }
        if let Some(fingerprint) = doc.get("fp").and_then(Value::as_str) {
            tls.insert(
                "utls".into(),
                json!({ "enabled": true, "fingerprint": fingerprint }),
            );
        }
        map.insert("tls".into(), Value::Object(tls));
    }

    Ok(Value::Object(map))
}

fn parse_vless(line: &str) -> Result<Value, Error> {
    let url = Url::parse(line).map_err(|err| Error::Uri(err.to_string()))?;
    let uuid = percent_decode(url.username());
    if uuid.is_empty() {
        return Err(Error::Uri("vless URI has no uuid".into()));
    }

    let params = Params::from_url(&url);
    let server = host_of(&url)?;
    let port = port_of(&url)?;

    let mut map = Map::new();
    map.insert("type".into(), json!("vless"));
    map.insert("tag".into(), json!(tag_of(&url)));
    map.insert("server".into(), json!(server));
    map.insert("server_port".into(), json!(port));
    map.insert("uuid".into(), json!(uuid));
    if let Some(flow) = params.get("flow").filter(|flow| !flow.is_empty()) {
        map.insert("flow".into(), json!(flow));
    }
    if let Some(transport) = transport_from(&params, &server) {
        map.insert("transport".into(), transport);
    }
    if let Some(tls) = tls_from(&params, &server, false) {
        map.insert("tls".into(), tls);
    }

    Ok(Value::Object(map))
}

fn parse_ss(line: &str) -> Result<Value, Error> {
    let body = line.trim_start_matches("ss://");
    let (payload, fragment) = match body.split_once('#') {
        Some((payload, fragment)) => (payload, Some(fragment)),
        None => (body, None),
    };

    let (userinfo, hostport) = match payload.rsplit_once('@') {
        Some((userinfo, hostport)) => (Some(userinfo), hostport),
        None => (None, payload),
    };

    let (method, password, host, port, params) = if let Some(userinfo) = userinfo {
        let credentials = decode_base64(userinfo)
            .filter(|decoded| decoded.contains(':'))
            .unwrap_or_else(|| percent_decode(userinfo));
        let (method, password) = credentials
            .split_once(':')
            .ok_or_else(|| Error::Uri("ss userinfo must be method:password".into()))?;
        let (hostport, query) = match hostport.split_once('?') {
            Some((hostport, query)) => (hostport, query),
            None => (hostport, ""),
        };
        let (host, port) = parse_hostport(hostport)?;
        (
            method.to_string(),
            password.to_string(),
            host,
            port,
            Params::from_query(query),
        )
    } else {
        let decoded = decode_base64(hostport)
            .ok_or_else(|| Error::Uri("ss payload is not valid base64".into()))?;
        let (credentials, hostport) = decoded
            .rsplit_once('@')
            .ok_or_else(|| Error::Uri("ss payload has no host".into()))?;
        let (method, password) = credentials
            .split_once(':')
            .ok_or_else(|| Error::Uri("ss credentials must be method:password".into()))?;
        let (host, port) = parse_hostport(hostport)?;
        (method.to_string(), password.to_string(), host, port, Params::empty())
    };

    let mut map = Map::new();
    map.insert("type".into(), json!("shadowsocks"));
    map.insert(
        "tag".into(),
        json!(tag_from_fragment(fragment, format!("{host}:{port}"))),
    );
    map.insert("server".into(), json!(host));
    map.insert("server_port".into(), json!(port));
    map.insert("method".into(), json!(method));
    map.insert("password".into(), json!(password));

    if let Some(plugin) = params.get("plugin").filter(|plugin| !plugin.is_empty()) {
        let (name, options) = match plugin.split_once(';') {
            Some((name, options)) => (name, Some(options)),
            None => (plugin, None),
        };
        map.insert("plugin".into(), json!(name));
        if let Some(options) = options.filter(|options| !options.is_empty()) {
            map.insert("plugin_opts".into(), json!(options));
        }
    }

    Ok(Value::Object(map))
}

fn parse_trojan(line: &str) -> Result<Value, Error> {
    let url = Url::parse(line).map_err(|err| Error::Uri(err.to_string()))?;
    let password = url
        .password()
        .map(percent_decode)
        .unwrap_or_else(|| percent_decode(url.username()));
    if password.is_empty() {
        return Err(Error::Uri("trojan URI has no password".into()));
    }

    let params = Params::from_url(&url);
    let server = host_of(&url)?;
    let port = port_of(&url)?;

    let mut map = Map::new();
    map.insert("type".into(), json!("trojan"));
    map.insert("tag".into(), json!(tag_of(&url)));
    map.insert("server".into(), json!(server));
    map.insert("server_port".into(), json!(port));
    map.insert("password".into(), json!(password));
    if let Some(transport) = transport_from(&params, &server) {
        map.insert("transport".into(), transport);
    }
    map.insert(
        "tls".into(),
        tls_from(&params, &server, true).unwrap_or_else(|| json!({ "enabled": true })),
    );

    Ok(Value::Object(map))
}

fn parse_hysteria2(line: &str) -> Result<Value, Error> {
    let url = Url::parse(line).map_err(|err| Error::Uri(err.to_string()))?;
    let password = url
        .password()
        .map(percent_decode)
        .unwrap_or_else(|| percent_decode(url.username()));
    if password.is_empty() {
        return Err(Error::Uri("hysteria2 URI has no password".into()));
    }

    let params = Params::from_url(&url);
    let server = host_of(&url)?;
    let port = port_of(&url)?;

    let mut map = Map::new();
    map.insert("type".into(), json!("hysteria2"));
    map.insert("tag".into(), json!(tag_of(&url)));
    map.insert("server".into(), json!(server));
    map.insert("server_port".into(), json!(port));
    map.insert("password".into(), json!(password));

    if let Some(obfs) = params.get("obfs").filter(|obfs| *obfs != "none") {
        map.insert(
            "obfs".into(),
            json!({
                "type": obfs,
                "password": params.get_any(&["obfs-password", "obfs_password"]).unwrap_or_default(),
            }),
        );
    }
    if let Some(tls) = tls_from(&params, &server, true) {
        map.insert("tls".into(), tls);
    }

    Ok(Value::Object(map))
}

fn parse_tuic(line: &str) -> Result<Value, Error> {
    let url = Url::parse(line).map_err(|err| Error::Uri(err.to_string()))?;
    let uuid = percent_decode(url.username());
    let password = url.password().map(percent_decode).unwrap_or_default();
    if uuid.is_empty() || password.is_empty() {
        return Err(Error::Uri("tuic URI must be uuid:password@host:port".into()));
    }

    let params = Params::from_url(&url);
    let server = host_of(&url)?;
    let port = port_of(&url)?;

    let mut map = Map::new();
    map.insert("type".into(), json!("tuic"));
    map.insert("tag".into(), json!(tag_of(&url)));
    map.insert("server".into(), json!(server));
    map.insert("server_port".into(), json!(port));
    map.insert("uuid".into(), json!(uuid));
    map.insert("password".into(), json!(password));
    if let Some(congestion) = params.get_any(&["congestion_control", "congestion-controller"]) {
        map.insert("congestion_control".into(), json!(congestion));
    }
    if let Some(relay_mode) = params.get_any(&["udp_relay_mode", "udp-relay-mode"]) {
        map.insert("udp_relay_mode".into(), json!(relay_mode));
    }
    if let Some(tls) = tls_from(&params, &server, true) {
        map.insert("tls".into(), tls);
    }

    Ok(Value::Object(map))
}

fn parse_anytls(line: &str) -> Result<Value, Error> {
    let url = Url::parse(line).map_err(|err| Error::Uri(err.to_string()))?;
    let password = url
        .password()
        .map(percent_decode)
        .unwrap_or_else(|| percent_decode(url.username()));
    if password.is_empty() {
        return Err(Error::Uri("anytls URI has no password".into()));
    }

    let params = Params::from_url(&url);
    let server = host_of(&url)?;
    let port = port_of(&url)?;

    let mut map = Map::new();
    map.insert("type".into(), json!("anytls"));
    map.insert("tag".into(), json!(tag_of(&url)));
    map.insert("server".into(), json!(server));
    map.insert("server_port".into(), json!(port));
    map.insert("password".into(), json!(password));
    if let Some(tls) = tls_from(&params, &server, true) {
        map.insert("tls".into(), tls);
    }

    Ok(Value::Object(map))
}

fn parse_socks(line: &str) -> Result<Value, Error> {
    let url = Url::parse(line).map_err(|err| Error::Uri(err.to_string()))?;
    let server = host_of(&url)?;
    let port = port_of(&url)?;

    let mut map = Map::new();
    map.insert("type".into(), json!("socks"));
    map.insert("tag".into(), json!(tag_of(&url)));
    map.insert("server".into(), json!(server));
    map.insert("server_port".into(), json!(port));
    map.insert("version".into(), json!("5"));
    if !url.username().is_empty() {
        map.insert("username".into(), json!(percent_decode(url.username())));
        map.insert(
            "password".into(),
            json!(url.password().map(percent_decode).unwrap_or_default()),
        );
    }

    Ok(Value::Object(map))
}

fn parse_http(line: &str) -> Result<Value, Error> {
    let url = Url::parse(line).map_err(|err| Error::Uri(err.to_string()))?;
    let params = Params::from_url(&url);
    let server = host_of(&url)?;
    let port = port_of(&url)?;

    let mut map = Map::new();
    map.insert("type".into(), json!("http"));
    map.insert("tag".into(), json!(tag_of(&url)));
    map.insert("server".into(), json!(server));
    map.insert("server_port".into(), json!(port));
    if !url.username().is_empty() {
        map.insert("username".into(), json!(percent_decode(url.username())));
        map.insert(
            "password".into(),
            json!(url.password().map(percent_decode).unwrap_or_default()),
        );
    }
    if url.scheme() == "https" || params.flag(&["tls"]) || params.get("security") == Some("tls") {
        map.insert("tls".into(), json!({ "enabled": true }));
    }

    Ok(Value::Object(map))
}

fn parse_hostport(value: &str) -> Result<(String, u16), Error> {
    let value = value.trim();
    if let Some(rest) = value.strip_prefix('[') {
        let (host, rest) = rest
            .split_once(']')
            .ok_or_else(|| Error::Uri("unterminated IPv6 address".into()))?;
        let port = rest
            .strip_prefix(':')
            .ok_or_else(|| Error::Uri("missing port".into()))?;
        return Ok((
            host.to_string(),
            port.parse().map_err(|_| Error::Uri("invalid port".into()))?,
        ));
    }

    let (host, port) = value
        .rsplit_once(':')
        .ok_or_else(|| Error::Uri("missing host or port".into()))?;
    if host.is_empty() {
        return Err(Error::Uri("missing host".into()));
    }
    Ok((
        host.to_string(),
        port.parse().map_err(|_| Error::Uri("invalid port".into()))?,
    ))
}

fn as_u64(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| {
        value
            .as_str()
            .and_then(|value| value.trim().parse::<u64>().ok())
    })
}
