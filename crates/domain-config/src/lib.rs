mod backup;
mod save;

use std::collections::BTreeMap;

use infra_db::Db;
use serde_json::{Map, Value, json};
use shared::{
    AppError, AppResult,
    model::{
        ChangeRow, ClientRow, EndpointRow, InboundRow, OutboundRow, ServiceRow, SettingRow, TlsRow,
    },
    settings::default_settings,
};
use time::OffsetDateTime;

#[derive(Clone)]
pub struct SettingsService {
    pool: Db,
}

impl SettingsService {
    pub fn new(pool: Db) -> Self {
        Self { pool }
    }

    pub async fn ensure_defaults(&self) -> AppResult<()> {
        for (key, value) in default_settings() {
            sqlx::query("INSERT OR IGNORE INTO settings (key, value) VALUES (?, ?)")
                .bind(key)
                .bind(value)
                .execute(&self.pool)
                .await?;
        }

        sqlx::query(
            "INSERT OR IGNORE INTO outbounds (id, kind, tag, options) VALUES (1, 'direct', 'direct', '{}')",
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn reset_defaults(&self) -> AppResult<()> {
        sqlx::query("DELETE FROM settings").execute(&self.pool).await?;
        self.ensure_defaults().await
    }

    pub async fn public_settings(&self) -> AppResult<BTreeMap<String, String>> {
        let rows =
            sqlx::query_as::<_, SettingRow>("SELECT id, key, value FROM settings ORDER BY key ASC")
                .fetch_all(&self.pool)
                .await?;

        let mut result = BTreeMap::new();
        for row in rows {
            if row.key == "config" || row.key == "version" {
                continue;
            }
            result.insert(row.key, row.value);
        }

        Ok(result)
    }

    pub async fn get_string(&self, key: &str) -> AppResult<String> {
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

    pub async fn get_int(&self, key: &str) -> AppResult<i64> {
        self.get_string(key)
            .await?
            .parse::<i64>()
            .map_err(|error| AppError::Validation(error.to_string()))
    }

    pub async fn get_bool(&self, key: &str) -> AppResult<bool> {
        match self.get_string(key).await?.as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            value => {
                Err(AppError::Validation(format!("setting {key} has invalid bool value {value}")))
            }
        }
    }

    pub async fn get_config(&self) -> AppResult<String> {
        self.get_string("config").await
    }

    pub async fn build_runtime_config(&self) -> AppResult<String> {
        let base_config = parse_json_text(&self.get_config().await?, Value::Object(Map::new()))?;
        let mut root = match base_config {
            Value::Object(object) => object,
            _ => {
                return Err(AppError::Validation("config root must be a JSON object".to_string()));
            }
        };

        let tls_servers = self.load_tls_servers().await?;
        let enabled_clients = self.load_enabled_clients().await?;
        let inbound_rows = self.load_inbound_rows().await?;
        root.insert(
            "inbounds".to_string(),
            Value::Array(
                inbound_rows
                    .iter()
                    .cloned()
                    .map(|row| runtime_inbound_to_value(row, &tls_servers, &enabled_clients))
                    .collect::<AppResult<Vec<_>>>()?,
            ),
        );
        inject_private_network_guards(&mut root, &inbound_rows)?;
        root.insert("outbounds".to_string(), Value::Array(self.load_runtime_outbounds().await?));
        root.insert(
            "services".to_string(),
            Value::Array(self.load_runtime_services(&tls_servers).await?),
        );
        root.insert("endpoints".to_string(), Value::Array(self.load_runtime_endpoints().await?));

        serde_json::to_string_pretty(&Value::Object(root)).map_err(Into::into)
    }

    pub async fn save_config(&self, config: &Value) -> AppResult<()> {
        let config_json = serde_json::to_string_pretty(config)?;
        sqlx::query("UPDATE settings SET value = ? WHERE key = 'config'")
            .bind(config_json)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn save_public_settings(&self, settings: &BTreeMap<String, String>) -> AppResult<()> {
        for (key, value) in settings {
            let normalized_value = if key == "webPath" || key == "subPath" {
                normalize_path_setting(value)
            } else {
                value.clone()
            };

            if key == "trafficAge" && normalized_value == "0" {
                sqlx::query("DELETE FROM stats").execute(&self.pool).await?;
            }

            sqlx::query("UPDATE settings SET value = ? WHERE key = ?")
                .bind(normalized_value)
                .bind(key)
                .execute(&self.pool)
                .await?;
        }

        Ok(())
    }

    pub async fn get_final_sub_uri(&self, host: &str) -> AppResult<String> {
        let settings = self.public_settings().await?;
        if let Some(sub_uri) = settings.get("subURI") {
            if !sub_uri.is_empty() {
                return Ok(sub_uri.clone());
            }
        }

        let protocol = if settings.get("subKeyFile").is_some_and(|value| !value.is_empty())
            && settings.get("subCertFile").is_some_and(|value| !value.is_empty())
        {
            "https"
        } else {
            "http"
        };

        let resolved_host = settings
            .get("subDomain")
            .filter(|value| !value.is_empty())
            .cloned()
            .unwrap_or_else(|| host.to_string());
        let sub_port = settings.get("subPort").cloned().unwrap_or_else(|| "2096".to_string());
        let sub_path = settings.get("subPath").cloned().unwrap_or_else(|| "/sub/".to_string());
        let port_suffix = if (sub_port == "80" && protocol == "http")
            || (sub_port == "443" && protocol == "https")
        {
            String::new()
        } else {
            format!(":{sub_port}")
        };

        Ok(format!("{protocol}://{resolved_host}{port_suffix}{sub_path}"))
    }

    pub async fn has_changes_since(&self, last_update: Option<i64>) -> AppResult<bool> {
        let Some(last_update) = last_update else {
            return Ok(true);
        };

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM changes WHERE date_time > ?")
            .bind(last_update)
            .fetch_one(&self.pool)
            .await?;
        Ok(count > 0)
    }

    pub async fn record_change(
        &self,
        actor: &str,
        key: &str,
        action: &str,
        obj: &Value,
    ) -> AppResult<()> {
        sqlx::query(
            "INSERT INTO changes (date_time, actor, key, action, obj) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(OffsetDateTime::now_utc().unix_timestamp())
        .bind(actor)
        .bind(key)
        .bind(action)
        .bind(serde_json::to_string(obj)?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_changes(
        &self,
        actor: Option<&str>,
        key: Option<&str>,
        count: i64,
    ) -> AppResult<Vec<ChangeRow>> {
        let mut query =
            String::from("SELECT id, date_time, actor, key, action, obj FROM changes WHERE id > 0");
        let mut bind_values: Vec<String> = Vec::new();

        if let Some(actor) = actor.filter(|value| !value.is_empty()) {
            query.push_str(" AND actor = ?");
            bind_values.push(actor.to_string());
        }
        if let Some(key) = key.filter(|value| !value.is_empty()) {
            query.push_str(" AND key = ?");
            bind_values.push(key.to_string());
        }
        query.push_str(" ORDER BY id DESC LIMIT ?");

        let mut statement = sqlx::query_as::<_, ChangeRow>(&query);
        for value in bind_values {
            statement = statement.bind(value);
        }

        let limit = if count > 0 { count } else { 20 };
        let rows = statement.bind(limit).fetch_all(&self.pool).await?;
        Ok(rows)
    }

    pub async fn panel_port(&self) -> AppResult<u16> {
        let port = self.get_int("webPort").await?;
        u16::try_from(port).map_err(|error| AppError::Validation(error.to_string()))
    }

    pub async fn subscription_port(&self) -> AppResult<u16> {
        let port = self.get_int("subPort").await?;
        u16::try_from(port).map_err(|error| AppError::Validation(error.to_string()))
    }

    pub async fn panel_path(&self) -> AppResult<String> {
        Ok(normalize_path_setting(&self.get_string("webPath").await?))
    }

    pub async fn subscription_path(&self) -> AppResult<String> {
        Ok(normalize_path_setting(&self.get_string("subPath").await?))
    }

    pub async fn session_max_age_minutes(&self) -> AppResult<i64> {
        self.get_int("sessionMaxAge").await
    }

    pub async fn traffic_age(&self) -> AppResult<i64> {
        self.get_int("trafficAge").await
    }

    pub async fn sub_updates(&self) -> AppResult<i64> {
        self.get_int("subUpdates").await
    }

    pub async fn sub_encode(&self) -> AppResult<bool> {
        self.get_bool("subEncode").await
    }

    pub async fn sub_show_info(&self) -> AppResult<bool> {
        self.get_bool("subShowInfo").await
    }

    pub async fn sub_json_ext(&self) -> AppResult<String> {
        self.get_string("subJsonExt").await
    }

    pub async fn sub_clash_ext(&self) -> AppResult<String> {
        self.get_string("subClashExt").await
    }

    pub async fn db_counts(&self) -> AppResult<BTreeMap<String, i64>> {
        let mut result = BTreeMap::new();
        for table in ["clients", "inbounds", "outbounds", "services", "endpoints"] {
            let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
                .fetch_one(&self.pool)
                .await?;
            result.insert(table.to_string(), count);
        }
        let client_up: i64 =
            sqlx::query_scalar("SELECT COALESCE(SUM(up + total_up), 0) FROM clients")
                .fetch_one(&self.pool)
                .await?;
        let client_down: i64 =
            sqlx::query_scalar("SELECT COALESCE(SUM(down + total_down), 0) FROM clients")
                .fetch_one(&self.pool)
                .await?;
        result.insert("clientUp".to_string(), client_up);
        result.insert("clientDown".to_string(), client_down);
        Ok(result)
    }

    pub async fn list_tls(&self) -> AppResult<Vec<Value>> {
        let rows =
            sqlx::query_as::<_, TlsRow>("SELECT id, name, server, client FROM tls ORDER BY id ASC")
                .fetch_all(&self.pool)
                .await?;

        rows.into_iter()
            .map(|row| {
                Ok(json!({
                    "id": row.id,
                    "name": row.name,
                    "server": parse_json_text(&row.server, Value::Object(Map::new()))?,
                    "client": parse_json_text(&row.client, Value::Object(Map::new()))?,
                }))
            })
            .collect()
    }

    pub async fn list_outbounds(&self) -> AppResult<Vec<Value>> {
        let rows = sqlx::query_as::<_, OutboundRow>(
            "SELECT id, kind, tag, options FROM outbounds ORDER BY id ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| merge_entity(row.id, &row.kind, &row.tag, &row.options))
            .collect()
    }

    pub async fn list_endpoints(&self) -> AppResult<Vec<Value>> {
        let rows = sqlx::query_as::<_, EndpointRow>(
            "SELECT id, kind, tag, options, ext FROM endpoints ORDER BY id ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                let mut value = merge_entity(row.id, &row.kind, &row.tag, &row.options)?;
                let object = value.as_object_mut().ok_or_else(|| {
                    AppError::Validation("endpoint payload must be an object".to_string())
                })?;
                object.insert(
                    "ext".to_string(),
                    parse_json_text(&row.ext, Value::Object(Map::new()))?,
                );
                Ok(value)
            })
            .collect()
    }

    pub async fn list_services(&self) -> AppResult<Vec<Value>> {
        let rows = sqlx::query_as::<_, ServiceRow>(
            "SELECT id, kind, tag, COALESCE(tls_id, 0) AS tls_id, options FROM services ORDER BY id ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                let mut value = merge_entity(row.id, &row.kind, &row.tag, &row.options)?;
                let object = value.as_object_mut().ok_or_else(|| {
                    AppError::Validation("service payload must be an object".to_string())
                })?;
                object.insert("tls_id".to_string(), json!(row.tls_id));
                Ok(value)
            })
            .collect()
    }

    pub async fn list_clients_summary(&self) -> AppResult<Vec<Value>> {
        let rows = sqlx::query_as::<_, ClientRow>(
            r#"
            SELECT
                id, enable, name, config, inbounds, links, volume, expiry, down, up, desc,
                group_name, delay_start, auto_reset, reset_days, next_reset, total_up, total_down
            FROM clients
            ORDER BY id ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                Ok(json!({
                    "id": row.id,
                    "enable": row.enable,
                    "name": row.name,
                    "desc": row.desc,
                    "group": row.group_name,
                    "inbounds": parse_json_text(&row.inbounds, Value::Array(Vec::new()))?,
                    "up": row.up,
                    "down": row.down,
                    "volume": row.volume,
                    "expiry": row.expiry,
                }))
            })
            .collect()
    }

    pub async fn list_clients_by_ids(&self, ids: &[i64]) -> AppResult<Vec<Value>> {
        let rows = if ids.is_empty() {
            sqlx::query_as::<_, ClientRow>(
                r#"
                SELECT
                    id, enable, name, config, inbounds, links, volume, expiry, down, up, desc,
                    group_name, delay_start, auto_reset, reset_days, next_reset, total_up, total_down
                FROM clients
                ORDER BY id ASC
                "#,
            )
            .fetch_all(&self.pool)
            .await?
        } else {
            let mut placeholders = String::new();
            for index in 0..ids.len() {
                if index > 0 {
                    placeholders.push_str(", ");
                }
                placeholders.push('?');
            }
            let query = format!(
                r#"
                SELECT
                    id, enable, name, config, inbounds, links, volume, expiry, down, up, desc,
                    group_name, delay_start, auto_reset, reset_days, next_reset, total_up, total_down
                FROM clients
                WHERE id IN ({placeholders})
                ORDER BY id ASC
                "#
            );
            let mut statement = sqlx::query_as::<_, ClientRow>(&query);
            for id in ids {
                statement = statement.bind(id);
            }
            statement.fetch_all(&self.pool).await?
        };

        rows.into_iter().map(client_to_value).collect()
    }

    pub async fn list_inbound_summaries(&self) -> AppResult<Vec<Value>> {
        let rows = sqlx::query_as::<_, InboundRow>(
            r#"
            SELECT id, kind, tag, allow_lan_access, COALESCE(tls_id, 0) AS tls_id, addrs, out_json, options
            FROM inbounds
            ORDER BY id ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        let clients = self.enabled_client_bindings().await?;
        rows.into_iter().map(|row| inbound_summary_to_value(&row, &clients)).collect()
    }

    pub async fn list_inbounds_by_ids(&self, ids: &[i64]) -> AppResult<Vec<Value>> {
        let rows = if ids.is_empty() {
            sqlx::query_as::<_, InboundRow>(
                r#"
                SELECT id, kind, tag, allow_lan_access, COALESCE(tls_id, 0) AS tls_id, addrs, out_json, options
                FROM inbounds
                ORDER BY id ASC
                "#,
            )
            .fetch_all(&self.pool)
            .await?
        } else {
            let mut placeholders = String::new();
            for index in 0..ids.len() {
                if index > 0 {
                    placeholders.push_str(", ");
                }
                placeholders.push('?');
            }
            let query = format!(
                r#"
                SELECT id, kind, tag, allow_lan_access, COALESCE(tls_id, 0) AS tls_id, addrs, out_json, options
                FROM inbounds
                WHERE id IN ({placeholders})
                ORDER BY id ASC
                "#
            );
            let mut statement = sqlx::query_as::<_, InboundRow>(&query);
            for id in ids {
                statement = statement.bind(id);
            }
            statement.fetch_all(&self.pool).await?
        };

        rows.into_iter().map(inbound_full_to_value).collect()
    }

    pub async fn load_dashboard_data(
        &self,
        host: &str,
        include_full_payload: bool,
    ) -> AppResult<Value> {
        let mut payload = Map::new();
        payload.insert(
            "onlines".to_string(),
            json!({
                "inbound": [],
                "outbound": [],
                "user": [],
            }),
        );

        if include_full_payload {
            let config = self.get_config().await?;
            payload
                .insert("config".to_string(), parse_json_text(&config, Value::Object(Map::new()))?);
            payload.insert("clients".to_string(), Value::Array(self.list_clients_summary().await?));
            payload.insert("tls".to_string(), Value::Array(self.list_tls().await?));
            payload
                .insert("inbounds".to_string(), Value::Array(self.list_inbound_summaries().await?));
            payload.insert("outbounds".to_string(), Value::Array(self.list_outbounds().await?));
            payload.insert("endpoints".to_string(), Value::Array(self.list_endpoints().await?));
            payload.insert("services".to_string(), Value::Array(self.list_services().await?));
            payload
                .insert("subURI".to_string(), Value::String(self.get_final_sub_uri(host).await?));
            payload.insert("enableTraffic".to_string(), Value::Bool(self.traffic_age().await? > 0));
        }

        Ok(Value::Object(payload))
    }

    async fn enabled_client_bindings(&self) -> AppResult<Vec<(String, Vec<i64>)>> {
        let rows = sqlx::query_as::<_, ClientRow>(
            r#"
            SELECT
                id, enable, name, config, inbounds, links, volume, expiry, down, up, desc,
                group_name, delay_start, auto_reset, reset_days, next_reset, total_up, total_down
            FROM clients
            WHERE enable = 1
            ORDER BY id ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                let ids = parse_i64_array(&row.inbounds)?;
                Ok((row.name, ids))
            })
            .collect()
    }

    async fn load_tls_servers(&self) -> AppResult<BTreeMap<i64, Map<String, Value>>> {
        let rows =
            sqlx::query_as::<_, TlsRow>("SELECT id, name, server, client FROM tls ORDER BY id ASC")
                .fetch_all(&self.pool)
                .await?;
        rows.into_iter()
            .map(|row| Ok((row.id, parse_json_object(&row.server, "tls server")?)))
            .collect()
    }

    async fn load_enabled_clients(&self) -> AppResult<Vec<ClientRow>> {
        sqlx::query_as::<_, ClientRow>(
            r#"
            SELECT
                id, enable, name, config, inbounds, links, volume, expiry, down, up, desc,
                group_name, delay_start, auto_reset, reset_days, next_reset, total_up, total_down
            FROM clients
            WHERE enable = 1
            ORDER BY id ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Into::into)
    }

    async fn load_inbound_rows(&self) -> AppResult<Vec<InboundRow>> {
        sqlx::query_as::<_, InboundRow>(
            r#"
            SELECT id, kind, tag, allow_lan_access, COALESCE(tls_id, 0) AS tls_id, addrs, out_json, options
            FROM inbounds
            ORDER BY id ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(Into::into)
    }

    async fn load_runtime_outbounds(&self) -> AppResult<Vec<Value>> {
        let rows = sqlx::query_as::<_, OutboundRow>(
            "SELECT id, kind, tag, options FROM outbounds ORDER BY id ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(runtime_outbound_to_value).collect()
    }

    async fn load_runtime_services(
        &self,
        tls_servers: &BTreeMap<i64, Map<String, Value>>,
    ) -> AppResult<Vec<Value>> {
        let rows = sqlx::query_as::<_, ServiceRow>(
            "SELECT id, kind, tag, COALESCE(tls_id, 0) AS tls_id, options FROM services ORDER BY id ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(|row| runtime_service_to_value(row, tls_servers)).collect()
    }

    async fn load_runtime_endpoints(&self) -> AppResult<Vec<Value>> {
        let rows = sqlx::query_as::<_, EndpointRow>(
            "SELECT id, kind, tag, options, ext FROM endpoints ORDER BY id ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(runtime_endpoint_to_value).collect()
    }
}

fn normalize_path_setting(value: &str) -> String {
    let mut normalized = value.trim().to_string();
    if !normalized.starts_with('/') {
        normalized.insert(0, '/');
    }
    if !normalized.ends_with('/') {
        normalized.push('/');
    }
    normalized
}

fn parse_json_text(raw: &str, fallback: Value) -> AppResult<Value> {
    if raw.trim().is_empty() {
        return Ok(fallback);
    }
    serde_json::from_str(raw).map_err(Into::into)
}

fn parse_json_object(raw: &str, field: &str) -> AppResult<Map<String, Value>> {
    match parse_json_text(raw, Value::Object(Map::new()))? {
        Value::Object(object) => Ok(object),
        _ => Err(AppError::Validation(format!("{field} must be a JSON object"))),
    }
}

fn merge_entity(id: i64, kind: &str, tag: &str, options_raw: &str) -> AppResult<Value> {
    let options_value = parse_json_text(options_raw, Value::Object(Map::new()))?;
    let mut object = match options_value {
        Value::Object(object) => object,
        _ => {
            return Err(AppError::Validation("entity options must be a JSON object".to_string()));
        }
    };
    object.insert("id".to_string(), json!(id));
    object.insert("type".to_string(), json!(kind));
    object.insert("tag".to_string(), json!(tag));
    Ok(Value::Object(object))
}

fn client_to_value(row: ClientRow) -> AppResult<Value> {
    Ok(json!({
        "id": row.id,
        "enable": row.enable,
        "name": row.name,
        "config": parse_json_text(&row.config, Value::Object(Map::new()))?,
        "inbounds": parse_json_text(&row.inbounds, Value::Array(Vec::new()))?,
        "links": parse_json_text(&row.links, Value::Array(Vec::new()))?,
        "volume": row.volume,
        "expiry": row.expiry,
        "down": row.down,
        "up": row.up,
        "desc": row.desc,
        "group": row.group_name,
        "delayStart": row.delay_start,
        "autoReset": row.auto_reset,
        "resetDays": row.reset_days,
        "nextReset": row.next_reset,
        "totalUp": row.total_up,
        "totalDown": row.total_down,
    }))
}

fn inbound_summary_to_value(row: &InboundRow, clients: &[(String, Vec<i64>)]) -> AppResult<Value> {
    let options = parse_json_text(&row.options, Value::Object(Map::new()))?;
    let mut object = Map::new();
    object.insert("id".to_string(), json!(row.id));
    object.insert("type".to_string(), json!(row.kind));
    object.insert("tag".to_string(), json!(row.tag));
    object.insert("proxy_home".to_string(), Value::Bool(row.allow_lan_access));
    object.insert("tls_id".to_string(), json!(row.tls_id));

    if let Some(options_object) = options.as_object() {
        if let Some(listen) = options_object.get("listen") {
            object.insert("listen".to_string(), listen.clone());
        }
        if let Some(listen_port) = options_object.get("listen_port") {
            object.insert("listen_port".to_string(), listen_port.clone());
        }
    }

    if inbound_has_users(row, &options) {
        let users = clients
            .iter()
            .filter(|(_, inbound_ids)| inbound_ids.contains(&row.id))
            .map(|(name, _)| Value::String(name.clone()))
            .collect::<Vec<_>>();
        object.insert("users".to_string(), Value::Array(users));
    }

    Ok(Value::Object(object))
}

fn inbound_full_to_value(row: InboundRow) -> AppResult<Value> {
    let mut value = merge_entity(row.id, &row.kind, &row.tag, &row.options)?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| AppError::Validation("inbound payload must be an object".to_string()))?;
    object.insert("proxy_home".to_string(), Value::Bool(row.allow_lan_access));
    object.insert("tls_id".to_string(), json!(row.tls_id));
    object.insert("addrs".to_string(), parse_json_text(&row.addrs, Value::Array(Vec::new()))?);
    object
        .insert("out_json".to_string(), parse_json_text(&row.out_json, Value::Object(Map::new()))?);
    Ok(value)
}

fn parse_i64_array(raw: &str) -> AppResult<Vec<i64>> {
    let value = parse_json_text(raw, Value::Array(Vec::new()))?;
    let Some(array) = value.as_array() else {
        return Err(AppError::Validation("expected JSON array of numeric ids".to_string()));
    };
    array
        .iter()
        .map(|value| {
            value.as_i64().ok_or_else(|| AppError::Validation("expected numeric id".to_string()))
        })
        .collect()
}

fn inbound_has_users(row: &InboundRow, options: &Value) -> bool {
    match row.kind.as_str() {
        "mixed" | "socks" | "http" | "vmess" | "trojan" | "naive" | "hysteria" | "tuic"
        | "hysteria2" | "vless" | "anytls" => true,
        "shadowtls" => options.get("version").and_then(Value::as_i64).unwrap_or_default() >= 3,
        "shadowsocks" => !options.get("managed").and_then(Value::as_bool).unwrap_or(false),
        _ => false,
    }
}

fn runtime_inbound_to_value(
    row: InboundRow,
    tls_servers: &BTreeMap<i64, Map<String, Value>>,
    clients: &[ClientRow],
) -> AppResult<Value> {
    let mut object = parse_json_object(&row.options, "inbound options")?;
    object.remove("allow_lan_access");
    object.remove("subscribe_server");
    object.insert("type".to_string(), Value::String(row.kind.clone()));
    object.insert("tag".to_string(), Value::String(row.tag.clone()));

    if let Some(tls) = tls_servers.get(&row.tls_id) {
        object.insert("tls".to_string(), Value::Object(tls.clone()));
    }

    if inbound_supports_users(&row.kind) {
        let users = build_runtime_inbound_users(&row, &object, clients)?;
        if !users.is_empty() || row.kind != "shadowtls" {
            object.insert("users".to_string(), Value::Array(users));
        }
    }

    Ok(Value::Object(object))
}

fn inject_private_network_guards(
    root: &mut Map<String, Value>,
    inbound_rows: &[InboundRow],
) -> AppResult<()> {
    let protected_inbounds = inbound_rows
        .iter()
        .filter(|row| inbound_needs_private_network_guard(row))
        .map(|row| Value::String(row.tag.clone()))
        .collect::<Vec<_>>();
    if protected_inbounds.is_empty() {
        return Ok(());
    }

    let route = root.entry("route".to_string()).or_insert_with(|| Value::Object(Map::new()));
    let route_object = route
        .as_object_mut()
        .ok_or_else(|| AppError::Validation("route must be a JSON object".to_string()))?;
    let existing_rules = route_object.remove("rules").unwrap_or_else(|| Value::Array(Vec::new()));
    let existing_rules = existing_rules
        .as_array()
        .cloned()
        .ok_or_else(|| AppError::Validation("route.rules must be a JSON array".to_string()))?;

    let mut rules = Vec::with_capacity(existing_rules.len() + 1);
    rules.push(json!({
        "inbound": protected_inbounds,
        "ip_is_private": true,
        "action": "reject",
    }));
    rules.extend(existing_rules);
    route_object.insert("rules".to_string(), Value::Array(rules));

    Ok(())
}

fn runtime_outbound_to_value(row: OutboundRow) -> AppResult<Value> {
    let mut object = parse_json_object(&row.options, "outbound options")?;
    object.insert("type".to_string(), Value::String(row.kind));
    object.insert("tag".to_string(), Value::String(row.tag));
    Ok(Value::Object(object))
}

fn runtime_service_to_value(
    row: ServiceRow,
    tls_servers: &BTreeMap<i64, Map<String, Value>>,
) -> AppResult<Value> {
    let mut object = parse_json_object(&row.options, "service options")?;
    object.insert("type".to_string(), Value::String(row.kind));
    object.insert("tag".to_string(), Value::String(row.tag));
    if let Some(tls) = tls_servers.get(&row.tls_id) {
        object.insert("tls".to_string(), Value::Object(tls.clone()));
    }
    Ok(Value::Object(object))
}

fn runtime_endpoint_to_value(row: EndpointRow) -> AppResult<Value> {
    let mut object = parse_json_object(&row.options, "endpoint options")?;
    let endpoint_type = if row.kind == "warp" { "wireguard" } else { row.kind.as_str() };
    object.insert("type".to_string(), Value::String(endpoint_type.to_string()));
    object.insert("tag".to_string(), Value::String(row.tag));
    Ok(Value::Object(object))
}

fn inbound_supports_users(kind: &str) -> bool {
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

fn inbound_needs_private_network_guard(row: &InboundRow) -> bool {
    !row.allow_lan_access && inbound_supports_proxy_home(&row.kind)
}

fn inbound_supports_proxy_home(kind: &str) -> bool {
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

fn build_runtime_inbound_users(
    row: &InboundRow,
    inbound: &Map<String, Value>,
    clients: &[ClientRow],
) -> AppResult<Vec<Value>> {
    if row.kind == "shadowtls"
        && inbound.get("version").and_then(Value::as_i64).unwrap_or_default() < 3
    {
        return Ok(Vec::new());
    }

    let config_key = if row.kind == "shadowsocks"
        && inbound.get("method").and_then(Value::as_str) == Some("2022-blake3-aes-128-gcm")
    {
        "shadowsocks16"
    } else {
        row.kind.as_str()
    };

    let mut users = Vec::new();
    for client in clients {
        let inbound_ids = parse_i64_array(&client.inbounds)?;
        if !inbound_ids.contains(&row.id) {
            continue;
        }

        let config = parse_json_object(&client.config, "client config")?;
        let Some(user) = config.get(config_key).cloned() else {
            continue;
        };
        let mut user = match user {
            Value::Object(object) => object,
            _ => continue,
        };

        if row.kind == "vless" && !inbound.contains_key("tls") {
            if let Some(Value::String(flow)) = user.get_mut("flow") {
                *flow = flow.replace("xtls-rprx-vision", "");
            }
        }

        users.push(Value::Object(user));
    }

    Ok(users)
}

#[cfg(test)]
mod tests {
    use super::{
        build_runtime_inbound_users, inject_private_network_guards, parse_json_object,
        runtime_endpoint_to_value, runtime_inbound_to_value, runtime_service_to_value,
    };
    use serde_json::{Map, Value, json};
    use shared::model::{ClientRow, EndpointRow, InboundRow, ServiceRow};
    use std::collections::BTreeMap;

    fn test_client(config: Value, inbounds: Value) -> ClientRow {
        ClientRow {
            id: 1,
            enable: true,
            name: "demo".to_string(),
            config: serde_json::to_string(&config).expect("client config"),
            inbounds: serde_json::to_string(&inbounds).expect("client inbound ids"),
            links: "[]".to_string(),
            volume: 0,
            expiry: 0,
            down: 0,
            up: 0,
            desc: String::new(),
            group_name: String::new(),
            delay_start: false,
            auto_reset: false,
            reset_days: 0,
            next_reset: 0,
            total_up: 0,
            total_down: 0,
        }
    }

    #[test]
    fn runtime_inbound_attaches_tls_and_users() {
        let row = InboundRow {
            id: 7,
            kind: "vless".to_string(),
            tag: "vless-33888".to_string(),
            allow_lan_access: true,
            tls_id: 2,
            addrs: "[]".to_string(),
            out_json: "{}".to_string(),
            options:
                r#"{"listen":"::","listen_port":33888,"transport":{},"allow_lan_access":true,"subscribe_server":"edge.example.com"}"#
                    .to_string(),
        };
        let mut tls_servers = BTreeMap::new();
        tls_servers.insert(
            2,
            parse_json_object(
                r#"{"enabled":true,"server_name":"nas.example","reality":{"enabled":true}}"#,
                "tls server",
            )
            .expect("tls object"),
        );
        let clients = vec![test_client(
            json!({"vless":{"name":"demo","uuid":"11111111-1111-1111-1111-111111111111","flow":"xtls-rprx-vision"}}),
            json!([7]),
        )];

        let value = runtime_inbound_to_value(row, &tls_servers, &clients).expect("runtime inbound");
        assert_eq!(value["type"], "vless");
        assert_eq!(value["tag"], "vless-33888");
        assert_eq!(value["listen_port"], 33888);
        assert!(value.get("allow_lan_access").is_none());
        assert!(value.get("subscribe_server").is_none());
        assert_eq!(value["tls"]["server_name"], "nas.example");
        assert_eq!(value["users"][0]["uuid"], "11111111-1111-1111-1111-111111111111");
    }

    #[test]
    fn runtime_vless_without_tls_clears_xtls_flow() {
        let row = InboundRow {
            id: 3,
            kind: "vless".to_string(),
            tag: "plain-vless".to_string(),
            allow_lan_access: false,
            tls_id: 0,
            addrs: "[]".to_string(),
            out_json: "{}".to_string(),
            options: r#"{"listen":"::","listen_port":443}"#.to_string(),
        };
        let inbound = parse_json_object(&row.options, "inbound options").expect("inbound options");
        let clients = vec![test_client(
            json!({"vless":{"name":"demo","uuid":"22222222-2222-2222-2222-222222222222","flow":"xtls-rprx-vision"}}),
            json!([3]),
        )];

        let users = build_runtime_inbound_users(&row, &inbound, &clients).expect("runtime users");
        assert_eq!(users[0]["flow"], "");
    }

    #[test]
    fn runtime_service_attaches_tls() {
        let row = ServiceRow {
            id: 1,
            kind: "derp".to_string(),
            tag: "derp-a".to_string(),
            tls_id: 9,
            options: r#"{"listen":"::","listen_port":4443}"#.to_string(),
        };
        let mut tls_servers = BTreeMap::new();
        tls_servers.insert(
            9,
            parse_json_object(r#"{"enabled":true,"server_name":"mesh.example"}"#, "tls server")
                .expect("tls object"),
        );

        let value = runtime_service_to_value(row, &tls_servers).expect("runtime service");
        assert_eq!(value["type"], "derp");
        assert_eq!(value["tls"]["server_name"], "mesh.example");
    }

    #[test]
    fn private_network_guard_is_added_for_non_proxy_home_inbounds() {
        let mut root = Map::new();
        root.insert(
            "route".to_string(),
            json!({
                "rules": [
                    { "action": "sniff" }
                ]
            }),
        );
        let inbounds = vec![
            InboundRow {
                id: 1,
                kind: "vless".to_string(),
                tag: "blocked".to_string(),
                allow_lan_access: false,
                tls_id: 0,
                addrs: "[]".to_string(),
                out_json: "{}".to_string(),
                options: "{}".to_string(),
            },
            InboundRow {
                id: 2,
                kind: "vless".to_string(),
                tag: "allowed".to_string(),
                allow_lan_access: true,
                tls_id: 0,
                addrs: "[]".to_string(),
                out_json: "{}".to_string(),
                options: "{}".to_string(),
            },
        ];

        inject_private_network_guards(&mut root, &inbounds).expect("inject private guards");

        let rules = root["route"]["rules"].as_array().expect("route rules");
        assert_eq!(rules[0]["action"], "reject");
        assert_eq!(rules[0]["ip_is_private"], true);
        assert_eq!(rules[0]["inbound"], json!(["blocked"]));
        assert_eq!(rules[1]["action"], "sniff");
    }

    #[test]
    fn private_network_guard_skips_non_proxy_home_types() {
        let mut root = Map::new();
        let inbounds = vec![InboundRow {
            id: 1,
            kind: "direct".to_string(),
            tag: "direct-in".to_string(),
            allow_lan_access: false,
            tls_id: 0,
            addrs: "[]".to_string(),
            out_json: "{}".to_string(),
            options: "{}".to_string(),
        }];

        inject_private_network_guards(&mut root, &inbounds).expect("inject private guards");

        assert!(root.get("route").is_none());
    }

    #[test]
    fn runtime_warp_endpoint_uses_wireguard_type() {
        let row = EndpointRow {
            id: 1,
            kind: "warp".to_string(),
            tag: "warp-out".to_string(),
            options: r#"{"address":["172.16.0.2/32"],"private_key":"k"}"#.to_string(),
            ext: "{}".to_string(),
        };

        let value = runtime_endpoint_to_value(row).expect("runtime endpoint");
        assert_eq!(value["type"], "wireguard");
        assert_eq!(value["tag"], "warp-out");
    }

    #[test]
    fn parse_json_object_rejects_non_object() {
        let error = parse_json_object("[]", "demo").expect_err("array must fail");
        assert_eq!(error.message(), "demo must be a JSON object");
    }

    #[test]
    fn build_runtime_inbound_users_skips_shadowtls_v2() {
        let row = InboundRow {
            id: 1,
            kind: "shadowtls".to_string(),
            tag: "shadowtls".to_string(),
            allow_lan_access: false,
            tls_id: 0,
            addrs: "[]".to_string(),
            out_json: "{}".to_string(),
            options: "{}".to_string(),
        };
        let mut inbound = Map::new();
        inbound.insert("version".to_string(), Value::from(2));
        let users = build_runtime_inbound_users(
            &row,
            &inbound,
            &[test_client(json!({"shadowtls":{"name":"demo","password":"secret"}}), json!([1]))],
        )
        .expect("shadowtls users");
        assert!(users.is_empty());
    }
}
