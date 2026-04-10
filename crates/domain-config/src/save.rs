use std::collections::BTreeSet;

use domain_subscription::{
    ClientLink, InboundLinkInput, TlsBundle, generate_links, prepare_tls, split_host_port,
};
use serde_json::{Map, Value, json};
use shared::{
    AppError, AppResult,
    model::{ClientRow, InboundRow, TlsRow},
};
use sqlx::{Sqlite, Transaction};
use time::OffsetDateTime;

use super::{SettingsService, parse_i64_array, parse_json_text};

impl SettingsService {
    pub async fn save_managed_object(
        &self,
        object: &str,
        action: &str,
        payload: &Value,
        init_users: Option<&str>,
        actor: &str,
        host: &str,
    ) -> AppResult<Value> {
        let normalized_host = split_host_port(host);
        let mut tx = self.pool.begin().await?;
        let reload = match object {
            "clients" => {
                self.save_clients_tx(&mut tx, action, payload, &normalized_host).await?;
                vec!["clients", "inbounds"]
            }
            "tls" => {
                self.save_tls_tx(&mut tx, action, payload, &normalized_host).await?;
                vec!["tls", "clients", "inbounds"]
            }
            "inbounds" => {
                self.save_inbounds_tx(&mut tx, action, payload, init_users, &normalized_host)
                    .await?;
                vec!["inbounds", "clients"]
            }
            "outbounds" => {
                self.save_outbounds_tx(&mut tx, action, payload).await?;
                vec!["outbounds"]
            }
            "services" => {
                self.save_services_tx(&mut tx, action, payload).await?;
                vec!["services"]
            }
            "endpoints" => {
                self.save_endpoints_tx(&mut tx, action, payload).await?;
                vec!["endpoints"]
            }
            _ => {
                return Err(AppError::Unsupported(format!("save: unsupported object {object}")));
            }
        };

        self.record_change_tx(&mut tx, actor, object, action, payload).await?;
        tx.commit().await?;
        self.load_partial_payload(&reload, &normalized_host).await
    }

    pub async fn load_partial_payload(&self, objects: &[&str], host: &str) -> AppResult<Value> {
        let mut payload = Map::new();
        for object in objects {
            match *object {
                "inbounds" => {
                    payload.insert(
                        "inbounds".to_string(),
                        Value::Array(self.list_inbound_summaries().await?),
                    );
                }
                "outbounds" => {
                    payload.insert(
                        "outbounds".to_string(),
                        Value::Array(self.list_outbounds().await?),
                    );
                }
                "endpoints" => {
                    payload.insert(
                        "endpoints".to_string(),
                        Value::Array(self.list_endpoints().await?),
                    );
                }
                "services" => {
                    payload
                        .insert("services".to_string(), Value::Array(self.list_services().await?));
                }
                "tls" => {
                    payload.insert("tls".to_string(), Value::Array(self.list_tls().await?));
                }
                "clients" => {
                    payload.insert(
                        "clients".to_string(),
                        Value::Array(self.list_clients_summary().await?),
                    );
                }
                "config" => {
                    let config = self.get_config().await?;
                    payload.insert(
                        "config".to_string(),
                        parse_json_text(&config, Value::Object(Map::new()))?,
                    );
                }
                "settings" => {
                    payload.insert("settings".to_string(), json!(self.public_settings().await?));
                }
                "subURI" => {
                    payload.insert(
                        "subURI".to_string(),
                        Value::String(self.get_final_sub_uri(host).await?),
                    );
                }
                _ => {}
            }
        }
        Ok(Value::Object(payload))
    }

    async fn record_change_tx(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
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
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    async fn save_clients_tx(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        action: &str,
        payload: &Value,
        host: &str,
    ) -> AppResult<()> {
        match action {
            "new" | "edit" => {
                let value = payload.as_object().ok_or_else(|| {
                    AppError::Validation("client payload must be an object".to_string())
                })?;
                self.upsert_client_tx(tx, value, host).await?;
            }
            "addbulk" | "editbulk" => {
                let values = payload.as_array().ok_or_else(|| {
                    AppError::Validation("bulk client payload must be an array".to_string())
                })?;
                for value in values {
                    let value = value.as_object().ok_or_else(|| {
                        AppError::Validation("bulk client entry must be an object".to_string())
                    })?;
                    self.upsert_client_tx(tx, value, host).await?;
                }
            }
            "del" => {
                let id = value_as_i64(payload).ok_or_else(|| {
                    AppError::Validation("client delete payload must be a numeric id".to_string())
                })?;
                sqlx::query("DELETE FROM clients WHERE id = ?").bind(id).execute(&mut **tx).await?;
            }
            "delbulk" => {
                for id in parse_id_list(payload)? {
                    sqlx::query("DELETE FROM clients WHERE id = ?")
                        .bind(id)
                        .execute(&mut **tx)
                        .await?;
                }
            }
            _ => {
                return Err(AppError::Unsupported(format!("unknown clients action: {action}")));
            }
        }
        Ok(())
    }

    async fn save_tls_tx(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        action: &str,
        payload: &Value,
        host: &str,
    ) -> AppResult<()> {
        match action {
            "new" | "edit" => {
                let object = payload.as_object().ok_or_else(|| {
                    AppError::Validation("tls payload must be an object".to_string())
                })?;
                let id = object.get("id").and_then(Value::as_i64).unwrap_or_default();
                let name = required_string(object, "name")?;
                let server =
                    pretty_json(object.get("server").unwrap_or(&Value::Object(Map::new())))?;
                let client =
                    pretty_json(object.get("client").unwrap_or(&Value::Object(Map::new())))?;

                let final_id = if id > 0 {
                    sqlx::query(
                        r#"
                        INSERT INTO tls (id, name, server, client) VALUES (?, ?, ?, ?)
                        ON CONFLICT(id) DO UPDATE SET name = excluded.name, server = excluded.server, client = excluded.client
                        "#,
                    )
                    .bind(id)
                    .bind(&name)
                    .bind(&server)
                    .bind(&client)
                    .execute(&mut **tx)
                    .await?;
                    id
                } else {
                    sqlx::query("INSERT INTO tls (name, server, client) VALUES (?, ?, ?)")
                        .bind(&name)
                        .bind(&server)
                        .bind(&client)
                        .execute(&mut **tx)
                        .await?
                        .last_insert_rowid()
                };

                let inbound_ids = sqlx::query_scalar::<_, i64>(
                    "SELECT id FROM inbounds WHERE tls_id = ? ORDER BY id ASC",
                )
                .bind(final_id)
                .fetch_all(&mut **tx)
                .await?;
                if !inbound_ids.is_empty() {
                    self.refresh_inbound_out_jsons_tx(tx, &inbound_ids, host).await?;
                    self.refresh_clients_for_inbounds_tx(tx, &inbound_ids, host).await?;
                }
            }
            "del" => {
                let id = value_as_i64(payload).ok_or_else(|| {
                    AppError::Validation("tls delete payload must be a numeric id".to_string())
                })?;
                let inbound_count: i64 =
                    sqlx::query_scalar("SELECT COUNT(*) FROM inbounds WHERE tls_id = ?")
                        .bind(id)
                        .fetch_one(&mut **tx)
                        .await?;
                let service_count: i64 =
                    sqlx::query_scalar("SELECT COUNT(*) FROM services WHERE tls_id = ?")
                        .bind(id)
                        .fetch_one(&mut **tx)
                        .await?;
                if inbound_count > 0 || service_count > 0 {
                    return Err(AppError::Conflict("tls in use".to_string()));
                }
                sqlx::query("DELETE FROM tls WHERE id = ?").bind(id).execute(&mut **tx).await?;
            }
            _ => return Err(AppError::Unsupported(format!("unknown tls action: {action}"))),
        }
        Ok(())
    }

    async fn save_inbounds_tx(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        action: &str,
        payload: &Value,
        init_users: Option<&str>,
        host: &str,
    ) -> AppResult<()> {
        match action {
            "new" | "edit" => {
                let object = payload.as_object().ok_or_else(|| {
                    AppError::Validation("inbound payload must be an object".to_string())
                })?;
                let (id, tag) = self.upsert_inbound_tx(tx, object, host).await?;
                if action == "new" {
                    let client_ids = parse_csv_ids(init_users);
                    if !client_ids.is_empty() {
                        self.add_inbound_to_clients_tx(tx, &client_ids, id, host).await?;
                    }
                } else {
                    self.refresh_clients_for_inbounds_tx(tx, &[id], host).await?;
                }
                if tag.is_empty() {
                    return Err(AppError::Validation("inbound tag can not be empty".to_string()));
                }
            }
            "del" => {
                let tag = payload.as_str().ok_or_else(|| {
                    AppError::Validation("inbound delete payload must be a tag string".to_string())
                })?;
                let Some(id) =
                    sqlx::query_scalar::<_, i64>("SELECT id FROM inbounds WHERE tag = ? LIMIT 1")
                        .bind(tag)
                        .fetch_optional(&mut **tx)
                        .await?
                else {
                    return Ok(());
                };
                self.remove_inbound_from_clients_tx(tx, id, host).await?;
                sqlx::query("DELETE FROM inbounds WHERE id = ?")
                    .bind(id)
                    .execute(&mut **tx)
                    .await?;
            }
            _ => {
                return Err(AppError::Unsupported(format!("unknown inbounds action: {action}")));
            }
        }
        Ok(())
    }

    async fn save_outbounds_tx(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        action: &str,
        payload: &Value,
    ) -> AppResult<()> {
        match action {
            "new" | "edit" => {
                let object = payload.as_object().ok_or_else(|| {
                    AppError::Validation("outbound payload must be an object".to_string())
                })?;
                let id = object.get("id").and_then(Value::as_i64).unwrap_or_default();
                let kind = required_string(object, "type")?;
                let tag = required_string(object, "tag")?;
                let options = strip_entity_fields(object, &["id", "type", "tag"])?;
                upsert_entity_tx(
                    tx,
                    "outbounds",
                    id,
                    &kind,
                    &tag,
                    &pretty_json(&Value::Object(options))?,
                )
                .await?;
            }
            "del" => {
                let tag = payload.as_str().ok_or_else(|| {
                    AppError::Validation("outbound delete payload must be a tag string".to_string())
                })?;
                sqlx::query("DELETE FROM outbounds WHERE tag = ?")
                    .bind(tag)
                    .execute(&mut **tx)
                    .await?;
            }
            _ => {
                return Err(AppError::Unsupported(format!("unknown outbounds action: {action}")));
            }
        }
        Ok(())
    }

    async fn save_services_tx(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        action: &str,
        payload: &Value,
    ) -> AppResult<()> {
        match action {
            "new" | "edit" => {
                let object = payload.as_object().ok_or_else(|| {
                    AppError::Validation("service payload must be an object".to_string())
                })?;
                let id = object.get("id").and_then(Value::as_i64).unwrap_or_default();
                let kind = required_string(object, "type")?;
                let tag = required_string(object, "tag")?;
                let tls_id = object.get("tls_id").and_then(Value::as_i64).unwrap_or_default();
                let options = strip_entity_fields(object, &["id", "type", "tag", "tls_id", "tls"])?;
                if id > 0 {
                    sqlx::query(
                        r#"
                        INSERT INTO services (id, kind, tag, tls_id, options) VALUES (?, ?, ?, ?, ?)
                        ON CONFLICT(id) DO UPDATE SET
                            kind = excluded.kind,
                            tag = excluded.tag,
                            tls_id = excluded.tls_id,
                            options = excluded.options
                        "#,
                    )
                    .bind(id)
                    .bind(&kind)
                    .bind(&tag)
                    .bind(nullable_tls_id(tls_id))
                    .bind(pretty_json(&Value::Object(options))?)
                    .execute(&mut **tx)
                    .await?;
                } else {
                    sqlx::query(
                        "INSERT INTO services (kind, tag, tls_id, options) VALUES (?, ?, ?, ?)",
                    )
                    .bind(&kind)
                    .bind(&tag)
                    .bind(nullable_tls_id(tls_id))
                    .bind(pretty_json(&Value::Object(options))?)
                    .execute(&mut **tx)
                    .await?;
                }
            }
            "del" => {
                let tag = payload.as_str().ok_or_else(|| {
                    AppError::Validation("service delete payload must be a tag string".to_string())
                })?;
                sqlx::query("DELETE FROM services WHERE tag = ?")
                    .bind(tag)
                    .execute(&mut **tx)
                    .await?;
            }
            _ => {
                return Err(AppError::Unsupported(format!("unknown services action: {action}")));
            }
        }
        Ok(())
    }

    async fn save_endpoints_tx(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        action: &str,
        payload: &Value,
    ) -> AppResult<()> {
        match action {
            "new" | "edit" => {
                let object = payload.as_object().ok_or_else(|| {
                    AppError::Validation("endpoint payload must be an object".to_string())
                })?;
                let id = object.get("id").and_then(Value::as_i64).unwrap_or_default();
                let kind = required_string(object, "type")?;
                let tag = required_string(object, "tag")?;
                let ext = object.get("ext").cloned().unwrap_or_else(|| Value::Object(Map::new()));
                let options = strip_entity_fields(object, &["id", "type", "tag", "ext"])?;
                if id > 0 {
                    sqlx::query(
                        r#"
                        INSERT INTO endpoints (id, kind, tag, options, ext) VALUES (?, ?, ?, ?, ?)
                        ON CONFLICT(id) DO UPDATE SET
                            kind = excluded.kind,
                            tag = excluded.tag,
                            options = excluded.options,
                            ext = excluded.ext
                        "#,
                    )
                    .bind(id)
                    .bind(&kind)
                    .bind(&tag)
                    .bind(pretty_json(&Value::Object(options))?)
                    .bind(pretty_json(&ext)?)
                    .execute(&mut **tx)
                    .await?;
                } else {
                    sqlx::query(
                        "INSERT INTO endpoints (kind, tag, options, ext) VALUES (?, ?, ?, ?)",
                    )
                    .bind(&kind)
                    .bind(&tag)
                    .bind(pretty_json(&Value::Object(options))?)
                    .bind(pretty_json(&ext)?)
                    .execute(&mut **tx)
                    .await?;
                }
            }
            "del" => {
                let tag = payload.as_str().ok_or_else(|| {
                    AppError::Validation("endpoint delete payload must be a tag string".to_string())
                })?;
                sqlx::query("DELETE FROM endpoints WHERE tag = ?")
                    .bind(tag)
                    .execute(&mut **tx)
                    .await?;
            }
            _ => {
                return Err(AppError::Unsupported(format!("unknown endpoints action: {action}")));
            }
        }
        Ok(())
    }

    async fn upsert_client_tx(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        object: &Map<String, Value>,
        host: &str,
    ) -> AppResult<i64> {
        let id = object.get("id").and_then(Value::as_i64).unwrap_or_default();
        let existing = if id > 0 {
            sqlx::query_as::<_, ClientRow>(
                r#"
                SELECT
                    id, enable, name, config, inbounds, links, volume, expiry, down, up, desc,
                    group_name, delay_start, auto_reset, reset_days, next_reset, total_up, total_down
                FROM clients
                WHERE id = ?
                LIMIT 1
                "#,
            )
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?
        } else {
            None
        };

        let config_value = object
            .get("config")
            .cloned()
            .or_else(|| existing.as_ref().and_then(|row| serde_json::from_str(&row.config).ok()))
            .unwrap_or_else(|| Value::Object(Map::new()));
        let inbounds_value = object
            .get("inbounds")
            .cloned()
            .or_else(|| existing.as_ref().and_then(|row| serde_json::from_str(&row.inbounds).ok()))
            .unwrap_or_else(|| Value::Array(Vec::new()));
        let inbound_ids = value_to_id_vec(&inbounds_value)?;
        let inbound_inputs = self.load_inbound_inputs_tx(tx, &inbound_ids).await?;
        let links_source = object
            .get("links")
            .cloned()
            .or_else(|| existing.as_ref().and_then(|row| serde_json::from_str(&row.links).ok()))
            .unwrap_or_else(|| Value::Array(Vec::new()));
        let local_links = build_local_links(&config_value, &inbound_inputs, host)?;
        let final_links = merge_client_links(&local_links, &links_source)?;

        let enable = object
            .get("enable")
            .and_then(Value::as_bool)
            .or(existing.as_ref().map(|row| row.enable))
            .unwrap_or(true);
        let name = object
            .get("name")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| existing.as_ref().map(|row| row.name.clone()))
            .ok_or_else(|| AppError::Validation("client name can not be empty".to_string()))?;

        let params = ClientUpsert {
            id,
            enable,
            name,
            config: pretty_json(&config_value)?,
            inbounds: pretty_json(&inbounds_value)?,
            links: pretty_json(&final_links)?,
            volume: object_number_or_existing(
                object.get("volume"),
                existing.as_ref().map(|row| row.volume),
            ),
            expiry: object_number_or_existing(
                object.get("expiry"),
                existing.as_ref().map(|row| row.expiry),
            ),
            down: object_number_or_existing(
                object.get("down"),
                existing.as_ref().map(|row| row.down),
            ),
            up: object_number_or_existing(object.get("up"), existing.as_ref().map(|row| row.up)),
            desc: object
                .get("desc")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .or_else(|| existing.as_ref().map(|row| row.desc.clone()))
                .unwrap_or_default(),
            group_name: object
                .get("group")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .or_else(|| existing.as_ref().map(|row| row.group_name.clone()))
                .unwrap_or_default(),
            delay_start: object
                .get("delayStart")
                .and_then(Value::as_bool)
                .or(existing.as_ref().map(|row| row.delay_start))
                .unwrap_or(false),
            auto_reset: object
                .get("autoReset")
                .and_then(Value::as_bool)
                .or(existing.as_ref().map(|row| row.auto_reset))
                .unwrap_or(false),
            reset_days: object_number_or_existing(
                object.get("resetDays"),
                existing.as_ref().map(|row| row.reset_days),
            ),
            next_reset: object_number_or_existing(
                object.get("nextReset"),
                existing.as_ref().map(|row| row.next_reset),
            ),
            total_up: object_number_or_existing(
                object.get("totalUp"),
                existing.as_ref().map(|row| row.total_up),
            ),
            total_down: object_number_or_existing(
                object.get("totalDown"),
                existing.as_ref().map(|row| row.total_down),
            ),
        };

        let saved_id = upsert_client_row_tx(tx, &params).await?;
        Ok(saved_id)
    }

    async fn upsert_inbound_tx(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        object: &Map<String, Value>,
        host: &str,
    ) -> AppResult<(i64, String)> {
        let id = object.get("id").and_then(Value::as_i64).unwrap_or_default();
        let kind = required_string(object, "type")?;
        let tag = required_string(object, "tag")?;
        let proxy_home = object.get("proxy_home").and_then(Value::as_bool).unwrap_or(false);
        let tls_id = object.get("tls_id").and_then(Value::as_i64).unwrap_or_default();
        let addrs = object.get("addrs").cloned().unwrap_or_else(|| Value::Array(Vec::new()));
        let mut options = strip_entity_fields(
            object,
            &[
                "id",
                "type",
                "tag",
                "proxy_home",
                "allow_lan_access",
                "tls_id",
                "tls",
                "addrs",
                "out_json",
                "users",
            ],
        )?;
        if proxy_home {
            options.entry("allow_lan_access".to_string()).or_insert(Value::Bool(true));
        }
        let tls = self.load_tls_bundle_tx(tx, tls_id).await?;
        let inbound = InboundLinkInput {
            id,
            kind: kind.clone(),
            tag: tag.clone(),
            proxy_home,
            tls_id,
            tls,
            addrs: addrs.clone(),
            out_json: object.get("out_json").cloned().unwrap_or_else(|| Value::Object(Map::new())),
            options: Value::Object(options.clone()),
        };
        let out_json = build_out_json(&inbound, host)?;
        let options_json = pretty_json(&Value::Object(options))?;
        let addrs_json = pretty_json(&addrs)?;
        let out_json_text = pretty_json(&out_json)?;

        let saved_id = if id > 0 {
            sqlx::query(
                r#"
                INSERT INTO inbounds (id, kind, tag, allow_lan_access, tls_id, addrs, out_json, options)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                ON CONFLICT(id) DO UPDATE SET
                    kind = excluded.kind,
                    tag = excluded.tag,
                    allow_lan_access = excluded.allow_lan_access,
                    tls_id = excluded.tls_id,
                    addrs = excluded.addrs,
                    out_json = excluded.out_json,
                    options = excluded.options
                "#,
            )
            .bind(id)
            .bind(&kind)
            .bind(&tag)
            .bind(proxy_home)
            .bind(nullable_tls_id(tls_id))
            .bind(&addrs_json)
            .bind(&out_json_text)
            .bind(&options_json)
            .execute(&mut **tx)
            .await?;
            id
        } else {
            sqlx::query(
                "INSERT INTO inbounds (kind, tag, allow_lan_access, tls_id, addrs, out_json, options) VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&kind)
            .bind(&tag)
            .bind(proxy_home)
            .bind(nullable_tls_id(tls_id))
            .bind(&addrs_json)
            .bind(&out_json_text)
            .bind(&options_json)
            .execute(&mut **tx)
            .await?
            .last_insert_rowid()
        };

        Ok((saved_id, tag))
    }

    async fn refresh_inbound_out_jsons_tx(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        inbound_ids: &[i64],
        host: &str,
    ) -> AppResult<()> {
        for id in inbound_ids {
            let inputs = self.load_inbound_inputs_tx(tx, &[*id]).await?;
            if let Some(inbound) = inputs.first() {
                let out_json = build_out_json(inbound, host)?;
                sqlx::query("UPDATE inbounds SET out_json = ? WHERE id = ?")
                    .bind(pretty_json(&out_json)?)
                    .bind(id)
                    .execute(&mut **tx)
                    .await?;
            }
        }
        Ok(())
    }

    async fn refresh_clients_for_inbounds_tx(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        inbound_ids: &[i64],
        host: &str,
    ) -> AppResult<()> {
        if inbound_ids.is_empty() {
            return Ok(());
        }
        let target_ids = BTreeSet::from_iter(inbound_ids.iter().copied());
        let clients = load_all_clients_tx(tx).await?;
        let affected = clients
            .into_iter()
            .filter(|client| {
                parse_i64_array(&client.inbounds)
                    .map(|ids| ids.into_iter().any(|id| target_ids.contains(&id)))
                    .unwrap_or(false)
            })
            .map(|client| client.id)
            .collect::<Vec<_>>();
        self.refresh_clients_for_ids_tx(tx, &affected, host).await
    }

    async fn refresh_clients_for_ids_tx(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        client_ids: &[i64],
        host: &str,
    ) -> AppResult<()> {
        for client_id in client_ids {
            let Some(client) = load_client_tx(tx, *client_id).await? else {
                continue;
            };
            let inbound_ids = parse_i64_array(&client.inbounds)?;
            let inbound_inputs = self.load_inbound_inputs_tx(tx, &inbound_ids).await?;
            let config_value = parse_json_text(&client.config, Value::Object(Map::new()))?;
            let links_value = parse_json_text(&client.links, Value::Array(Vec::new()))?;
            let local_links = build_local_links(&config_value, &inbound_inputs, host)?;
            let final_links = merge_client_links(&local_links, &links_value)?;
            sqlx::query("UPDATE clients SET links = ? WHERE id = ?")
                .bind(pretty_json(&final_links)?)
                .bind(client_id)
                .execute(&mut **tx)
                .await?;
        }
        Ok(())
    }

    async fn add_inbound_to_clients_tx(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        client_ids: &[i64],
        inbound_id: i64,
        host: &str,
    ) -> AppResult<()> {
        for client_id in client_ids {
            let Some(client) = load_client_tx(tx, *client_id).await? else {
                continue;
            };
            let mut inbound_ids = parse_i64_array(&client.inbounds)?;
            if !inbound_ids.contains(&inbound_id) {
                inbound_ids.push(inbound_id);
            }
            sqlx::query("UPDATE clients SET inbounds = ? WHERE id = ?")
                .bind(pretty_json(&json!(inbound_ids))?)
                .bind(client_id)
                .execute(&mut **tx)
                .await?;
        }
        self.refresh_clients_for_ids_tx(tx, client_ids, host).await
    }

    async fn remove_inbound_from_clients_tx(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        inbound_id: i64,
        host: &str,
    ) -> AppResult<()> {
        let clients = load_all_clients_tx(tx).await?;
        let mut affected = Vec::new();
        for client in clients {
            let mut inbound_ids = parse_i64_array(&client.inbounds)?;
            let original_len = inbound_ids.len();
            inbound_ids.retain(|id| *id != inbound_id);
            if inbound_ids.len() != original_len {
                sqlx::query("UPDATE clients SET inbounds = ? WHERE id = ?")
                    .bind(pretty_json(&json!(inbound_ids))?)
                    .bind(client.id)
                    .execute(&mut **tx)
                    .await?;
                affected.push(client.id);
            }
        }
        self.refresh_clients_for_ids_tx(tx, &affected, host).await
    }

    async fn load_inbound_inputs_tx(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        ids: &[i64],
    ) -> AppResult<Vec<InboundLinkInput>> {
        let mut result = Vec::with_capacity(ids.len());
        for id in ids {
            let Some(row) = sqlx::query_as::<_, InboundRow>(
                "SELECT id, kind, tag, allow_lan_access, COALESCE(tls_id, 0) AS tls_id, addrs, out_json, options FROM inbounds WHERE id = ? LIMIT 1",
            )
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?
            else {
                continue;
            };
            let tls = self.load_tls_bundle_tx(tx, row.tls_id).await?;
            result.push(InboundLinkInput {
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
        Ok(result)
    }

    async fn load_tls_bundle_tx(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        tls_id: i64,
    ) -> AppResult<Option<TlsBundle>> {
        if tls_id <= 0 {
            return Ok(None);
        }
        let Some(row) = sqlx::query_as::<_, TlsRow>(
            "SELECT id, name, server, client FROM tls WHERE id = ? LIMIT 1",
        )
        .bind(tls_id)
        .fetch_optional(&mut **tx)
        .await?
        else {
            return Ok(None);
        };
        Ok(Some(TlsBundle {
            server: parse_json_text(&row.server, Value::Object(Map::new()))?,
            client: parse_json_text(&row.client, Value::Object(Map::new()))?,
        }))
    }
}

#[derive(Debug)]
struct ClientUpsert {
    id: i64,
    enable: bool,
    name: String,
    config: String,
    inbounds: String,
    links: String,
    volume: i64,
    expiry: i64,
    down: i64,
    up: i64,
    desc: String,
    group_name: String,
    delay_start: bool,
    auto_reset: bool,
    reset_days: i64,
    next_reset: i64,
    total_up: i64,
    total_down: i64,
}

async fn upsert_client_row_tx(
    tx: &mut Transaction<'_, Sqlite>,
    client: &ClientUpsert,
) -> AppResult<i64> {
    if client.id > 0 {
        sqlx::query(
            r#"
            INSERT INTO clients (
                id, enable, name, config, inbounds, links, volume, expiry, down, up, desc,
                group_name, delay_start, auto_reset, reset_days, next_reset, total_up, total_down
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
                enable = excluded.enable,
                name = excluded.name,
                config = excluded.config,
                inbounds = excluded.inbounds,
                links = excluded.links,
                volume = excluded.volume,
                expiry = excluded.expiry,
                down = excluded.down,
                up = excluded.up,
                desc = excluded.desc,
                group_name = excluded.group_name,
                delay_start = excluded.delay_start,
                auto_reset = excluded.auto_reset,
                reset_days = excluded.reset_days,
                next_reset = excluded.next_reset,
                total_up = excluded.total_up,
                total_down = excluded.total_down
            "#,
        )
        .bind(client.id)
        .bind(client.enable)
        .bind(&client.name)
        .bind(&client.config)
        .bind(&client.inbounds)
        .bind(&client.links)
        .bind(client.volume)
        .bind(client.expiry)
        .bind(client.down)
        .bind(client.up)
        .bind(&client.desc)
        .bind(&client.group_name)
        .bind(client.delay_start)
        .bind(client.auto_reset)
        .bind(client.reset_days)
        .bind(client.next_reset)
        .bind(client.total_up)
        .bind(client.total_down)
        .execute(&mut **tx)
        .await?;
        Ok(client.id)
    } else {
        Ok(sqlx::query(
            r#"
            INSERT INTO clients (
                enable, name, config, inbounds, links, volume, expiry, down, up, desc,
                group_name, delay_start, auto_reset, reset_days, next_reset, total_up, total_down
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(client.enable)
        .bind(&client.name)
        .bind(&client.config)
        .bind(&client.inbounds)
        .bind(&client.links)
        .bind(client.volume)
        .bind(client.expiry)
        .bind(client.down)
        .bind(client.up)
        .bind(&client.desc)
        .bind(&client.group_name)
        .bind(client.delay_start)
        .bind(client.auto_reset)
        .bind(client.reset_days)
        .bind(client.next_reset)
        .bind(client.total_up)
        .bind(client.total_down)
        .execute(&mut **tx)
        .await?
        .last_insert_rowid())
    }
}

async fn upsert_entity_tx(
    tx: &mut Transaction<'_, Sqlite>,
    table: &str,
    id: i64,
    kind: &str,
    tag: &str,
    options: &str,
) -> AppResult<()> {
    let query = format!(
        r#"
        INSERT INTO {table} (id, kind, tag, options) VALUES (?, ?, ?, ?)
        ON CONFLICT(id) DO UPDATE SET
            kind = excluded.kind,
            tag = excluded.tag,
            options = excluded.options
        "#
    );
    if id > 0 {
        sqlx::query(&query).bind(id).bind(kind).bind(tag).bind(options).execute(&mut **tx).await?;
    } else {
        let query = format!("INSERT INTO {table} (kind, tag, options) VALUES (?, ?, ?)");
        sqlx::query(&query).bind(kind).bind(tag).bind(options).execute(&mut **tx).await?;
    }
    Ok(())
}

async fn load_client_tx(tx: &mut Transaction<'_, Sqlite>, id: i64) -> AppResult<Option<ClientRow>> {
    sqlx::query_as::<_, ClientRow>(
        r#"
        SELECT
            id, enable, name, config, inbounds, links, volume, expiry, down, up, desc,
            group_name, delay_start, auto_reset, reset_days, next_reset, total_up, total_down
        FROM clients
        WHERE id = ?
        LIMIT 1
        "#,
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(Into::into)
}

async fn load_all_clients_tx(tx: &mut Transaction<'_, Sqlite>) -> AppResult<Vec<ClientRow>> {
    sqlx::query_as::<_, ClientRow>(
        r#"
        SELECT
            id, enable, name, config, inbounds, links, volume, expiry, down, up, desc,
            group_name, delay_start, auto_reset, reset_days, next_reset, total_up, total_down
        FROM clients
        ORDER BY id ASC
        "#,
    )
    .fetch_all(&mut **tx)
    .await
    .map_err(Into::into)
}

fn build_local_links(
    client_config: &Value,
    inbounds: &[InboundLinkInput],
    host: &str,
) -> AppResult<Vec<ClientLink>> {
    let mut result = Vec::new();
    for inbound in inbounds {
        for uri in generate_links(client_config, inbound, host)? {
            result.push(ClientLink {
                kind: "local".to_string(),
                remark: Some(inbound.tag.clone()),
                uri,
            });
        }
    }
    Ok(result)
}

fn merge_client_links(local_links: &[ClientLink], links_source: &Value) -> AppResult<Value> {
    let mut links = Vec::new();
    links.extend(local_links.iter().cloned());
    if let Some(source_links) = links_source.as_array() {
        for value in source_links {
            let Ok(link) = serde_json::from_value::<ClientLink>(value.clone()) else {
                continue;
            };
            if link.kind != "local" {
                links.push(link);
            }
        }
    }
    Ok(serde_json::to_value(links)?)
}

fn build_out_json(inbound: &InboundLinkInput, host: &str) -> AppResult<Value> {
    match inbound.kind.as_str() {
        "direct" | "tun" | "redirect" | "tproxy" => return Ok(inbound.out_json.clone()),
        _ => {}
    }

    let options = inbound
        .options
        .as_object()
        .ok_or_else(|| AppError::Validation("inbound options must be an object".to_string()))?;
    let mut out = inbound.out_json.as_object().cloned().unwrap_or_default();
    if let Some(tls) = inbound.tls.as_ref().and_then(|bundle| prepare_tls(bundle).ok()).flatten() {
        out.insert("tls".to_string(), Value::Object(tls));
    } else {
        out.remove("tls");
    }
    out.insert("type".to_string(), Value::String(inbound.kind.clone()));
    out.insert("tag".to_string(), Value::String(inbound.tag.clone()));
    out.insert("server".to_string(), Value::String(host.to_string()));
    if let Some(port) = options.get("listen_port") {
        out.insert("server_port".to_string(), port.clone());
    }

    match inbound.kind.as_str() {
        "http" | "socks" | "mixed" | "anytls" => {}
        "naive" => {
            if let Some(congestion) = options.get("quic_congestion_control").and_then(Value::as_str)
            {
                out.insert("quic".to_string(), Value::Bool(true));
                let value = match congestion {
                    "bbr_standard" => "bbr",
                    "bbr2_variant" => "bbr2",
                    _ => congestion,
                };
                out.insert("quic_congestion_control".to_string(), Value::String(value.to_string()));
            }
        }
        "shadowsocks" => {
            if let Some(method) = options.get("method") {
                out.insert("method".to_string(), method.clone());
            }
        }
        "shadowtls" => {
            let version = options.get("version").and_then(Value::as_i64).unwrap_or_default();
            if version == 3 {
                out.clear();
                out.insert("version".to_string(), json!(3));
                out.insert("tls".to_string(), json!({ "enabled": true }));
            } else {
                out.clear();
            }
        }
        "hysteria" => {
            for key in ["down_mbps", "up_mbps", "obfs", "recv_window_conn", "disable_mtu_discovery"]
            {
                out.remove(key);
            }
            if let Some(value) = options.get("down_mbps") {
                out.insert("up_mbps".to_string(), value.clone());
            }
            if let Some(value) = options.get("up_mbps") {
                out.insert("down_mbps".to_string(), value.clone());
            }
            for key in ["obfs", "recv_window_conn", "disable_mtu_discovery"] {
                if let Some(value) = options.get(key) {
                    out.insert(key.to_string(), value.clone());
                }
            }
        }
        "hysteria2" => {
            for key in ["down_mbps", "up_mbps", "obfs"] {
                out.remove(key);
            }
            if let Some(value) = options.get("down_mbps") {
                out.insert("up_mbps".to_string(), value.clone());
            }
            if let Some(value) = options.get("up_mbps") {
                out.insert("down_mbps".to_string(), value.clone());
            }
            if let Some(value) = options.get("obfs") {
                out.insert("obfs".to_string(), value.clone());
            }
        }
        "tuic" => {
            out.remove("zero_rtt_handshake");
            out.remove("heartbeat");
            out.insert(
                "congestion_control".to_string(),
                options
                    .get("congestion_control")
                    .cloned()
                    .unwrap_or_else(|| Value::String("cubic".to_string())),
            );
            for key in ["zero_rtt_handshake", "heartbeat"] {
                if let Some(value) = options.get(key) {
                    out.insert(key.to_string(), value.clone());
                }
            }
        }
        "vless" | "trojan" | "vmess" => {
            out.remove("transport");
            if let Some(transport) = options.get("transport") {
                out.insert("transport".to_string(), transport.clone());
            }
            if inbound.kind == "vmess" {
                out.insert("alter_id".to_string(), json!(0));
            }
        }
        _ => {
            out.clear();
        }
    }

    Ok(Value::Object(out))
}

fn pretty_json(value: &Value) -> AppResult<String> {
    serde_json::to_string_pretty(value).map_err(Into::into)
}

fn strip_entity_fields(
    object: &Map<String, Value>,
    fields: &[&str],
) -> AppResult<Map<String, Value>> {
    let mut value = object.clone();
    for field in fields {
        value.remove(*field);
    }
    Ok(value)
}

fn required_string(object: &Map<String, Value>, key: &str) -> AppResult<String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| AppError::Validation(format!("missing string field {key}")))
}

fn value_as_i64(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| value.as_u64().map(|value| value as i64))
}

fn parse_id_list(value: &Value) -> AppResult<Vec<i64>> {
    let Some(values) = value.as_array() else {
        return Err(AppError::Validation("expected JSON array of ids".to_string()));
    };
    values
        .iter()
        .map(|value| {
            value_as_i64(value)
                .ok_or_else(|| AppError::Validation("expected numeric id".to_string()))
        })
        .collect()
}

fn value_to_id_vec(value: &Value) -> AppResult<Vec<i64>> {
    let Some(values) = value.as_array() else {
        return Err(AppError::Validation("expected JSON array of ids".to_string()));
    };
    values
        .iter()
        .map(|value| {
            value_as_i64(value)
                .ok_or_else(|| AppError::Validation("expected numeric id".to_string()))
        })
        .collect()
}

fn object_number_or_existing(value: Option<&Value>, existing: Option<i64>) -> i64 {
    value.and_then(value_as_i64).or(existing).unwrap_or_default()
}

fn parse_csv_ids(value: Option<&str>) -> Vec<i64> {
    value
        .unwrap_or_default()
        .split(',')
        .filter_map(|part| part.trim().parse::<i64>().ok())
        .collect()
}

fn nullable_tls_id(value: i64) -> Option<i64> {
    (value > 0).then_some(value)
}
