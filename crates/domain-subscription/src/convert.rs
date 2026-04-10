use base64::{Engine, engine::general_purpose::STANDARD};
use percent_encoding::percent_decode_str;
use serde_json::{Map, Value, json};
use shared::{AppError, AppResult};
use url::{Url, form_urlencoded};

use crate::{ClientLink, decode_base64_or_plain};

pub fn append_external_client_outbounds(
    raw_links: &str,
    outbounds: &mut Vec<Value>,
    out_tags: &mut Vec<String>,
) -> AppResult<()> {
    let links: Vec<ClientLink> = serde_json::from_str(raw_links).unwrap_or_default();
    let external_links = links
        .into_iter()
        .filter(|link| link.kind == "external" && !link.uri.trim().is_empty())
        .collect::<Vec<_>>();

    let tag_numbering = if external_links.len() > 1 { 1 } else { 0 };
    for (index, link) in external_links.iter().enumerate() {
        let numbering = (index + 1) * tag_numbering;
        let (outbound, tag) = convert_link(&link.uri, numbering)?;
        if !tag.is_empty() {
            outbounds.push(outbound);
            out_tags.push(tag);
        }
    }

    Ok(())
}

pub async fn convert_external_subscription(
    client: &reqwest::Client,
    url: &str,
) -> AppResult<Vec<Value>> {
    if url.trim().is_empty() {
        return Err(AppError::Validation("no url".to_string()));
    }

    let response =
        client.get(url).send().await.map_err(|error| AppError::Validation(error.to_string()))?;
    let body = response.text().await.map_err(|error| AppError::Validation(error.to_string()))?;
    let data = decode_base64_or_plain(&body);
    let trimmed = data.trim();

    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        let value: Value = serde_json::from_str(trimmed)?;
        let outbounds = value
            .get("outbounds")
            .and_then(Value::as_array)
            .ok_or_else(|| AppError::Validation("no result".to_string()))?;

        let result = outbounds
            .iter()
            .filter_map(|outbound| {
                let object = outbound.as_object()?;
                let kind = object.get("type").and_then(Value::as_str).unwrap_or_default();
                if matches!(kind, "urltest" | "direct" | "selector" | "block") {
                    None
                } else {
                    Some(Value::Object(object.clone()))
                }
            })
            .collect::<Vec<_>>();

        if result.is_empty() {
            return Err(AppError::Validation("no result".to_string()));
        }
        return Ok(result);
    }

    let mut result = Vec::new();
    for line in trimmed.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if let Ok((outbound, _tag)) = convert_link(line, 0) {
            result.push(outbound);
        }
    }

    if result.is_empty() {
        return Err(AppError::Validation("no result".to_string()));
    }
    Ok(result)
}

pub fn convert_link(uri: &str, index: usize) -> AppResult<(Value, String)> {
    let raw = uri.trim();
    if raw.is_empty() {
        return Err(AppError::Validation("unsupported link format".to_string()));
    }

    if let Some(payload) = raw.strip_prefix("vmess://") {
        return parse_vmess(payload, index);
    }

    let url =
        Url::parse(raw).map_err(|_| AppError::Validation("unsupported link format".to_string()))?;
    match url.scheme() {
        "vless" => parse_vless(&url, index),
        "trojan" => parse_trojan(&url, index),
        "hy" | "hysteria" => parse_hysteria(&url, index),
        "hy2" | "hysteria2" => parse_hysteria2(&url, index),
        "anytls" => parse_anytls(&url, index),
        "tuic" => parse_tuic(&url, index),
        "ss" | "shadowsocks" => parse_shadowsocks(&url, index),
        "naive+https" | "naive+quic" | "http2" => parse_naive(&url, index),
        _ => Err(AppError::Validation("unsupported link format".to_string())),
    }
}

fn parse_vmess(payload: &str, index: usize) -> AppResult<(Value, String)> {
    let decoded =
        STANDARD.decode(payload).map_err(|error| AppError::Validation(error.to_string()))?;
    let value: Value = serde_json::from_slice(&decoded)?;
    let object =
        value.as_object().ok_or_else(|| AppError::Validation("invalid vmess".to_string()))?;

    let transport = vmess_transport(object)?;
    let tls = vmess_tls(object);
    let tag = with_index(object.get("ps").and_then(Value::as_str).unwrap_or_default(), index);
    let alter_id = object.get("aid").and_then(value_as_i64).unwrap_or_default();
    let port = object
        .get("port")
        .and_then(value_as_i64)
        .ok_or_else(|| AppError::Validation("invalid vmess port".to_string()))?;

    Ok((
        json!({
            "type": "vmess",
            "tag": tag,
            "server": object.get("add").cloned().unwrap_or(Value::Null),
            "server_port": port,
            "uuid": object.get("id").cloned().unwrap_or(Value::Null),
            "security": "auto",
            "alter_id": alter_id,
            "tls": Value::Object(tls),
            "transport": Value::Object(transport),
        }),
        tag,
    ))
}

fn parse_vless(url: &Url, index: usize) -> AppResult<(Value, String)> {
    let query = query_pairs(url);
    let security = query.get("security").map(String::as_str).unwrap_or_default();
    let (host, port) = host_port_for(url, security, 80, 443)?;
    let transport =
        get_transport(query.get("type").map(String::as_str).unwrap_or_default(), &query);
    let tag = with_index(url.fragment().unwrap_or_default(), index);

    Ok((
        json!({
            "type": "vless",
            "tag": tag,
            "server": host,
            "server_port": port,
            "uuid": url.username(),
            "flow": query.get("flow").cloned().unwrap_or_default(),
            "tls": Value::Object(get_tls(security, &query)),
            "transport": Value::Object(transport),
        }),
        tag,
    ))
}

fn parse_trojan(url: &Url, index: usize) -> AppResult<(Value, String)> {
    let query = query_pairs(url);
    let security = query.get("security").map(String::as_str).unwrap_or_default();
    let (host, port) = host_port_for(url, security, 80, 443)?;
    let transport =
        get_transport(query.get("type").map(String::as_str).unwrap_or_default(), &query);
    let tag = with_index(url.fragment().unwrap_or_default(), index);

    Ok((
        json!({
            "type": "trojan",
            "tag": tag,
            "server": host,
            "server_port": port,
            "password": url.username(),
            "tls": Value::Object(get_tls(security, &query)),
            "transport": Value::Object(transport),
        }),
        tag,
    ))
}

fn parse_hysteria(url: &Url, index: usize) -> AppResult<(Value, String)> {
    let query = query_pairs(url);
    let security = query
        .get("security")
        .filter(|value| !value.is_empty())
        .map(String::as_str)
        .unwrap_or("tls");
    let (host, port) = host_port_for(url, security, 443, 443)?;
    let tag = with_index(url.fragment().unwrap_or_default(), index);
    let mut outbound = Map::from_iter([
        ("type".to_string(), Value::String("hysteria".to_string())),
        ("tag".to_string(), Value::String(tag.clone())),
        ("server".to_string(), Value::String(host)),
        ("server_port".to_string(), json!(port)),
        ("obfs".to_string(), Value::String(query.get("obfsParam").cloned().unwrap_or_default())),
        ("auth_str".to_string(), Value::String(query.get("auth").cloned().unwrap_or_default())),
        ("tls".to_string(), Value::Object(get_tls(security, &query))),
    ]);
    insert_i64_if_positive(&mut outbound, "down_mbps", query.get("downmbps"));
    insert_i64_if_positive(&mut outbound, "up_mbps", query.get("upmbps"));
    insert_i64_if_positive(&mut outbound, "recv_window_conn", query.get("recv_window_conn"));
    insert_i64_if_positive(&mut outbound, "recv_window", query.get("recv_window"));
    Ok((Value::Object(outbound), tag))
}

fn parse_hysteria2(url: &Url, index: usize) -> AppResult<(Value, String)> {
    let query = query_pairs(url);
    let security = query
        .get("security")
        .filter(|value| !value.is_empty())
        .map(String::as_str)
        .unwrap_or("tls");
    let (host, port) = host_port_for(url, security, 443, 443)?;
    let tag = with_index(url.fragment().unwrap_or_default(), index);
    let mut outbound = Map::from_iter([
        ("type".to_string(), Value::String("hysteria2".to_string())),
        ("tag".to_string(), Value::String(tag.clone())),
        ("server".to_string(), Value::String(host)),
        ("server_port".to_string(), json!(port)),
        ("password".to_string(), Value::String(url.username().to_string())),
        ("tls".to_string(), Value::Object(get_tls(security, &query))),
    ]);
    insert_i64_if_positive(&mut outbound, "down_mbps", query.get("downmbps"));
    insert_i64_if_positive(&mut outbound, "up_mbps", query.get("upmbps"));
    if query.get("obfs").map(String::as_str) == Some("salamander") {
        outbound.insert(
            "obfs".to_string(),
            json!({
                "type": "salamander",
                "password": query.get("obfs-password").cloned().unwrap_or_default(),
            }),
        );
    }
    if let Some(ports) = query.get("mport").filter(|value| !value.is_empty()) {
        outbound.insert(
            "server_ports".to_string(),
            Value::Array(
                ports
                    .replace('-', ":")
                    .split(',')
                    .map(|value| Value::String(value.to_string()))
                    .collect(),
            ),
        );
    }
    if truthy(query.get("fastopen")) {
        outbound.insert("fastopen".to_string(), Value::Bool(true));
    }
    Ok((Value::Object(outbound), tag))
}

fn parse_anytls(url: &Url, index: usize) -> AppResult<(Value, String)> {
    let query = query_pairs(url);
    let security = query
        .get("security")
        .filter(|value| !value.is_empty())
        .map(String::as_str)
        .unwrap_or("tls");
    let (host, port) = host_port_for(url, security, 443, 443)?;
    let tag = with_index(url.fragment().unwrap_or_default(), index);
    Ok((
        json!({
            "type": "anytls",
            "tag": tag,
            "server": host,
            "server_port": port,
            "password": url.username(),
            "tls": Value::Object(get_tls(security, &query)),
        }),
        tag,
    ))
}

fn parse_tuic(url: &Url, index: usize) -> AppResult<(Value, String)> {
    let query = query_pairs(url);
    let security = query
        .get("security")
        .filter(|value| !value.is_empty())
        .map(String::as_str)
        .unwrap_or("tls");
    let (host, port) = host_port_for(url, security, 443, 443)?;
    let tag = with_index(url.fragment().unwrap_or_default(), index);
    let password = url.password().unwrap_or_default();
    Ok((
        json!({
            "type": "tuic",
            "tag": tag,
            "server": host,
            "server_port": port,
            "uuid": url.username(),
            "password": password,
            "congestion_control": query.get("congestion_control").cloned().unwrap_or_default(),
            "udp_relay_mode": query.get("udp_relay_mode").cloned().unwrap_or_default(),
            "tls": Value::Object(get_tls(security, &query)),
        }),
        tag,
    ))
}

fn parse_shadowsocks(url: &Url, index: usize) -> AppResult<(Value, String)> {
    let query = query_pairs(url);
    let (host, port) = host_port_for(url, "", 443, 443)?;
    let raw_method = percent_decode_str(url.username()).decode_utf8_lossy().into_owned();
    let (method, password) = if let Some(password) = url.password() {
        (raw_method, password.to_string())
    } else {
        let decoded = decode_base64_or_plain(&raw_method);
        let Some((decoded_method, decoded_password)) = decoded.split_once(':') else {
            return Err(AppError::Validation("unsupported shadowsocks".to_string()));
        };
        (decoded_method.to_string(), decoded_password.to_string())
    };

    let tag = with_index(url.fragment().unwrap_or_default(), index);
    let mut outbound = Map::from_iter([
        ("type".to_string(), Value::String("shadowsocks".to_string())),
        ("tag".to_string(), Value::String(tag.clone())),
        ("server".to_string(), Value::String(host)),
        ("server_port".to_string(), json!(port)),
        ("method".to_string(), Value::String(method)),
        ("password".to_string(), Value::String(password)),
    ]);

    if let Some(v2ray_type) = query.get("type").filter(|value| !value.is_empty()) {
        let mut plugin_opts = Vec::new();
        if query.get("security").map(String::as_str) == Some("tls") {
            plugin_opts.push("tls".to_string());
        }
        if v2ray_type == "quic" {
            plugin_opts.push("mode=quic".to_string());
        }
        if let Some(host_header) = query.get("host").filter(|value| !value.is_empty()) {
            plugin_opts.push(format!("host={host_header}"));
        }
        outbound.insert("plugin".to_string(), Value::String("v2ray-plugin".to_string()));
        outbound.insert("plugin_opts".to_string(), Value::String(plugin_opts.join(";")));
    }
    if let Some(plugin) = query.get("plugin").filter(|value| !value.is_empty()) {
        let mut parts = plugin.split(';');
        if let Some(name) = parts.next() {
            outbound.insert("plugin".to_string(), Value::String(name.to_string()));
            outbound.insert(
                "plugin_opts".to_string(),
                Value::String(parts.collect::<Vec<_>>().join(";")),
            );
        }
    }

    Ok((Value::Object(outbound), tag))
}

fn parse_naive(url: &Url, index: usize) -> AppResult<(Value, String)> {
    let (host, port, username, password) = match url.scheme() {
        "http2" => {
            let decoded = decode_base64_or_plain(url.host_str().unwrap_or_default());
            let Some((user_info, host_port)) = decoded.split_once('@') else {
                return Err(AppError::Validation("invalid naive link (http2)".to_string()));
            };
            let (username, password) = user_info
                .split_once(':')
                .map(|(user, pass)| (user.to_string(), pass.to_string()))
                .unwrap_or_else(|| (user_info.to_string(), String::new()));
            let (host, port) = split_host_port(host_port, 443)?;
            (host, port, username, password)
        }
        "naive+https" | "naive+quic" => {
            let (host, port) = host_port_for(url, "tls", 443, 443)?;
            (host, port, url.username().to_string(), url.password().unwrap_or_default().to_string())
        }
        _ => return Err(AppError::Validation("unsupported naive scheme".to_string())),
    };

    let tag = if url.fragment().unwrap_or_default().is_empty() {
        if index > 0 { format!("naive-{index}") } else { "naive-0".to_string() }
    } else {
        with_index(url.fragment().unwrap_or_default(), index)
    };

    let query = query_pairs(url);
    let mut outbound = Map::from_iter([
        ("type".to_string(), Value::String("naive".to_string())),
        ("tag".to_string(), Value::String(tag.clone())),
        ("server".to_string(), Value::String(host)),
        ("server_port".to_string(), json!(port)),
        ("username".to_string(), Value::String(username)),
        ("password".to_string(), Value::String(password)),
        ("tls".to_string(), json!({ "enabled": true })),
    ]);

    if let Some(peer) = query.get("peer").filter(|value| !value.is_empty()) {
        outbound
            .get_mut("tls")
            .and_then(Value::as_object_mut)
            .expect("naive tls object")
            .insert("server_name".to_string(), Value::String(peer.to_string()));
    }
    if truthy(query.get("insecure")) {
        outbound
            .get_mut("tls")
            .and_then(Value::as_object_mut)
            .expect("naive tls object")
            .insert("insecure".to_string(), Value::Bool(true));
    }
    if let Some(alpn) = query.get("alpn").filter(|value| !value.is_empty()) {
        outbound.get_mut("tls").and_then(Value::as_object_mut).expect("naive tls object").insert(
            "alpn".to_string(),
            Value::Array(alpn.split(',').map(|value| Value::String(value.to_string())).collect()),
        );
    }
    if url.scheme() == "naive+quic" {
        outbound.insert("quic".to_string(), Value::Bool(true));
    }

    Ok((Value::Object(outbound), tag))
}

fn vmess_transport(object: &Map<String, Value>) -> AppResult<Map<String, Value>> {
    let mut transport = Map::new();
    let network = object.get("net").and_then(Value::as_str).unwrap_or_default();
    let transport_type = object.get("type").and_then(Value::as_str).unwrap_or_default();
    let host = object.get("host").and_then(Value::as_str).unwrap_or_default();
    let path = object.get("path").and_then(Value::as_str).unwrap_or_default();

    match network.to_ascii_lowercase().as_str() {
        "tcp" | "" => {
            if transport_type == "http" {
                transport.insert("type".to_string(), Value::String(transport_type.to_string()));
                if !host.is_empty() {
                    transport.insert(
                        "host".to_string(),
                        Value::Array(
                            host.split(',').map(|value| Value::String(value.to_string())).collect(),
                        ),
                    );
                }
                transport.insert("path".to_string(), Value::String(path.to_string()));
            }
        }
        "http" | "h2" => {
            transport.insert("type".to_string(), Value::String("http".to_string()));
            if !host.is_empty() {
                transport.insert(
                    "host".to_string(),
                    Value::Array(
                        host.split(',').map(|value| Value::String(value.to_string())).collect(),
                    ),
                );
            }
            transport.insert("path".to_string(), Value::String(path.to_string()));
        }
        "ws" => {
            transport.insert("type".to_string(), Value::String("ws".to_string()));
            transport.insert("path".to_string(), Value::String(path.to_string()));
            transport.insert(
                "early_data_header_name".to_string(),
                Value::String("Sec-WebSocket-Protocol".to_string()),
            );
            if !host.is_empty() {
                transport.insert("headers".to_string(), json!({ "Host": host }));
            }
        }
        "quic" => {
            transport.insert("type".to_string(), Value::String("quic".to_string()));
        }
        "grpc" => {
            transport.insert("type".to_string(), Value::String("grpc".to_string()));
            transport.insert("service_name".to_string(), Value::String(path.to_string()));
        }
        "httpupgrade" => {
            transport.insert("type".to_string(), Value::String("httpupgrade".to_string()));
            transport.insert("path".to_string(), Value::String(path.to_string()));
            transport.insert("host".to_string(), Value::String(host.to_string()));
        }
        _ => return Err(AppError::Validation("invalid vmess".to_string())),
    }

    Ok(transport)
}

fn vmess_tls(object: &Map<String, Value>) -> Map<String, Value> {
    let mut tls = Map::new();
    if object.get("tls").and_then(Value::as_str) != Some("tls") {
        return tls;
    }

    tls.insert("enabled".to_string(), Value::Bool(true));
    if let Some(server_name) =
        object.get("sni").and_then(Value::as_str).filter(|value| !value.is_empty())
    {
        tls.insert("server_name".to_string(), Value::String(server_name.to_string()));
    }
    if let Some(alpn) = object.get("alpn").and_then(Value::as_str).filter(|value| !value.is_empty())
    {
        tls.insert(
            "alpn".to_string(),
            Value::Array(alpn.split(',').map(|value| Value::String(value.to_string())).collect()),
        );
    }
    if object.contains_key("allowInsecure") {
        tls.insert("insecure".to_string(), Value::Bool(true));
    }
    if let Some(fp) = object.get("fp").and_then(Value::as_str).filter(|value| !value.is_empty()) {
        tls.insert(
            "utls".to_string(),
            json!({
                "enabled": true,
                "fingerprint": fp,
            }),
        );
    }
    tls
}

fn get_transport(
    transport_type: &str,
    query: &std::collections::BTreeMap<String, String>,
) -> Map<String, Value> {
    let mut transport = Map::new();
    let host = query.get("host").cloned().unwrap_or_default();
    let path = query.get("path").cloned().unwrap_or_default();
    match transport_type.to_ascii_lowercase().as_str() {
        "tcp" | "" => {
            if query.get("headerType").map(String::as_str) == Some("http") {
                transport.insert("type".to_string(), Value::String("http".to_string()));
                if !host.is_empty() {
                    transport.insert(
                        "host".to_string(),
                        Value::Array(
                            host.split(',').map(|value| Value::String(value.to_string())).collect(),
                        ),
                    );
                }
                transport.insert("path".to_string(), Value::String(path));
            }
        }
        "http" | "h2" => {
            transport.insert("type".to_string(), Value::String("http".to_string()));
            if !host.is_empty() {
                transport.insert(
                    "host".to_string(),
                    Value::Array(
                        host.split(',').map(|value| Value::String(value.to_string())).collect(),
                    ),
                );
            }
            transport.insert("path".to_string(), Value::String(path));
        }
        "ws" => {
            transport.insert("type".to_string(), Value::String("ws".to_string()));
            transport.insert("path".to_string(), Value::String(path));
            if !host.is_empty() {
                transport.insert("headers".to_string(), json!({ "Host": host }));
            }
        }
        "quic" => {
            transport.insert("type".to_string(), Value::String("quic".to_string()));
        }
        "grpc" => {
            transport.insert("type".to_string(), Value::String("grpc".to_string()));
            transport.insert(
                "service_name".to_string(),
                Value::String(query.get("serviceName").cloned().unwrap_or_default()),
            );
        }
        "httpupgrade" => {
            transport.insert("type".to_string(), Value::String("httpupgrade".to_string()));
            transport.insert("path".to_string(), Value::String(path));
            transport.insert("host".to_string(), Value::String(host));
        }
        _ => {}
    }
    transport
}

fn get_tls(
    security: &str,
    query: &std::collections::BTreeMap<String, String>,
) -> Map<String, Value> {
    let mut tls = Map::new();
    match security {
        "tls" => {
            tls.insert("enabled".to_string(), Value::Bool(true));
        }
        "reality" => {
            tls.insert("enabled".to_string(), Value::Bool(true));
            tls.insert(
                "reality".to_string(),
                json!({
                    "enabled": true,
                    "public_key": query.get("pbk").cloned().unwrap_or_default(),
                    "short_id": query.get("sid").cloned().unwrap_or_default(),
                }),
            );
        }
        _ => {}
    }
    if let Some(server_name) = query.get("sni").filter(|value| !value.is_empty()) {
        tls.insert("server_name".to_string(), Value::String(server_name.to_string()));
    }
    if let Some(alpn) = query.get("alpn").filter(|value| !value.is_empty()) {
        tls.insert(
            "alpn".to_string(),
            Value::Array(alpn.split(',').map(|value| Value::String(value.to_string())).collect()),
        );
    }
    if truthy(query.get("insecure")) || truthy(query.get("allowInsecure")) {
        tls.insert("insecure".to_string(), Value::Bool(true));
    }
    if let Some(fp) = query.get("fp").filter(|value| !value.is_empty()) {
        tls.insert(
            "utls".to_string(),
            json!({
                "enabled": true,
                "fingerprint": fp,
            }),
        );
    }
    if let Some(ech) = query.get("ech").filter(|value| !value.is_empty()) {
        tls.insert(
            "ech".to_string(),
            json!({
                "enabled": true,
                "config": [ech],
            }),
        );
    }
    if truthy(query.get("disable_sni")) {
        tls.insert("disable_sni".to_string(), Value::Bool(true));
    }
    tls
}

fn query_pairs(url: &Url) -> std::collections::BTreeMap<String, String> {
    form_urlencoded::parse(url.query().unwrap_or_default().as_bytes()).into_owned().collect()
}

fn host_port_for(
    url: &Url,
    security: &str,
    default_plain: u16,
    default_secure: u16,
) -> AppResult<(String, u16)> {
    let host =
        url.host_str().ok_or_else(|| AppError::Validation("invalid host".to_string()))?.to_string();
    let port = url.port().unwrap_or(if matches!(security, "tls" | "reality") {
        default_secure
    } else {
        default_plain
    });
    Ok((host, port))
}

fn split_host_port(host_port: &str, default_port: u16) -> AppResult<(String, u16)> {
    let url = Url::parse(&format!("https://{host_port}"))
        .map_err(|error| AppError::Validation(error.to_string()))?;
    let host =
        url.host_str().ok_or_else(|| AppError::Validation("invalid host".to_string()))?.to_string();
    Ok((host, url.port().unwrap_or(default_port)))
}

fn with_index(tag: &str, index: usize) -> String {
    if index > 0 { format!("{index}.{tag}") } else { tag.to_string() }
}

fn truthy(value: Option<&String>) -> bool {
    matches!(value.map(String::as_str), Some("1" | "true"))
}

fn insert_i64_if_positive(object: &mut Map<String, Value>, key: &str, value: Option<&String>) {
    let Some(value) = value else {
        return;
    };
    let Ok(value) = value.parse::<i64>() else {
        return;
    };
    if value > 0 {
        object.insert(key.to_string(), json!(value));
    }
}

fn value_as_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().map(|value| value as i64))
        .or_else(|| value.as_str().and_then(|value| value.parse::<i64>().ok()))
}

#[cfg(test)]
mod tests {
    use super::{append_external_client_outbounds, convert_link};
    use serde_json::json;

    #[test]
    fn converts_socks5_like_shadowsocks_link() {
        let (outbound, tag) = convert_link("ss://YWVzLTEyOC1nY206cGFzczEyMw==@1.2.3.4:443#demo", 0)
            .expect("convert link");
        assert_eq!(tag, "demo");
        assert_eq!(outbound["type"], "shadowsocks");
        assert_eq!(outbound["server"], "1.2.3.4");
        assert_eq!(outbound["server_port"], 443);
    }

    #[test]
    fn appends_numbered_external_tags() {
        let raw_links = json!([
            { "type": "external", "remark": "", "uri": "trojan://pass@host1:443#one" },
            { "type": "external", "remark": "", "uri": "vless://uuid@host2:443?security=tls#two" }
        ])
        .to_string();
        let mut outbounds = Vec::new();
        let mut tags = Vec::new();
        append_external_client_outbounds(&raw_links, &mut outbounds, &mut tags)
            .expect("append external outbounds");
        assert_eq!(tags, vec!["1.one", "2.two"]);
        assert_eq!(outbounds.len(), 2);
    }
}
