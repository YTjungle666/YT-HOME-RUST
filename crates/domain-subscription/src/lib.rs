mod convert;

use std::borrow::Cow;

use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE, URL_SAFE_NO_PAD},
};
use convert::{append_external_client_outbounds, convert_external_subscription, convert_link};
use infra_db::Db;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use shared::{AppError, AppResult, model::ClientRow};
use url::{Url, form_urlencoded::byte_serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsBundle {
    pub server: Value,
    pub client: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboundLinkInput {
    pub id: i64,
    pub kind: String,
    pub tag: String,
    pub proxy_home: bool,
    pub tls_id: i64,
    pub tls: Option<TlsBundle>,
    pub addrs: Value,
    pub out_json: Value,
    pub options: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientLink {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub remark: Option<String>,
    pub uri: String,
}

#[derive(Clone)]
pub struct SubscriptionService {
    pool: Db,
    client: reqwest::Client,
}

impl SubscriptionService {
    pub fn new(pool: Db) -> AppResult<Self> {
        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .build()
            .map_err(|error| AppError::Validation(error.to_string()))?;
        Ok(Self { pool, client })
    }

    pub fn pool(&self) -> &Db {
        &self.pool
    }

    pub fn http_client(&self) -> &reqwest::Client {
        &self.client
    }

    pub fn convert_link(&self, uri: &str) -> AppResult<Value> {
        convert_link(uri, 0).map(|(outbound, _tag)| outbound)
    }

    pub async fn convert_subscription_link(&self, url: &str) -> AppResult<Vec<Value>> {
        convert_external_subscription(&self.client, url).await
    }

    pub async fn get_plain_subscription(&self, sub_id: &str) -> AppResult<SubscriptionDocument> {
        let client = self.load_client_by_sub_id(sub_id).await?;
        let show_info = self.get_bool_setting("subShowInfo").await?;
        let encode = self.get_bool_setting("subEncode").await?;
        let update_interval = self.get_i64_setting("subUpdates").await?;
        let client_info = if show_info { build_client_info(&client) } else { String::new() };
        let links = self.get_links(&client.links, true, &client_info).await?;
        let body = if encode { STANDARD.encode(links.join("\n")) } else { links.join("\n") };
        Ok(SubscriptionDocument { body, headers: subscription_headers(&client, update_interval) })
    }

    pub async fn get_json_subscription(
        &self,
        sub_id: &str,
        inbound_ref: Option<&str>,
    ) -> AppResult<SubscriptionDocument> {
        let client = self.load_client_by_sub_id(sub_id).await?;
        let inbounds = self.load_client_inbounds(&client, inbound_ref).await?;
        let (mut outbounds, out_tags) = build_local_outbounds(
            &parse_json_text(&client.config, Value::Object(Map::new()))?,
            &inbounds,
        )?;
        let mut out_tags = out_tags;

        if inbound_ref.is_none() {
            append_external_client_outbounds(&client.links, &mut outbounds, &mut out_tags)?;
        }

        let body = if inbound_ref.is_some() && has_proxy_home_enabled(&inbounds) {
            let mut json_config: Value = serde_json::from_str(DEFAULT_HOME_PROXY_JSON)?;
            if let Some(root) = json_config.as_object_mut() {
                if let Some(home_dns) = build_home_proxy_dns(&outbounds) {
                    root.insert("dns".to_string(), home_dns);
                }
                add_home_proxy_outbounds(&mut outbounds, &out_tags);
                root.insert("outbounds".to_string(), Value::Array(outbounds));
                root.insert(
                    "route".to_string(),
                    json!({
                        "auto_detect_interface": true,
                        "final": "proxy"
                    }),
                );
            }
            serde_json::to_string_pretty(&json_config)?
        } else {
            let mut json_config: Value = serde_json::from_str(DEFAULT_JSON)?;
            if let Some(root) = json_config.as_object_mut() {
                add_default_outbounds(&mut outbounds, &out_tags);
                root.insert("outbounds".to_string(), Value::Array(outbounds));
                merge_other_json_settings(root, &self.get_setting_value("subJsonExt").await?)?;
            }
            serde_json::to_string_pretty(&json_config)?
        };

        Ok(SubscriptionDocument {
            body,
            headers: subscription_headers(&client, self.get_i64_setting("subUpdates").await?),
        })
    }

    pub async fn get_clash_subscription(
        &self,
        sub_id: &str,
        inbound_ref: Option<&str>,
    ) -> AppResult<SubscriptionDocument> {
        let client = self.load_client_by_sub_id(sub_id).await?;
        let inbounds = self.load_client_inbounds(&client, inbound_ref).await?;
        let (mut outbounds, _out_tags) = build_local_outbounds(
            &parse_json_text(&client.config, Value::Object(Map::new()))?,
            &inbounds,
        )?;

        if inbound_ref.is_none() {
            let mut extra_tags = Vec::new();
            append_external_client_outbounds(&client.links, &mut outbounds, &mut extra_tags)?;
        }

        let base_config = if inbound_ref.is_some() && has_proxy_home_enabled(&inbounds) {
            build_home_proxy_clash_config(&outbounds)?
        } else {
            let value = self.get_setting_value("subClashExt").await?;
            if value.trim().is_empty() { BASIC_CLASH_CONFIG.to_string() } else { value }
        };
        let body = convert_to_clash_meta(&outbounds, &base_config)?;
        Ok(SubscriptionDocument {
            body,
            headers: subscription_headers(&client, self.get_i64_setting("subUpdates").await?),
        })
    }

    async fn get_links(
        &self,
        raw_links: &str,
        include_local: bool,
        client_info: &str,
    ) -> AppResult<Vec<String>> {
        let values = parse_json_text(raw_links, Value::Array(Vec::new()))?;
        let mut result = Vec::new();
        for value in values.as_array().cloned().unwrap_or_default() {
            let link: ClientLink = serde_json::from_value(value)?;
            match link.kind.as_str() {
                "external" => result.push(link.uri),
                "sub" => {
                    let content = self.fetch_external_link(&link.uri).await?;
                    result.extend(
                        content
                            .lines()
                            .map(str::trim)
                            .filter(|line| !line.is_empty())
                            .map(ToOwned::to_owned),
                    );
                }
                "local" if include_local => result.push(add_client_info(&link.uri, client_info)?),
                _ => {}
            }
        }
        Ok(result)
    }

    async fn fetch_external_link(&self, url: &str) -> AppResult<String> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|error| AppError::Validation(error.to_string()))?;
        let body =
            response.text().await.map_err(|error| AppError::Validation(error.to_string()))?;
        Ok(decode_base64_or_plain(&body).into_owned())
    }

    async fn load_client_by_sub_id(&self, sub_id: &str) -> AppResult<ClientRow> {
        let Some(client) = sqlx::query_as::<_, ClientRow>(
            r#"
            SELECT
                id, enable, name, config, inbounds, links, volume, expiry, down, up, desc,
                group_name, delay_start, auto_reset, reset_days, next_reset, total_up, total_down
            FROM clients
            WHERE enable = 1 AND name = ?
            LIMIT 1
            "#,
        )
        .bind(sub_id)
        .fetch_optional(&self.pool)
        .await?
        else {
            return Err(AppError::NotFound("subscription client not found".to_string()));
        };
        Ok(client)
    }

    async fn load_client_inbounds(
        &self,
        client: &ClientRow,
        inbound_ref: Option<&str>,
    ) -> AppResult<Vec<InboundLinkInput>> {
        let inbound_ids = parse_i64_array(&client.inbounds)?;
        let mut inbounds = Vec::new();
        for id in inbound_ids {
            let Some(row) = sqlx::query_as::<_, shared::model::InboundRow>(
                "SELECT id, kind, tag, COALESCE(tls_id, 0) AS tls_id, allow_lan_access, addrs, out_json, options FROM inbounds WHERE id = ? LIMIT 1",
            )
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            else {
                continue;
            };
            let tls = if row.tls_id > 0 {
                match sqlx::query_as::<_, shared::model::TlsRow>(
                    "SELECT id, name, server, client FROM tls WHERE id = ? LIMIT 1",
                )
                .bind(row.tls_id)
                .fetch_optional(&self.pool)
                .await?
                {
                    Some(tls) => Some(TlsBundle {
                        server: parse_json_text(&tls.server, Value::Object(Map::new()))
                            .unwrap_or_else(|_| Value::Object(Map::new())),
                        client: parse_json_text(&tls.client, Value::Object(Map::new()))
                            .unwrap_or_else(|_| Value::Object(Map::new())),
                    }),
                    None => None,
                }
            } else {
                None
            };

            inbounds.push(InboundLinkInput {
                id: row.id,
                kind: row.kind,
                tag: row.tag,
                proxy_home: row.allow_lan_access,
                tls_id: row.tls_id,
                tls,
                addrs: parse_json_text(&row.addrs, Value::Array(Vec::new()))?,
                out_json: parse_json_text(&row.out_json, Value::Object(Map::new()))?,
                options: parse_json_text(&row.options, Value::Object(Map::new()))?,
            });
        }

        filter_subscription_inbounds(inbounds, inbound_ref)
    }

    async fn get_setting_value(&self, key: &str) -> AppResult<String> {
        let Some(value) =
            sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key = ? LIMIT 1")
                .bind(key)
                .fetch_optional(&self.pool)
                .await?
        else {
            return Err(AppError::NotFound(format!("setting {key} not found")));
        };
        Ok(value)
    }

    async fn get_i64_setting(&self, key: &str) -> AppResult<i64> {
        self.get_setting_value(key)
            .await?
            .parse::<i64>()
            .map_err(|error| AppError::Validation(error.to_string()))
    }

    async fn get_bool_setting(&self, key: &str) -> AppResult<bool> {
        match self.get_setting_value(key).await?.as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            other => {
                Err(AppError::Validation(format!("setting {key} has invalid bool value {other}")))
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct SubscriptionDocument {
    pub body: String,
    pub headers: SubscriptionHeaders,
}

#[derive(Debug, Clone)]
pub struct SubscriptionHeaders {
    pub userinfo: String,
    pub update_interval: String,
    pub title: String,
}

const DEFAULT_JSON: &str = r#"
{
  "inbounds": [
    {
      "type": "tun",
      "address": [
        "172.19.0.1/30",
        "fdfe:dcba:9876::1/126"
      ],
      "mtu": 9000,
      "auto_route": true,
      "strict_route": false,
      "endpoint_independent_nat": false,
      "stack": "system",
      "platform": {
        "http_proxy": {
          "enabled": true,
          "server": "127.0.0.1",
          "server_port": 2080
        }
      }
    },
    {
      "type": "mixed",
      "listen": "127.0.0.1",
      "listen_port": 2080,
      "users": []
    }
  ]
}
"#;

const DEFAULT_HOME_PROXY_JSON: &str = DEFAULT_JSON;

const BASIC_CLASH_CONFIG: &str = r#"mixed-port: 7890
allow-lan: false
mode: rule
log-level: info
external-controller: 127.0.0.1:9090
tun:
  enable: true
  stack: system
  auto-route: true
  auto-detect-interface: true
  dns-hijack:
    - any:53
dns:
  enable: true
  ipv6: false
  enhanced-mode: fake-ip
  fake-ip-range: 198.18.0.1/16
  default-nameserver:
    - 8.8.8.8
    - 1.1.1.1
  nameserver:
    - https://doh.pub/dns-query
    - https://1.0.0.1/dns-query
  fallback:
    - tcp://9.9.9.9:53
  fake-ip-filter:
    - "*.lan"
    - localhost
    - "*.local"
rules:
  - GEOIP,Private,DIRECT
  - MATCH,Proxy
"#;

const PROXY_GROUPS: &str = r#"- name: Proxy
  type: select
  proxies: []
- name: Auto
  type: url-test
  proxies: []
  url: http://www.gstatic.com/generate_204
  interval: 300
  tolerance: 50
"#;

pub fn prepare_tls(bundle: &TlsBundle) -> AppResult<Option<Map<String, Value>>> {
    let Some(server) = bundle.server.as_object() else {
        return Ok(None);
    };
    let Some(client) = bundle.client.as_object() else {
        return Ok(None);
    };

    let mut output = client.clone();
    for key in [
        "enabled",
        "server_name",
        "alpn",
        "min_version",
        "max_version",
        "certificate",
        "cipher_suites",
    ] {
        if let Some(value) = server.get(key) {
            output.insert(key.to_string(), value.clone());
        }
    }

    if let Some(reality) = server.get("reality").and_then(Value::as_object) {
        let enabled = reality.get("enabled").and_then(Value::as_bool).unwrap_or(false);
        if enabled {
            let mut client_reality =
                output.get("reality").and_then(Value::as_object).cloned().unwrap_or_default();
            client_reality.insert("enabled".to_string(), Value::Bool(true));
            if let Some(public_key) = reality.get("public_key") {
                client_reality.insert("public_key".to_string(), public_key.clone());
            }
            if output.get("server_name").and_then(Value::as_str).unwrap_or_default().is_empty() {
                if let Some(server_name) = reality
                    .get("handshake")
                    .and_then(Value::as_object)
                    .and_then(|handshake| handshake.get("server"))
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                {
                    output
                        .insert("server_name".to_string(), Value::String(server_name.to_string()));
                }
            }
            if let Some(short_id) = pick_first_short_id(reality.get("short_id")) {
                client_reality.insert("short_id".to_string(), Value::String(short_id));
            }
            output.insert("reality".to_string(), Value::Object(client_reality));
        }
    }

    if let Some(ech) = server.get("ech").and_then(Value::as_object) {
        let enabled = ech.get("enabled").and_then(Value::as_bool).unwrap_or(false);
        if enabled {
            let mut client_ech =
                output.get("ech").and_then(Value::as_object).cloned().unwrap_or_default();
            client_ech.insert("enabled".to_string(), Value::Bool(true));
            for key in ["pq_signature_schemes_enabled", "dynamic_record_sizing_disabled", "config"]
            {
                if let Some(value) = ech.get(key) {
                    client_ech.insert(key.to_string(), value.clone());
                }
            }
            output.insert("ech".to_string(), Value::Object(client_ech));
        }
    }

    Ok(Some(output))
}

pub fn generate_links(
    client_config: &Value,
    inbound: &InboundLinkInput,
    hostname: &str,
) -> AppResult<Vec<String>> {
    let user_config = client_config
        .as_object()
        .ok_or_else(|| AppError::Validation("client config must be a JSON object".to_string()))?;
    let options = inbound
        .options
        .as_object()
        .ok_or_else(|| AppError::Validation("inbound options must be a JSON object".to_string()))?;
    let tls = inbound.tls.as_ref().map(prepare_tls).transpose()?.flatten();

    let mut addrs = normalize_addrs(inbound, options, hostname, tls.as_ref())?;
    if addrs.is_empty() {
        return Ok(Vec::new());
    }

    let links = match inbound.kind.as_str() {
        "socks" => socks_links(user_config.get("socks"), &addrs)?,
        "http" => http_links(user_config.get("http"), &addrs)?,
        "mixed" => {
            let mut links = socks_links(user_config.get("socks"), &addrs)?;
            links.extend(http_links(user_config.get("http"), &addrs)?);
            links
        }
        "shadowsocks" => shadowsocks_links(user_config, options, &addrs)?,
        "naive" => naive_links(user_config.get("naive"), options, &addrs)?,
        "hysteria" => {
            hysteria_links(user_config.get("hysteria"), options, &inbound.out_json, &addrs)?
        }
        "hysteria2" => {
            hysteria2_links(user_config.get("hysteria2"), options, &inbound.out_json, &addrs)?
        }
        "tuic" => tuic_links(user_config.get("tuic"), options, &addrs)?,
        "vless" => vless_links(user_config.get("vless"), options, &addrs)?,
        "anytls" => anytls_links(user_config.get("anytls"), &addrs)?,
        "trojan" => trojan_links(user_config.get("trojan"), options, &addrs)?,
        "vmess" => vmess_links(user_config.get("vmess"), options, &addrs)?,
        _ => Vec::new(),
    };

    addrs.clear();
    Ok(links)
}

#[derive(Debug, Clone)]
struct LinkAddr {
    server: String,
    server_port: u64,
    remark: String,
    tls: Option<Map<String, Value>>,
}

fn normalize_addrs(
    inbound: &InboundLinkInput,
    options: &Map<String, Value>,
    hostname: &str,
    tls: Option<&Map<String, Value>>,
) -> AppResult<Vec<LinkAddr>> {
    let mut addrs = Vec::new();
    let subscription_server = subscription_server_override(options).unwrap_or(hostname).to_string();
    let listen_port = options
        .get("listen_port")
        .and_then(Value::as_u64)
        .or_else(|| options.get("listen_port").and_then(Value::as_i64).map(|value| value as u64))
        .unwrap_or_default();

    let addr_values = inbound.addrs.as_array().cloned().unwrap_or_default();

    if addr_values.is_empty() {
        addrs.push(LinkAddr {
            server: subscription_server,
            server_port: listen_port,
            remark: inbound.tag.clone(),
            tls: tls.cloned(),
        });
        return Ok(addrs);
    }

    for addr in addr_values {
        let Some(addr_object) = addr.as_object() else {
            continue;
        };
        let server =
            addr_object.get("server").and_then(Value::as_str).unwrap_or(hostname).to_string();
        let server_port = addr_object
            .get("server_port")
            .and_then(Value::as_u64)
            .or_else(|| {
                addr_object.get("server_port").and_then(Value::as_i64).map(|value| value as u64)
            })
            .unwrap_or(listen_port);
        let remark = format!(
            "{}{}",
            inbound.tag,
            addr_object.get("remark").and_then(Value::as_str).unwrap_or_default()
        );
        let merged_tls = merge_addr_tls(tls, addr_object.get("tls"))?;
        addrs.push(LinkAddr { server, server_port, remark, tls: merged_tls });
    }

    Ok(addrs)
}

fn subscription_server_override(options: &Map<String, Value>) -> Option<&str> {
    options
        .get("subscribe_server")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn merge_addr_tls(
    base_tls: Option<&Map<String, Value>>,
    addr_tls: Option<&Value>,
) -> AppResult<Option<Map<String, Value>>> {
    let mut merged = base_tls.cloned();
    if let Some(addr_tls) = addr_tls {
        let Some(addr_tls) = addr_tls.as_object() else {
            return Err(AppError::Validation(
                "addr tls override must be a JSON object".to_string(),
            ));
        };
        let output = merged.get_or_insert_with(Map::new);
        for (key, value) in addr_tls {
            output.insert(key.clone(), value.clone());
        }
    }
    Ok(merged)
}

fn socks_links(config: Option<&Value>, addrs: &[LinkAddr]) -> AppResult<Vec<String>> {
    let config = config_object(config, "socks")?;
    let username = get_required_string(config, "username")?;
    let password = get_required_string(config, "password")?;
    Ok(addrs
        .iter()
        .map(|addr| format!("socks5://{username}:{password}@{}:{}", addr.server, addr.server_port))
        .collect())
}

fn http_links(config: Option<&Value>, addrs: &[LinkAddr]) -> AppResult<Vec<String>> {
    let config = config_object(config, "http")?;
    let username = get_required_string(config, "username")?;
    let password = get_required_string(config, "password")?;
    Ok(addrs
        .iter()
        .map(|addr| {
            let scheme = if addr.tls.is_some() { "https" } else { "http" };
            format!("{scheme}://{username}:{password}@{}:{}", addr.server, addr.server_port)
        })
        .collect())
}

fn shadowsocks_links(
    configs: &Map<String, Value>,
    inbound: &Map<String, Value>,
    addrs: &[LinkAddr],
) -> AppResult<Vec<String>> {
    let method = get_required_string(inbound, "method")?;
    let mut passwords = Vec::new();
    if method.starts_with("2022") {
        passwords.push(get_required_string(inbound, "password")?);
    }

    let password_config_key =
        if method == "2022-blake3-aes-128-gcm" { "shadowsocks16" } else { "shadowsocks" };
    let password = config_object(configs.get(password_config_key), password_config_key)
        .and_then(|config| get_required_string(config, "password"))?;
    passwords.push(password);
    let uri_base = format!("ss://{}", STANDARD.encode(format!("{method}:{}", passwords.join(":"))));

    Ok(addrs
        .iter()
        .map(|addr| format!("{uri_base}@{}:{}#{}", addr.server, addr.server_port, addr.remark))
        .collect())
}

fn naive_links(
    config: Option<&Value>,
    inbound: &Map<String, Value>,
    addrs: &[LinkAddr],
) -> AppResult<Vec<String>> {
    let config = config_object(config, "naive")?;
    let username = get_required_string(config, "username")?;
    let password = get_required_string(config, "password")?;
    let mut links = Vec::with_capacity(addrs.len());
    for addr in addrs {
        let mut params = vec![("padding".to_string(), "1".to_string())];
        if let Some(tls) = addr.tls.as_ref() {
            if let Some(peer) = tls.get("server_name").and_then(Value::as_str) {
                params.push(("peer".to_string(), peer.to_string()));
            }
            if let Some(alpn) = value_array_to_csv(tls.get("alpn")) {
                params.push(("alpn".to_string(), alpn));
            }
            if tls.get("insecure").and_then(Value::as_bool).unwrap_or(false) {
                params.push(("insecure".to_string(), "1".to_string()));
            }
        }
        let tfo = inbound.get("tcp_fast_open").and_then(Value::as_bool).unwrap_or(false);
        params.push(("tfo".to_string(), if tfo { "1" } else { "0" }.to_string()));

        let encoded =
            STANDARD.encode(format!("{username}:{password}@{}:{}", addr.server, addr.server_port));
        links.push(add_params(&format!("http2://{encoded}"), params, &addr.remark));
    }
    Ok(links)
}

fn hysteria_links(
    config: Option<&Value>,
    inbound: &Map<String, Value>,
    out_json: &Value,
    addrs: &[LinkAddr],
) -> AppResult<Vec<String>> {
    let config = config_object(config, "hysteria")?;
    let auth = get_optional_string(config, "auth_str");
    let mut links = Vec::with_capacity(addrs.len());
    for addr in addrs {
        let mut params = Vec::new();
        if let Some(up) = number_to_string(inbound.get("up_mbps")) {
            params.push(("downmbps".to_string(), up));
        }
        if let Some(down) = number_to_string(inbound.get("down_mbps")) {
            params.push(("upmbps".to_string(), down));
        }
        if let Some(auth) = auth.clone() {
            params.push(("auth".to_string(), auth));
        }
        if let Some(tls) = addr.tls.as_ref() {
            push_tls_params(&mut params, tls, "insecure");
        }
        if let Some(obfs) = get_optional_string(inbound, "obfs") {
            params.push(("obfs".to_string(), obfs));
        }
        let fastopen = inbound.get("tcp_fast_open").and_then(Value::as_bool).unwrap_or(false);
        params.push(("fastopen".to_string(), if fastopen { "1" } else { "0" }.to_string()));
        if let Some(mports) = out_json
            .as_object()
            .and_then(|object| object.get("server_ports"))
            .and_then(|value| value_array_to_csv_opt(Some(value)))
        {
            params.push(("mport".to_string(), mports));
        }
        links.push(add_params(
            &format!("hysteria://{}:{}", addr.server, addr.server_port),
            params,
            &addr.remark,
        ));
    }
    Ok(links)
}

fn hysteria2_links(
    config: Option<&Value>,
    inbound: &Map<String, Value>,
    out_json: &Value,
    addrs: &[LinkAddr],
) -> AppResult<Vec<String>> {
    let config = config_object(config, "hysteria2")?;
    let password = get_required_string(config, "password")?;
    let mut links = Vec::with_capacity(addrs.len());
    for addr in addrs {
        let mut params = Vec::new();
        if let Some(up) = number_to_string(inbound.get("up_mbps")) {
            params.push(("downmbps".to_string(), up));
        }
        if let Some(down) = number_to_string(inbound.get("down_mbps")) {
            params.push(("upmbps".to_string(), down));
        }
        if let Some(tls) = addr.tls.as_ref() {
            push_tls_params(&mut params, tls, "insecure");
        }
        if let Some(obfs) = inbound.get("obfs").and_then(Value::as_object) {
            if let Some(kind) = obfs.get("type").and_then(Value::as_str) {
                params.push(("obfs".to_string(), kind.to_string()));
            }
            if let Some(password) = obfs.get("password").and_then(Value::as_str) {
                params.push(("obfs-password".to_string(), password.to_string()));
            }
        }
        let fastopen = inbound.get("tcp_fast_open").and_then(Value::as_bool).unwrap_or(false);
        params.push(("fastopen".to_string(), if fastopen { "1" } else { "0" }.to_string()));
        if let Some(mports) = out_json
            .as_object()
            .and_then(|object| object.get("server_ports"))
            .and_then(|value| value_array_to_csv_opt(Some(value)))
        {
            params.push(("mport".to_string(), mports));
        }
        links.push(add_params(
            &format!("hysteria2://{password}@{}:{}", addr.server, addr.server_port),
            params,
            &addr.remark,
        ));
    }
    Ok(links)
}

fn anytls_links(config: Option<&Value>, addrs: &[LinkAddr]) -> AppResult<Vec<String>> {
    let config = config_object(config, "anytls")?;
    let password = get_required_string(config, "password")?;
    Ok(addrs
        .iter()
        .map(|addr| {
            let mut params = Vec::new();
            if let Some(tls) = addr.tls.as_ref() {
                push_tls_params(&mut params, tls, "insecure");
            }
            add_params(
                &format!("anytls://{password}@{}:{}", addr.server, addr.server_port),
                params,
                &addr.remark,
            )
        })
        .collect())
}

fn tuic_links(
    config: Option<&Value>,
    inbound: &Map<String, Value>,
    addrs: &[LinkAddr],
) -> AppResult<Vec<String>> {
    let config = config_object(config, "tuic")?;
    let uuid = get_required_string(config, "uuid")?;
    let password = get_required_string(config, "password")?;
    Ok(addrs
        .iter()
        .map(|addr| {
            let mut params = Vec::new();
            if let Some(tls) = addr.tls.as_ref() {
                push_tls_params(&mut params, tls, "insecure");
            }
            if let Some(congestion) = get_optional_string(inbound, "congestion_control") {
                params.push(("congestion_control".to_string(), congestion));
            }
            add_params(
                &format!("tuic://{uuid}:{password}@{}:{}", addr.server, addr.server_port),
                params,
                &addr.remark,
            )
        })
        .collect())
}

fn vless_links(
    config: Option<&Value>,
    inbound: &Map<String, Value>,
    addrs: &[LinkAddr],
) -> AppResult<Vec<String>> {
    let config = config_object(config, "vless")?;
    let uuid = get_required_string(config, "uuid")?;
    let base_params = transport_params(inbound.get("transport"));
    Ok(addrs
        .iter()
        .map(|addr| {
            let mut params = base_params.clone();
            if let Some(tls) = addr.tls.as_ref() {
                if tls.get("enabled").and_then(Value::as_bool).unwrap_or(false) {
                    push_tls_params(&mut params, tls, "allowInsecure");
                    if let Some(flow) = get_optional_string(config, "flow") {
                        params.push(("flow".to_string(), flow));
                    }
                }
            }
            add_params(
                &format!("vless://{uuid}@{}:{}", addr.server, addr.server_port),
                params,
                &addr.remark,
            )
        })
        .collect())
}

fn trojan_links(
    config: Option<&Value>,
    inbound: &Map<String, Value>,
    addrs: &[LinkAddr],
) -> AppResult<Vec<String>> {
    let config = config_object(config, "trojan")?;
    let password = get_required_string(config, "password")?;
    let base_params = transport_params(inbound.get("transport"));
    Ok(addrs
        .iter()
        .map(|addr| {
            let mut params = base_params.clone();
            if let Some(tls) = addr.tls.as_ref() {
                if tls.get("enabled").and_then(Value::as_bool).unwrap_or(false) {
                    push_tls_params(&mut params, tls, "allowInsecure");
                }
            }
            add_params(
                &format!("trojan://{password}@{}:{}", addr.server, addr.server_port),
                params,
                &addr.remark,
            )
        })
        .collect())
}

fn vmess_links(
    config: Option<&Value>,
    inbound: &Map<String, Value>,
    addrs: &[LinkAddr],
) -> AppResult<Vec<String>> {
    let config = config_object(config, "vmess")?;
    let uuid = get_required_string(config, "uuid")?;
    let transport_params = transport_params(inbound.get("transport"));
    let mut net = "tcp".to_string();
    let mut typ = None::<String>;
    let mut host = None::<String>;
    let mut path = None::<String>;
    for (key, value) in &transport_params {
        match key.as_str() {
            "type" => net = value.clone(),
            "host" => host = Some(value.clone()),
            "path" => path = Some(value.clone()),
            _ => {}
        }
    }
    if net == "http" {
        net = "tcp".to_string();
        typ = Some("http".to_string());
    }

    let mut links = Vec::with_capacity(addrs.len());
    for addr in addrs {
        let mut object = Map::from_iter([
            ("v".to_string(), Value::String("2".to_string())),
            ("id".to_string(), Value::String(uuid.clone())),
            ("aid".to_string(), json!(0)),
            ("add".to_string(), Value::String(addr.server.clone())),
            ("port".to_string(), Value::String(addr.server_port.to_string())),
            ("ps".to_string(), Value::String(addr.remark.clone())),
            ("net".to_string(), Value::String(net.clone())),
        ]);
        if let Some(value) = typ.as_ref() {
            object.insert("type".to_string(), Value::String(value.clone()));
        }
        if let Some(value) = host.as_ref() {
            object.insert("host".to_string(), Value::String(value.clone()));
        }
        if let Some(value) = path.as_ref() {
            object.insert("path".to_string(), Value::String(value.clone()));
        }
        populate_vmess_tls(&mut object, addr.tls.as_ref());
        links.push(format!(
            "vmess://{}",
            STANDARD.encode(serde_json::to_vec(&Value::Object(object))?)
        ));
    }
    Ok(links)
}

fn populate_vmess_tls(object: &mut Map<String, Value>, tls: Option<&Map<String, Value>>) {
    let Some(tls) = tls else {
        object.insert("tls".to_string(), Value::String("none".to_string()));
        return;
    };
    if !tls.get("enabled").and_then(Value::as_bool).unwrap_or(false) {
        object.insert("tls".to_string(), Value::String("none".to_string()));
        return;
    }

    object.insert("tls".to_string(), Value::String("tls".to_string()));
    let mut params = Vec::new();
    push_tls_params(&mut params, tls, "allowInsecure");
    for (key, value) in params {
        match key.as_str() {
            "security" => {}
            "allowInsecure" => {
                object.insert("allowInsecure".to_string(), json!(1));
            }
            "sni" => {
                object.insert("sni".to_string(), Value::String(value));
            }
            "fp" => {
                object.insert("fp".to_string(), Value::String(value));
            }
            "alpn" => {
                object.insert("alpn".to_string(), Value::String(value));
            }
            _ => {}
        }
    }
}

fn transport_params(transport: Option<&Value>) -> Vec<(String, String)> {
    let mut params = Vec::new();
    let Some(transport) = transport.and_then(Value::as_object) else {
        params.push(("type".to_string(), "tcp".to_string()));
        return params;
    };
    let transport_type = transport.get("type").and_then(Value::as_str).unwrap_or("tcp").to_string();
    params.push(("type".to_string(), transport_type.clone()));
    if transport_type == "tcp" {
        return params;
    }

    match transport_type.as_str() {
        "http" => {
            if let Some(host) = value_array_to_csv_opt(transport.get("host")) {
                params.push(("host".to_string(), host));
            }
            if let Some(path) = transport.get("path").and_then(Value::as_str) {
                params.push(("path".to_string(), path.to_string()));
            }
        }
        "ws" => {
            if let Some(path) = transport.get("path").and_then(Value::as_str) {
                params.push(("path".to_string(), path.to_string()));
            }
            if let Some(host) = transport
                .get("headers")
                .and_then(Value::as_object)
                .and_then(|headers| headers.get("Host"))
                .and_then(Value::as_str)
            {
                params.push(("host".to_string(), host.to_string()));
            }
        }
        "grpc" => {
            if let Some(service_name) = transport.get("service_name").and_then(Value::as_str) {
                params.push(("serviceName".to_string(), service_name.to_string()));
            }
        }
        "httpupgrade" => {
            if let Some(host) = transport.get("host").and_then(Value::as_str) {
                params.push(("host".to_string(), host.to_string()));
            }
            if let Some(path) = transport.get("path").and_then(Value::as_str) {
                params.push(("path".to_string(), path.to_string()));
            }
        }
        _ => {}
    }
    params
}

fn push_tls_params(
    params: &mut Vec<(String, String)>,
    tls: &Map<String, Value>,
    insecure_key: &str,
) {
    let is_reality = tls
        .get("reality")
        .and_then(Value::as_object)
        .and_then(|reality| reality.get("enabled"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if is_reality {
        params.push(("security".to_string(), "reality".to_string()));
        if let Some(public_key) = tls
            .get("reality")
            .and_then(Value::as_object)
            .and_then(|reality| reality.get("public_key"))
            .and_then(Value::as_str)
        {
            params.push(("pbk".to_string(), public_key.to_string()));
        }
        if let Some(short_id) = tls
            .get("reality")
            .and_then(Value::as_object)
            .and_then(|reality| pick_first_short_id(reality.get("short_id")))
        {
            params.push(("sid".to_string(), short_id));
        }
    } else {
        params.push(("security".to_string(), "tls".to_string()));
        if tls.get("insecure").and_then(Value::as_bool).unwrap_or(false) {
            params.push((insecure_key.to_string(), "1".to_string()));
        }
        if tls.get("disable_sni").and_then(Value::as_bool).unwrap_or(false) {
            params.push(("disable_sni".to_string(), "1".to_string()));
        }
    }
    if let Some(fingerprint) = tls
        .get("utls")
        .and_then(Value::as_object)
        .and_then(|utls| utls.get("fingerprint"))
        .and_then(Value::as_str)
    {
        params.push(("fp".to_string(), fingerprint.to_string()));
    }
    if let Some(sni) = tls.get("server_name").and_then(Value::as_str) {
        params.push(("sni".to_string(), sni.to_string()));
    }
    if let Some(alpn) = value_array_to_csv_opt(tls.get("alpn")) {
        params.push(("alpn".to_string(), alpn));
    }
}

fn add_params(uri: &str, params: Vec<(String, String)>, remark: &str) -> String {
    if params.is_empty() && remark.is_empty() {
        return uri.to_string();
    }

    let query = params
        .into_iter()
        .map(|(key, value)| {
            if matches!(key.as_str(), "mport" | "alpn") {
                format!("{key}={value}")
            } else {
                format!("{key}={}", encode_uri_component(&value))
            }
        })
        .collect::<Vec<_>>()
        .join("&");

    let mut result = uri.to_string();
    if !query.is_empty() {
        result.push('?');
        result.push_str(&query);
    }
    if !remark.is_empty() {
        result.push('#');
        result.push_str(&encode_uri_component(remark));
    }
    result
}

fn encode_uri_component(value: &str) -> String {
    byte_serialize(value.as_bytes()).collect()
}

fn config_object<'a>(value: Option<&'a Value>, label: &str) -> AppResult<&'a Map<String, Value>> {
    value
        .and_then(Value::as_object)
        .ok_or_else(|| AppError::Validation(format!("{label} config must be a JSON object")))
}

fn get_required_string(object: &Map<String, Value>, key: &str) -> AppResult<String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| AppError::Validation(format!("missing string field {key}")))
}

fn get_optional_string(object: &Map<String, Value>, key: &str) -> Option<String> {
    object.get(key).and_then(Value::as_str).map(ToOwned::to_owned)
}

fn number_to_string(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::Number(number)) => {
            if let Some(value) = number.as_i64() {
                Some(value.to_string())
            } else {
                number.as_u64().map(|value| value.to_string())
            }
        }
        _ => None,
    }
}

fn value_array_to_csv(value: Option<&Value>) -> Option<String> {
    value_array_to_csv_opt(value)
}

fn value_array_to_csv_opt(value: Option<&Value>) -> Option<String> {
    let array = value?.as_array()?;
    let parts = array.iter().filter_map(value_to_string).collect::<Vec<_>>();
    if parts.is_empty() { None } else { Some(parts.join(",")) }
}

fn value_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn pick_first_short_id(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(value)) if !value.is_empty() => Some(value.clone()),
        Some(Value::Array(values)) => values.iter().find_map(value_to_string),
        _ => None,
    }
}

pub fn split_host_port(host: &str) -> String {
    if let Ok(url) = Url::parse(&format!("http://{host}")) {
        if let Some(parsed_host) = url.host_str() {
            if parsed_host.contains(':') {
                return format!("[{parsed_host}]");
            }
            return parsed_host.to_string();
        }
    }

    if let Some(stripped) =
        host.strip_prefix('[').and_then(|value| value.split_once(']').map(|(left, _)| left))
    {
        return format!("[{stripped}]");
    }

    if let Some((left, right)) = host.rsplit_once(':') {
        if right.chars().all(|ch| ch.is_ascii_digit()) && !left.contains(':') {
            return left.to_string();
        }
    }

    host.to_string()
}

pub fn decode_base64_or_plain(value: &str) -> Cow<'_, str> {
    let trimmed = value.trim();
    for engine in [STANDARD, URL_SAFE, URL_SAFE_NO_PAD] {
        if let Ok(decoded) = engine.decode(trimmed) {
            if let Ok(decoded) = String::from_utf8(decoded) {
                return Cow::Owned(decoded);
            }
        }
    }
    Cow::Borrowed(trimmed)
}

fn subscription_headers(client: &ClientRow, update_interval: i64) -> SubscriptionHeaders {
    SubscriptionHeaders {
        userinfo: format!(
            "upload={}; download={}; total={}; expire={}",
            client.up, client.down, client.volume, client.expiry
        ),
        update_interval: update_interval.to_string(),
        title: client.name.clone(),
    }
}

fn build_client_info(client: &ClientRow) -> String {
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let mut parts = Vec::new();
    let remaining = client.volume - (client.up + client.down);
    if remaining > 0 {
        parts.push(format!("{}📊", format_traffic(remaining)));
    }
    if client.expiry > 0 {
        parts.push(format!("{}Days⏳", (client.expiry - now) / 86_400));
    }
    if parts.is_empty() { " ♾".to_string() } else { format!(" {}", parts.join(" ")) }
}

fn format_traffic(bytes: i64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    const TB: f64 = GB * 1024.0;
    const EB: f64 = TB * 1024.0;

    let value = bytes as f64;
    if value < KB {
        format!("{value:.2}B")
    } else if value < MB {
        format!("{:.2}KB", value / KB)
    } else if value < GB {
        format!("{:.2}MB", value / MB)
    } else if value < TB {
        format!("{:.2}GB", value / GB)
    } else if value < EB {
        format!("{:.2}TB", value / TB)
    } else {
        format!("{:.2}EB", value / EB)
    }
}

fn add_client_info(uri: &str, client_info: &str) -> AppResult<String> {
    if client_info.is_empty() {
        return Ok(uri.to_string());
    }
    let Some((scheme, payload)) = uri.split_once("://") else {
        return Ok(uri.to_string());
    };
    if scheme != "vmess" {
        return Ok(format!("{uri}{client_info}"));
    }

    let decoded =
        STANDARD.decode(payload).map_err(|error| AppError::Validation(error.to_string()))?;
    let mut value: Value = serde_json::from_slice(&decoded)?;
    let Some(object) = value.as_object_mut() else {
        return Ok(uri.to_string());
    };
    let current = object.get("ps").and_then(Value::as_str).unwrap_or_default().to_string();
    object.insert("ps".to_string(), Value::String(format!("{current}{client_info}")));
    Ok(format!("vmess://{}", STANDARD.encode(serde_json::to_vec_pretty(&value)?)))
}

fn parse_json_text(raw: &str, fallback: Value) -> AppResult<Value> {
    if raw.trim().is_empty() {
        return Ok(fallback);
    }
    serde_json::from_str(raw).map_err(Into::into)
}

fn parse_i64_array(raw: &str) -> AppResult<Vec<i64>> {
    let value = parse_json_text(raw, Value::Array(Vec::new()))?;
    let Some(values) = value.as_array() else {
        return Err(AppError::Validation("expected JSON array of numeric ids".to_string()));
    };
    values
        .iter()
        .map(|value| {
            value.as_i64().ok_or_else(|| AppError::Validation("expected numeric id".to_string()))
        })
        .collect()
}

fn filter_subscription_inbounds(
    inbounds: Vec<InboundLinkInput>,
    inbound_ref: Option<&str>,
) -> AppResult<Vec<InboundLinkInput>> {
    let Some(inbound_ref) = inbound_ref.filter(|value| !value.is_empty()) else {
        return Ok(inbounds);
    };

    if let Ok(id) = inbound_ref.parse::<i64>() {
        if let Some(inbound) = inbounds.iter().find(|inbound| inbound.id == id) {
            return Ok(vec![inbound.clone()]);
        }
    }
    if let Some(inbound) = inbounds.iter().find(|inbound| inbound.tag == inbound_ref) {
        return Ok(vec![inbound.clone()]);
    }
    Err(AppError::NotFound(format!("inbound \"{inbound_ref}\" not found in subscription")))
}

fn build_local_outbounds(
    client_config: &Value,
    inbounds: &[InboundLinkInput],
) -> AppResult<(Vec<Value>, Vec<String>)> {
    let client_config = client_config
        .as_object()
        .ok_or_else(|| AppError::Validation("client config must be an object".to_string()))?;
    let mut outbounds = Vec::new();
    let mut out_tags = Vec::new();

    for inbound in inbounds {
        let mut outbound = inbound.out_json.as_object().cloned().unwrap_or_default();
        if outbound.len() < 2 {
            continue;
        }
        let options = inbound
            .options
            .as_object()
            .ok_or_else(|| AppError::Validation("inbound options must be an object".to_string()))?;
        let protocol = outbound
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or(inbound.kind.as_str())
            .to_string();

        if protocol == "shadowsocks" {
            let method = options.get("method").and_then(Value::as_str).unwrap_or_default();
            let mut passwords = Vec::new();
            if method.starts_with("2022") {
                if let Some(password) = options.get("password").and_then(Value::as_str) {
                    passwords.push(password.to_string());
                }
            }
            let config_key =
                if method == "2022-blake3-aes-128-gcm" { "shadowsocks16" } else { "shadowsocks" };
            let password = client_config
                .get(config_key)
                .and_then(Value::as_object)
                .and_then(|config| config.get("password"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            passwords.push(password);
            outbound.insert("password".to_string(), Value::String(passwords.join(":")));
        } else if let Some(config) = client_config.get(&protocol).and_then(Value::as_object) {
            for (key, value) in config {
                if key == "name" || key == "alterId" || (key == "flow" && inbound.tls_id == 0) {
                    continue;
                }
                outbound.insert(key.clone(), value.clone());
            }
        }

        let addrs = inbound.addrs.as_array().cloned().unwrap_or_default();
        let tag = outbound.get("tag").and_then(Value::as_str).unwrap_or_default().to_string();
        if addrs.is_empty() {
            if let Some(server) = subscription_server_override(options) {
                outbound.insert("server".to_string(), Value::String(server.to_string()));
            }
            let value = Value::Object(outbound);
            if protocol == "mixed" {
                push_mixed_outbounds(&mut outbounds, &mut out_tags, value)?;
            } else {
                out_tags.push(tag);
                outbounds.push(value);
            }
        } else {
            for (index, addr) in addrs.into_iter().enumerate() {
                let Some(addr) = addr.as_object() else {
                    continue;
                };
                let mut cloned = outbound.clone();
                if let Some(server) = addr.get("server").and_then(Value::as_str) {
                    cloned.insert("server".to_string(), Value::String(server.to_string()));
                }
                if let Some(port) = addr.get("server_port") {
                    cloned.insert("server_port".to_string(), port.clone());
                }
                if let Some(addr_tls) = addr.get("tls").and_then(Value::as_object) {
                    let mut merged_tls =
                        cloned.get("tls").and_then(Value::as_object).cloned().unwrap_or_default();
                    for (key, value) in addr_tls {
                        merged_tls.insert(key.clone(), value.clone());
                    }
                    cloned.insert("tls".to_string(), Value::Object(merged_tls));
                }
                let remark = addr.get("remark").and_then(Value::as_str).unwrap_or_default();
                let new_tag = format!("{}.{}{}", index + 1, tag, remark);
                cloned.insert("tag".to_string(), Value::String(new_tag.clone()));
                let value = Value::Object(cloned);
                if protocol == "mixed" {
                    push_mixed_outbounds(&mut outbounds, &mut out_tags, value)?;
                } else {
                    out_tags.push(new_tag);
                    outbounds.push(value);
                }
            }
        }
    }

    Ok((outbounds, out_tags))
}

fn push_mixed_outbounds(
    outbounds: &mut Vec<Value>,
    out_tags: &mut Vec<String>,
    outbound: Value,
) -> AppResult<()> {
    let object = outbound
        .as_object()
        .ok_or_else(|| AppError::Validation("mixed outbound must be an object".to_string()))?;
    let tag = object.get("tag").and_then(Value::as_str).unwrap_or_default().to_string();
    let mut socks = object.clone();
    socks.insert("type".to_string(), Value::String("socks".to_string()));
    socks.insert("tag".to_string(), Value::String(format!("{tag}-socks")));
    let mut http = object.clone();
    http.insert("type".to_string(), Value::String("http".to_string()));
    http.insert("tag".to_string(), Value::String(format!("{tag}-http")));
    outbounds.push(Value::Object(socks));
    outbounds.push(Value::Object(http));
    out_tags.push(format!("{tag}-socks"));
    out_tags.push(format!("{tag}-http"));
    Ok(())
}

fn add_default_outbounds(outbounds: &mut Vec<Value>, out_tags: &[String]) {
    let mut selector_outbounds = vec!["auto".to_string(), "direct".to_string()];
    selector_outbounds.extend(out_tags.iter().cloned());
    let mut prefix = vec![
        json!({
            "outbounds": selector_outbounds,
            "tag": "proxy",
            "type": "selector",
        }),
        json!({
            "tag": "auto",
            "type": "urltest",
            "outbounds": out_tags,
            "url": "http://www.gstatic.com/generate_204",
            "interval": "10m",
            "tolerance": 50,
        }),
        json!({
            "type": "direct",
            "tag": "direct",
        }),
    ];
    prefix.append(outbounds);
    *outbounds = prefix;
}

fn add_home_proxy_outbounds(outbounds: &mut Vec<Value>, out_tags: &[String]) {
    if out_tags.is_empty() {
        return;
    }
    let mut selector_outbounds = out_tags.to_vec();
    let mut prefix = Vec::new();
    if out_tags.len() > 1 {
        prefix.push(json!({
            "tag": "auto",
            "type": "urltest",
            "outbounds": out_tags,
            "url": "http://www.gstatic.com/generate_204",
            "interval": "10m",
            "tolerance": 50,
        }));
        selector_outbounds.insert(0, "auto".to_string());
    }
    prefix.insert(
        0,
        json!({
            "outbounds": selector_outbounds,
            "tag": "proxy",
            "type": "selector",
        }),
    );
    prefix.append(outbounds);
    *outbounds = prefix;
}

fn supports_proxy_home(kind: &str) -> bool {
    matches!(
        kind,
        "mixed"
            | "socks"
            | "http"
            | "shadowsocks"
            | "vmess"
            | "trojan"
            | "naive"
            | "hysteria"
            | "shadowtls"
            | "tuic"
            | "hysteria2"
            | "vless"
            | "anytls"
    )
}

fn has_proxy_home_enabled(inbounds: &[InboundLinkInput]) -> bool {
    inbounds.iter().any(|inbound| supports_proxy_home(&inbound.kind) && inbound.proxy_home)
}

fn first_proxy_server(outbounds: &[Value]) -> Option<String> {
    outbounds.iter().find_map(|outbound| {
        let object = outbound.as_object()?;
        let kind = object.get("type").and_then(Value::as_str)?;
        if matches!(kind, "selector" | "urltest" | "direct") {
            return None;
        }
        object.get("server").and_then(Value::as_str).map(ToOwned::to_owned)
    })
}

fn build_home_proxy_dns(outbounds: &[Value]) -> Option<Value> {
    let server = first_proxy_server(outbounds)?;
    Some(json!({
        "servers": [{
            "tag": "proxy-dns",
            "type": "udp",
            "server": server,
            "server_port": 53,
            "detour": "proxy"
        }],
        "final": "proxy-dns"
    }))
}

fn merge_other_json_settings(root: &mut Map<String, Value>, raw: &str) -> AppResult<()> {
    let rules_start = vec![
        json!({ "action": "sniff" }),
        json!({
            "clash_mode": "Direct",
            "action": "route",
            "outbound": "direct"
        }),
    ];
    let rules_end = vec![json!({
        "clash_mode": "Global",
        "action": "route",
        "outbound": "proxy"
    })];
    let mut route = json!({
        "auto_detect_interface": true,
        "final": "proxy",
        "rules": rules_start.clone(),
    });
    if raw.trim().is_empty() {
        root.insert("route".to_string(), route);
        return Ok(());
    }
    let others = parse_json_text(raw, Value::Object(Map::new()))?;
    let others = others
        .as_object()
        .ok_or_else(|| AppError::Validation("subJsonExt must be a JSON object".to_string()))?;
    for key in ["log", "dns", "inbounds", "experimental"] {
        if let Some(value) = others.get(key) {
            root.insert(key.to_string(), value.clone());
        }
    }
    if let Some(rule_set) = others.get("rule_set") {
        route
            .as_object_mut()
            .expect("route object")
            .insert("rule_set".to_string(), rule_set.clone());
    }
    if let Some(rules) = others.get("rules").and_then(Value::as_array) {
        let mut merged = rules_start;
        merged.extend(rules.iter().cloned());
        merged.extend(rules_end);
        route
            .as_object_mut()
            .expect("route object")
            .insert("rules".to_string(), Value::Array(merged));
    }
    if let Some(resolver) = others.get("default_domain_resolver").and_then(Value::as_str) {
        route
            .as_object_mut()
            .expect("route object")
            .insert("default_domain_resolver".to_string(), Value::String(resolver.to_string()));
    }
    root.insert("route".to_string(), route);
    Ok(())
}

fn build_home_proxy_clash_config(outbounds: &[Value]) -> AppResult<String> {
    let mut output = json!({
        "mixed-port": 7890,
        "allow-lan": false,
        "mode": "global",
        "log-level": "info",
        "external-controller": "127.0.0.1:9090",
        "tun": {
            "enable": true,
            "stack": "system",
            "auto-route": true,
            "auto-detect-interface": true
        }
    });
    if let Some(server) = first_proxy_server(outbounds) {
        output.as_object_mut().expect("clash config object").insert(
            "dns".to_string(),
            json!({
                "enable": true,
                "ipv6": false,
                "nameserver": [format_clash_dns_server(&server)]
            }),
        );
    }
    serde_yaml::to_string(&output).map_err(|error| AppError::Validation(error.to_string()))
}

fn format_clash_dns_server(server: &str) -> String {
    if server.is_empty() {
        return String::new();
    }
    let server = if server.contains(':')
        && !server.contains('.')
        && !(server.starts_with('[') && server.ends_with(']'))
    {
        format!("[{server}]")
    } else {
        server.to_string()
    };
    format!("udp://{server}:53")
}

fn convert_to_clash_meta(outbounds: &[Value], basic_config: &str) -> AppResult<String> {
    let mut proxies = Vec::new();
    let mut proxy_tags = Vec::new();

    for outbound in outbounds {
        let Some(ob) = outbound.as_object() else {
            continue;
        };
        let kind = ob.get("type").and_then(Value::as_str).unwrap_or_default();
        if matches!(kind, "selector" | "urltest" | "direct") {
            continue;
        }
        let mut proxy = Map::new();
        proxy.insert(
            "name".to_string(),
            Value::String(ob.get("tag").and_then(Value::as_str).unwrap_or_default().to_string()),
        );
        proxy.insert("type".to_string(), Value::String(kind.to_string()));
        if let Some(server) = ob.get("server").and_then(Value::as_str) {
            let server = if server.contains(':')
                && !server.contains('.')
                && !(server.starts_with('[') && server.ends_with(']'))
            {
                format!("[{server}]")
            } else {
                server.to_string()
            };
            proxy.insert("server".to_string(), Value::String(server));
        }
        if let Some(port) = ob.get("server_port") {
            proxy.insert("port".to_string(), port.clone());
        }

        match kind {
            "vmess" | "vless" | "tuic" => {
                if let Some(uuid) = ob.get("uuid") {
                    proxy.insert("uuid".to_string(), uuid.clone());
                }
                if kind == "vmess" {
                    proxy.insert("alterId".to_string(), json!(0));
                    proxy.insert("cipher".to_string(), Value::String("auto".to_string()));
                }
                if kind == "vless" {
                    if let Some(flow) = ob.get("flow") {
                        proxy.insert("flow".to_string(), flow.clone());
                    }
                }
                if kind == "tuic" {
                    if let Some(password) = ob.get("password") {
                        proxy.insert("password".to_string(), password.clone());
                    }
                    if let Some(congestion) = ob.get("congestion_control") {
                        proxy.insert("congestion-controller".to_string(), congestion.clone());
                    }
                }
            }
            "trojan" => {
                if let Some(password) = ob.get("password") {
                    proxy.insert("password".to_string(), password.clone());
                }
            }
            "socks" | "http" => {
                if kind == "socks" {
                    proxy.insert("type".to_string(), Value::String("socks5".to_string()));
                }
                for key in ["username", "password"] {
                    if let Some(value) = ob.get(key) {
                        proxy.insert(key.to_string(), value.clone());
                    }
                }
            }
            "hysteria" | "hysteria2" => {
                if let Some(up) = ob.get("up_mbps") {
                    proxy.insert("up".to_string(), up.clone());
                }
                if let Some(down) = ob.get("down_mbps") {
                    proxy.insert("down".to_string(), down.clone());
                }
                if kind == "hysteria" {
                    if let Some(auth) = ob.get("auth_str") {
                        proxy.insert("auth-str".to_string(), auth.clone());
                    }
                    if let Some(obfs) = ob.get("obfs") {
                        proxy.insert("obfs".to_string(), obfs.clone());
                    }
                } else {
                    if let Some(password) = ob.get("password") {
                        proxy.insert("password".to_string(), password.clone());
                    }
                    if let Some(obfs) = ob.get("obfs").and_then(Value::as_object) {
                        if let Some(kind) = obfs.get("type") {
                            proxy.insert("obfs".to_string(), kind.clone());
                        }
                        if let Some(password) = obfs.get("password") {
                            proxy.insert("obfs-password".to_string(), password.clone());
                        }
                    }
                }
            }
            "anytls" => {
                if let Some(password) = ob.get("password") {
                    proxy.insert("password".to_string(), password.clone());
                }
            }
            "shadowsocks" => {
                proxy.insert("type".to_string(), Value::String("ss".to_string()));
                if let Some(cipher) = ob.get("method") {
                    proxy.insert("cipher".to_string(), cipher.clone());
                }
                if let Some(password) = ob.get("password") {
                    proxy.insert("password".to_string(), password.clone());
                }
            }
            _ => continue,
        }

        if let Some(tls) = ob.get("tls").and_then(Value::as_object) {
            apply_clash_tls_settings(&mut proxy, kind, tls);
        }

        if let Some(transport) = ob.get("transport").and_then(Value::as_object) {
            match transport.get("type").and_then(Value::as_str).unwrap_or_default() {
                "http" => {
                    if ob
                        .get("tls")
                        .and_then(Value::as_object)
                        .and_then(|tls| tls.get("enabled"))
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                    {
                        proxy.insert("network".to_string(), Value::String("h2".to_string()));
                    } else {
                        proxy.insert("network".to_string(), Value::String("http".to_string()));
                    }
                }
                "ws" | "httpupgrade" => {
                    proxy.insert("network".to_string(), Value::String("ws".to_string()));
                }
                "grpc" => {
                    proxy.insert("network".to_string(), Value::String("grpc".to_string()));
                }
                _ => {}
            }
        }

        proxies.push(Value::Object(proxy));
        if let Some(tag) = ob.get("tag").and_then(Value::as_str) {
            proxy_tags.push(tag.to_string());
        }
    }

    let mut proxy_groups: Vec<Value> = serde_yaml::from_str(PROXY_GROUPS)
        .map_err(|error| AppError::Validation(error.to_string()))?;
    if let Some(auto) = proxy_groups.get_mut(1).and_then(Value::as_object_mut) {
        auto.insert("proxies".to_string(), json!(proxy_tags));
    }
    if let Some(proxy) = proxy_groups.get_mut(0).and_then(Value::as_object_mut) {
        let mut values = vec![Value::String("Auto".to_string())];
        values.extend(proxy_tags.iter().cloned().map(Value::String));
        proxy.insert("proxies".to_string(), Value::Array(values));
    }

    let mut output: Value = serde_yaml::from_str(basic_config)
        .map_err(|error| AppError::Validation(error.to_string()))?;
    let root = output
        .as_object_mut()
        .ok_or_else(|| AppError::Validation("clash config must be a map".to_string()))?;

    match root.get_mut("proxies").and_then(Value::as_array_mut) {
        Some(existing) => existing.extend(proxies),
        None => {
            root.insert("proxies".to_string(), Value::Array(proxies));
        }
    }
    match root.get_mut("proxy-groups").and_then(Value::as_array_mut) {
        Some(existing) => existing.extend(proxy_groups),
        None => {
            root.insert("proxy-groups".to_string(), Value::Array(proxy_groups));
        }
    }

    serde_yaml::to_string(&output).map_err(|error| AppError::Validation(error.to_string()))
}

fn apply_clash_tls_settings(proxy: &mut Map<String, Value>, kind: &str, tls: &Map<String, Value>) {
    if !tls.get("enabled").and_then(Value::as_bool).unwrap_or(false) {
        return;
    }

    proxy.insert("tls".to_string(), Value::Bool(true));

    if let Some(alpn) = tls.get("alpn") {
        proxy.insert("alpn".to_string(), alpn.clone());
    }
    if let Some(server_name) = tls.get("server_name").and_then(Value::as_str) {
        let key = if matches!(kind, "vmess" | "vless") { "servername" } else { "sni" };
        proxy.insert(key.to_string(), Value::String(server_name.to_string()));
    }
    if tls.get("insecure").and_then(Value::as_bool).unwrap_or(false) {
        proxy.insert("skip-cert-verify".to_string(), Value::Bool(true));
    }

    if let Some(fingerprint) = tls
        .get("utls")
        .and_then(Value::as_object)
        .and_then(|utls| utls.get("fingerprint"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        proxy.insert("client-fingerprint".to_string(), Value::String(fingerprint.to_string()));
    }

    let Some(reality) = tls.get("reality").and_then(Value::as_object) else {
        return;
    };
    if !reality.get("enabled").and_then(Value::as_bool).unwrap_or(false) {
        return;
    }

    let mut reality_opts = Map::new();
    if let Some(public_key) =
        reality.get("public_key").and_then(Value::as_str).filter(|value| !value.is_empty())
    {
        reality_opts.insert("public-key".to_string(), Value::String(public_key.to_string()));
    }
    if let Some(short_id) = pick_first_short_id(reality.get("short_id")) {
        reality_opts.insert("short-id".to_string(), Value::String(short_id));
    }
    if !reality_opts.is_empty() {
        proxy.insert("reality-opts".to_string(), Value::Object(reality_opts));
    }
}

#[cfg(test)]
mod tests {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde_json::{Value, json};

    use super::{
        BASIC_CLASH_CONFIG, InboundLinkInput, TlsBundle, add_client_info, build_local_outbounds,
        convert_to_clash_meta, generate_links, prepare_tls,
    };

    #[test]
    fn prepares_tls_bundle_like_go_generator() {
        let bundle = TlsBundle {
            server: json!({
                "enabled": true,
                "server_name": "edge.example",
                "alpn": ["h2", "http/1.1"],
                "reality": {
                    "enabled": true,
                    "public_key": "pub-key",
                    "short_id": ["abcd", "efgh"],
                    "handshake": {
                        "server": "fallback.example"
                    }
                }
            }),
            client: json!({
                "utls": {
                    "enabled": true,
                    "fingerprint": "chrome"
                },
                "reality": {}
            }),
        };

        let prepared = prepare_tls(&bundle).expect("prepare tls").expect("tls output");
        assert_eq!(prepared["server_name"], "edge.example");
        assert_eq!(prepared["utls"]["fingerprint"], "chrome");
        assert_eq!(prepared["reality"]["short_id"], "abcd");
    }

    #[test]
    fn generates_vless_links_in_old_client_compatible_format() {
        let links = generate_links(
            &json!({
                "vless": {
                    "uuid": "11111111-1111-1111-1111-111111111111",
                    "flow": "xtls-rprx-vision"
                }
            }),
            &InboundLinkInput {
                id: 1,
                kind: "vless".to_string(),
                tag: "demo".to_string(),
                proxy_home: false,
                tls_id: 1,
                tls: Some(TlsBundle {
                    server: json!({
                        "enabled": true,
                        "server_name": "edge.example",
                        "alpn": ["h2", "http/1.1"]
                    }),
                    client: json!({
                        "utls": {
                            "enabled": true,
                            "fingerprint": "chrome"
                        }
                    }),
                }),
                addrs: json!([{
                    "server": "demo.example",
                    "server_port": 443,
                    "remark": "-node"
                }]),
                out_json: json!({}),
                options: json!({
                    "transport": {
                        "type": "ws",
                        "path": "/ws",
                        "headers": {
                            "Host": "cdn.example"
                        }
                    }
                }),
            },
            "panel.example",
        )
        .expect("generate links");

        assert_eq!(
            links,
            vec![
                "vless://11111111-1111-1111-1111-111111111111@demo.example:443?type=ws&path=%2Fws&host=cdn.example&security=tls&fp=chrome&sni=edge.example&alpn=h2,http/1.1&flow=xtls-rprx-vision#demo-node"
            ]
        );
    }

    #[test]
    fn generates_trojan_links_with_reality_params_in_go_order() {
        let links = generate_links(
            &json!({
                "trojan": {
                    "password": "secret"
                }
            }),
            &InboundLinkInput {
                id: 1,
                kind: "trojan".to_string(),
                tag: "home".to_string(),
                proxy_home: false,
                tls_id: 1,
                tls: Some(TlsBundle {
                    server: json!({
                        "enabled": true,
                        "reality": {
                            "enabled": true,
                            "public_key": "public-key",
                            "short_id": ["a1b2"],
                            "handshake": {
                                "server": "reality.example"
                            }
                        }
                    }),
                    client: json!({
                        "utls": {
                            "enabled": true,
                            "fingerprint": "chrome"
                        },
                        "reality": {}
                    }),
                }),
                addrs: json!([{
                    "server": "node.example",
                    "server_port": 8443,
                    "remark": "-main"
                }]),
                out_json: json!({}),
                options: json!({
                    "transport": {
                        "type": "tcp"
                    }
                }),
            },
            "panel.example",
        )
        .expect("generate links");

        assert_eq!(
            links,
            vec![
                "trojan://secret@node.example:8443?type=tcp&security=reality&pbk=public-key&sid=a1b2&fp=chrome&sni=reality.example#home-main"
            ]
        );
    }

    #[test]
    fn uses_subscription_server_when_inbound_has_no_extra_addresses() {
        let links = generate_links(
            &json!({
                "vless": {
                    "uuid": "33333333-3333-3333-3333-333333333333",
                    "flow": "xtls-rprx-vision"
                }
            }),
            &InboundLinkInput {
                id: 9,
                kind: "vless".to_string(),
                tag: "edge-node".to_string(),
                proxy_home: false,
                tls_id: 0,
                tls: None,
                addrs: json!([]),
                out_json: json!({}),
                options: json!({
                    "listen_port": 2443,
                    "subscribe_server": "edge.example.com"
                }),
            },
            "10.10.10.210",
        )
        .expect("generate links");

        assert_eq!(
            links,
            vec![
                "vless://33333333-3333-3333-3333-333333333333@edge.example.com:2443?type=tcp#edge-node"
            ]
        );
    }

    #[test]
    fn json_subscription_uses_subscription_server_override() {
        let (outbounds, out_tags) = build_local_outbounds(
            &json!({
                "vless": {
                    "uuid": "33333333-3333-3333-3333-333333333333",
                    "flow": "xtls-rprx-vision"
                }
            }),
            &[InboundLinkInput {
                id: 9,
                kind: "vless".to_string(),
                tag: "edge-node".to_string(),
                proxy_home: false,
                tls_id: 0,
                tls: None,
                addrs: json!([]),
                out_json: json!({
                    "type": "vless",
                    "tag": "edge-node",
                    "server": "10.10.10.210",
                    "server_port": 2443
                }),
                options: json!({
                    "listen_port": 2443,
                    "subscribe_server": "edge.example.com"
                }),
            }],
        )
        .expect("build local outbounds");

        assert_eq!(out_tags, vec!["edge-node"]);
        assert_eq!(outbounds.len(), 1);
        assert_eq!(outbounds[0]["server"], "edge.example.com");
        assert_eq!(outbounds[0]["server_port"], 2443);
    }

    #[test]
    fn clash_subscription_preserves_reality_fields() {
        let rendered = convert_to_clash_meta(
            &[json!({
                "type": "vless",
                "tag": "reality-node",
                "server": "10.10.10.210",
                "server_port": 29974,
                "uuid": "7e27cf1f-7f77-4b68-a6b7-f3fd202569d4",
                "flow": "xtls-rprx-vision",
                "tls": {
                    "enabled": true,
                    "server_name": "nas.ytjungle.top",
                    "utls": {
                        "enabled": true,
                        "fingerprint": "chrome"
                    },
                    "reality": {
                        "enabled": true,
                        "public_key": "public-key",
                        "short_id": ""
                    }
                }
            })],
            BASIC_CLASH_CONFIG,
        )
        .expect("render clash config");

        let parsed: Value = serde_yaml::from_str(&rendered).expect("parse clash yaml");
        let proxy = parsed["proxies"][0].as_object().expect("first proxy object");
        assert_eq!(proxy["type"], "vless");
        assert_eq!(proxy["server"], "10.10.10.210");
        assert_eq!(proxy["port"], 29974);
        assert_eq!(proxy["servername"], "nas.ytjungle.top");
        assert_eq!(proxy["client-fingerprint"], "chrome");
        assert_eq!(proxy["reality-opts"]["public-key"], "public-key");
        assert!(proxy["reality-opts"].get("short-id").is_none());
    }

    #[test]
    fn vless_links_omit_empty_reality_short_id() {
        let links = generate_links(
            &json!({
                "vless": {
                    "uuid": "44444444-4444-4444-4444-444444444444",
                    "flow": "xtls-rprx-vision"
                }
            }),
            &InboundLinkInput {
                id: 3,
                kind: "vless".to_string(),
                tag: "lan".to_string(),
                proxy_home: false,
                tls_id: 1,
                tls: Some(TlsBundle {
                    server: json!({
                        "enabled": true,
                        "reality": {
                            "enabled": true,
                            "public_key": "public-key",
                            "short_id": ""
                        },
                        "server_name": "nas.ytjungle.top"
                    }),
                    client: json!({
                        "utls": {
                            "enabled": true,
                            "fingerprint": "chrome"
                        },
                        "reality": {}
                    }),
                }),
                addrs: json!([{
                    "server": "10.10.10.210",
                    "server_port": 29974
                }]),
                out_json: json!({}),
                options: json!({
                    "transport": {
                        "type": "tcp"
                    }
                }),
            },
            "10.10.10.210",
        )
        .expect("generate links");

        assert_eq!(
            links,
            vec![
                "vless://44444444-4444-4444-4444-444444444444@10.10.10.210:29974?type=tcp&security=reality&pbk=public-key&fp=chrome&sni=nas.ytjungle.top&flow=xtls-rprx-vision#lan"
            ]
        );
    }

    #[test]
    fn vmess_client_info_matches_go_pretty_encoded_payload() {
        let updated = add_client_info(
            &format!("vmess://{}", STANDARD.encode(r#"{"ps":"demo","add":"host","port":"443"}"#)),
            " TEST",
        )
        .expect("append client info");

        let payload = updated.strip_prefix("vmess://").expect("vmess prefix");
        let decoded =
            String::from_utf8(STANDARD.decode(payload).expect("decode base64")).expect("utf8");
        assert_eq!(
            decoded,
            "{\n  \"add\": \"host\",\n  \"port\": \"443\",\n  \"ps\": \"demo TEST\"\n}"
        );
    }
}
