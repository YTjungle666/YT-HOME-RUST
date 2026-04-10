use std::{collections::BTreeMap, path::PathBuf};

use axum::{
    Json, Router,
    body::Body,
    extract::{Form, Multipart, Query, State},
    http::{HeaderMap, HeaderValue},
    response::Response,
    routing::{get, post},
};
use axum_extra::extract::cookie::CookieJar;
use domain_auth::{
    AuthService, AuthenticatedUser, is_secure_request, maybe_default_password_display,
    removal_cookie,
};
use domain_config::SettingsService;
use domain_core::CoreService;
use domain_stats::StatsService;
use domain_subscription::{SubscriptionService, split_host_port};
use serde::Deserialize;
use serde_json::{Value, json};
use shared::{
    AppError,
    http::ApiMessage,
    settings::{APP_NAME, APP_VERSION, SESSION_COOKIE},
};
use time::{OffsetDateTime, macros::format_description};
use tower_http::services::{ServeDir, ServeFile};
use tracing::error;

#[derive(Clone)]
pub struct AppState {
    pub auth: AuthService,
    pub settings: SettingsService,
    pub core: CoreService,
    pub stats: StatsService,
    pub subscription: SubscriptionService,
}

pub fn router(
    state: AppState,
    sub_path: &str,
    panel_path: &str,
    web_dir: Option<PathBuf>,
) -> Router {
    let router = Router::new()
        .route("/health", get(health))
        .route("/api/login", post(login))
        .route("/api/logout", get(logout))
        .route("/api/session", get(get_session))
        .route("/api/load", get(load_data))
        .route("/api/users", get(get_users))
        .route("/api/settings", get(get_settings))
        .route("/api/status", get(get_status))
        .route("/api/stats", get(get_stats))
        .route("/api/logs", get(get_logs))
        .route("/api/changes", get(get_changes))
        .route("/api/keypairs", get(get_keypairs))
        .route("/api/getdb", get(get_db))
        .route("/api/tokens", get(get_tokens))
        .route("/api/clients", get(get_clients))
        .route("/api/inbounds", get(get_inbounds))
        .route("/api/outbounds", get(get_outbounds))
        .route("/api/endpoints", get(get_endpoints))
        .route("/api/services", get(get_services))
        .route("/api/tls", get(get_tls))
        .route("/api/config", get(get_config))
        .route("/api/changePass", post(change_password))
        .route("/api/addToken", post(add_token))
        .route("/api/deleteToken", post(delete_token))
        .route("/api/restartSb", post(restart_core))
        .route("/api/restartApp", post(restart_app))
        .route("/api/linkConvert", post(link_convert))
        .route("/api/subConvert", post(sub_convert))
        .route("/api/importdb", post(import_db))
        .route("/api/singbox-config", get(get_singbox_config))
        .route("/api/checkOutbound", get(check_outbound))
        .route("/api/save", post(save))
        .nest(sub_path, subscription_router())
        .with_state(state);

    if let Some(web_dir) = web_dir {
        attach_web_router(router, panel_path, web_dir)
    } else {
        router
    }
}

fn subscription_router() -> Router<AppState> {
    Router::new().route("/{subid}", get(get_subscription).head(head_subscription))
}

fn attach_web_router(router: Router, panel_path: &str, web_dir: PathBuf) -> Router {
    let index = web_dir.join("index.html");
    if !index.is_file() {
        return router;
    }

    let normalized_panel_path = normalize_panel_path(panel_path);
    if normalized_panel_path == "/" {
        return router
            .fallback_service(ServeDir::new(web_dir).not_found_service(ServeFile::new(index)));
    }

    router
        .route_service(normalized_panel_path.trim_end_matches('/'), ServeFile::new(index.clone()))
        .route_service(normalized_panel_path.as_str(), ServeFile::new(index.clone()))
        .nest_service(
            normalized_panel_path.trim_end_matches('/'),
            ServeDir::new(web_dir).not_found_service(ServeFile::new(index)),
        )
}

async fn health() -> Json<ApiMessage<Value>> {
    Json(ApiMessage::success(json!({
        "name": APP_NAME,
        "version": APP_VERSION,
    })))
}

async fn login(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Form(form): Form<LoginForm>,
) -> Result<(CookieJar, Json<ApiMessage<Value>>), Json<ApiMessage<Value>>> {
    let session_max_age = state.settings.session_max_age_minutes().await.map_err(api_error)?;
    let secure_cookie =
        is_secure_request(header_string(&headers, "x-forwarded-proto").as_deref(), false);
    let remote_ip = extract_remote_ip(&headers);
    let session = state
        .auth
        .login(&form.user, &form.pass, &remote_ip, session_max_age, secure_cookie)
        .await
        .map_err(api_error)?;

    Ok((jar.add(session.cookie), Json(ApiMessage::success_without_obj())))
}

async fn logout(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
) -> Result<(CookieJar, Json<ApiMessage<Value>>), Json<ApiMessage<Value>>> {
    if let Some(cookie) = jar.get(SESSION_COOKIE) {
        state.auth.logout(cookie.value()).await.map_err(api_error)?;
    }

    let secure_cookie =
        is_secure_request(header_string(&headers, "x-forwarded-proto").as_deref(), false);
    Ok((jar.add(removal_cookie(secure_cookie)), Json(ApiMessage::success_without_obj())))
}

async fn get_session(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Json<ApiMessage<Value>>, Json<ApiMessage<Value>>> {
    let user = current_user(&state, &jar).await?;
    Ok(Json(ApiMessage::success(json!({
        "username": user.username,
    }))))
}

async fn load_data(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Query(query): Query<LoadQuery>,
) -> Result<Json<ApiMessage<Value>>, Json<ApiMessage<Value>>> {
    let _user = current_user(&state, &jar).await?;
    let include_full_payload =
        state.settings.has_changes_since(query.lu).await.map_err(api_error)?;
    let host = extract_host(&headers);
    let mut payload =
        state.settings.load_dashboard_data(&host, include_full_payload).await.map_err(api_error)?;

    if state.core.status().await["running"] == Value::Bool(false) {
        let logs = state.core.logs(1, None).await;
        if let Some(last_log) = logs.first() {
            payload["lastLog"] = Value::String(last_log.clone());
        }
    }

    Ok(Json(ApiMessage::success(payload)))
}

async fn get_users(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Json<ApiMessage<Value>>, Json<ApiMessage<Value>>> {
    let _user = current_user(&state, &jar).await?;
    let users = state.auth.get_public_users().await.map_err(api_error)?;
    Ok(Json(ApiMessage::success(json!(users))))
}

async fn get_settings(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Json<ApiMessage<Value>>, Json<ApiMessage<Value>>> {
    let _user = current_user(&state, &jar).await?;
    let settings = state.settings.public_settings().await.map_err(api_error)?;
    Ok(Json(ApiMessage::success(json!(settings))))
}

async fn get_status(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(query): Query<StatusQuery>,
) -> Result<Json<ApiMessage<Value>>, Json<ApiMessage<Value>>> {
    let _user = current_user(&state, &jar).await?;
    let db_info = state.settings.db_counts().await.map_err(api_error)?;
    let status =
        state.stats.get_status(query.r.as_deref().unwrap_or_default(), db_info, &state.core).await;
    Ok(Json(ApiMessage::success(status)))
}

async fn get_stats(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(query): Query<StatsQuery>,
) -> Result<Json<ApiMessage<Value>>, Json<ApiMessage<Value>>> {
    let _user = current_user(&state, &jar).await?;
    let stats = state
        .stats
        .get_stats(query.resource.as_deref(), query.tag.as_deref(), query.limit.unwrap_or(100))
        .await
        .map_err(api_error)?;
    Ok(Json(ApiMessage::success(json!(stats))))
}

async fn get_changes(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(query): Query<ChangesQuery>,
) -> Result<Json<ApiMessage<Value>>, Json<ApiMessage<Value>>> {
    let _user = current_user(&state, &jar).await?;
    let changes = state
        .settings
        .get_changes(query.a.as_deref(), query.k.as_deref(), query.c.unwrap_or(20))
        .await
        .map_err(api_error)?;
    Ok(Json(ApiMessage::success(json!(changes))))
}

async fn get_logs(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(query): Query<LogsQuery>,
) -> Result<Json<ApiMessage<Value>>, Json<ApiMessage<Value>>> {
    let _user = current_user(&state, &jar).await?;
    let count = query.c.unwrap_or(10).max(1);
    let logs = state.core.logs(count, query.l.as_deref()).await;
    Ok(Json(ApiMessage::success(json!(logs))))
}

async fn get_keypairs(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(query): Query<KeypairQuery>,
) -> Result<Json<ApiMessage<Value>>, Json<ApiMessage<Value>>> {
    let _user = current_user(&state, &jar).await?;
    let keypair = state.core.generate_keypair(&query.k, query.o.as_deref()).map_err(api_error)?;
    Ok(Json(ApiMessage::success(json!(keypair))))
}

async fn get_db(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(query): Query<BackupQuery>,
) -> Result<Response, Json<ApiMessage<Value>>> {
    let _user = current_user(&state, &jar).await?;
    let excludes = query
        .exclude
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let exclude_refs = excludes.iter().map(String::as_str).collect::<Vec<_>>();
    let body = state.settings.export_database(&exclude_refs).await.map_err(api_error)?;
    download_response(
        "application/octet-stream",
        &format!("s-ui_{}.db", file_timestamp().map_err(api_error)?),
        body,
    )
}

async fn get_tokens(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Json<ApiMessage<Value>>, Json<ApiMessage<Value>>> {
    let user = current_user(&state, &jar).await?;
    let tokens = state.auth.get_user_tokens(&user.username).await.map_err(api_error)?;
    Ok(Json(ApiMessage::success(json!(tokens))))
}

async fn get_singbox_config(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Response, Json<ApiMessage<Value>>> {
    let _user = current_user(&state, &jar).await?;
    let config = state.settings.build_runtime_config().await.map_err(api_error)?;
    download_response(
        "application/json",
        &format!("config_{}.json", file_timestamp().map_err(api_error)?),
        config.into_bytes(),
    )
}

async fn get_clients(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(query): Query<IdQuery>,
) -> Result<Json<ApiMessage<Value>>, Json<ApiMessage<Value>>> {
    let _user = current_user(&state, &jar).await?;
    let ids = parse_ids(query.id.as_deref())?;
    let clients = if ids.is_empty() {
        state.settings.list_clients_summary().await.map_err(api_error)?
    } else {
        state.settings.list_clients_by_ids(&ids).await.map_err(api_error)?
    };
    Ok(Json(ApiMessage::success(json!({ "clients": clients }))))
}

async fn get_inbounds(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(query): Query<IdQuery>,
) -> Result<Json<ApiMessage<Value>>, Json<ApiMessage<Value>>> {
    let _user = current_user(&state, &jar).await?;
    let ids = parse_ids(query.id.as_deref())?;
    let inbounds = if ids.is_empty() {
        state.settings.list_inbound_summaries().await.map_err(api_error)?
    } else {
        state.settings.list_inbounds_by_ids(&ids).await.map_err(api_error)?
    };
    Ok(Json(ApiMessage::success(json!({ "inbounds": inbounds }))))
}

async fn get_outbounds(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Json<ApiMessage<Value>>, Json<ApiMessage<Value>>> {
    let _user = current_user(&state, &jar).await?;
    let outbounds = state.settings.list_outbounds().await.map_err(api_error)?;
    Ok(Json(ApiMessage::success(json!({ "outbounds": outbounds }))))
}

async fn get_endpoints(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Json<ApiMessage<Value>>, Json<ApiMessage<Value>>> {
    let _user = current_user(&state, &jar).await?;
    let endpoints = state.settings.list_endpoints().await.map_err(api_error)?;
    Ok(Json(ApiMessage::success(json!({ "endpoints": endpoints }))))
}

async fn get_services(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Json<ApiMessage<Value>>, Json<ApiMessage<Value>>> {
    let _user = current_user(&state, &jar).await?;
    let services = state.settings.list_services().await.map_err(api_error)?;
    Ok(Json(ApiMessage::success(json!({ "services": services }))))
}

async fn get_tls(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Json<ApiMessage<Value>>, Json<ApiMessage<Value>>> {
    let _user = current_user(&state, &jar).await?;
    let tls = state.settings.list_tls().await.map_err(api_error)?;
    Ok(Json(ApiMessage::success(json!({ "tls": tls }))))
}

async fn get_config(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Json<ApiMessage<Value>>, Json<ApiMessage<Value>>> {
    let _user = current_user(&state, &jar).await?;
    let config = state.settings.get_config().await.map_err(api_error)?;
    let config_value =
        serde_json::from_str::<Value>(&config).map_err(|error| api_error(error.into()))?;
    Ok(Json(ApiMessage::success(json!({
        "config": config_value,
    }))))
}

async fn change_password(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<ChangePassForm>,
) -> Result<Json<ApiMessage<Value>>, Json<ApiMessage<Value>>> {
    let _user = current_user(&state, &jar).await?;
    let id = form
        .id
        .parse::<i64>()
        .map_err(|error| api_error(AppError::Validation(format!("invalid user id: {error}"))))?;
    state
        .auth
        .change_password(id, &form.old_pass, &form.new_username, &form.new_pass)
        .await
        .map_err(api_error)?;
    Ok(Json(ApiMessage::action("save")))
}

async fn add_token(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<AddTokenForm>,
) -> Result<Json<ApiMessage<Value>>, Json<ApiMessage<Value>>> {
    let user = current_user(&state, &jar).await?;
    let token =
        state.auth.add_token(&user.username, form.expiry, &form.desc).await.map_err(api_error)?;
    Ok(Json(ApiMessage::success(json!(token))))
}

async fn delete_token(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<DeleteTokenForm>,
) -> Result<Json<ApiMessage<Value>>, Json<ApiMessage<Value>>> {
    let _user = current_user(&state, &jar).await?;
    state.auth.delete_token(form.id).await.map_err(api_error)?;
    Ok(Json(ApiMessage::success_without_obj()))
}

async fn restart_core(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Json<ApiMessage<Value>>, Json<ApiMessage<Value>>> {
    let _user = current_user(&state, &jar).await?;
    let config = state.settings.build_runtime_config().await.map_err(api_error)?;
    state.core.restart_with_config(config).await.map_err(api_error)?;
    Ok(Json(ApiMessage::action("restartSb")))
}

async fn restart_app(
    State(_state): State<AppState>,
    jar: CookieJar,
) -> Result<Json<ApiMessage<Value>>, Json<ApiMessage<Value>>> {
    let _user = current_user(&_state, &jar).await?;
    Ok(Json(ApiMessage::action("restartApp")))
}

async fn link_convert(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<LinkForm>,
) -> Result<Json<ApiMessage<Value>>, Json<ApiMessage<Value>>> {
    let _user = current_user(&state, &jar).await?;
    let outbound = state.subscription.convert_link(&form.link).map_err(api_error)?;
    Ok(Json(ApiMessage::success(outbound)))
}

async fn sub_convert(
    State(state): State<AppState>,
    jar: CookieJar,
    Form(form): Form<LinkForm>,
) -> Result<Json<ApiMessage<Value>>, Json<ApiMessage<Value>>> {
    let _user = current_user(&state, &jar).await?;
    let outbounds =
        state.subscription.convert_subscription_link(&form.link).await.map_err(api_error)?;
    Ok(Json(ApiMessage::success(json!(outbounds))))
}

async fn import_db(
    State(state): State<AppState>,
    jar: CookieJar,
    mut multipart: Multipart,
) -> Result<Json<ApiMessage<Value>>, Json<ApiMessage<Value>>> {
    let _user = current_user(&state, &jar).await?;
    let mut db_bytes = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| api_error(AppError::Validation(error.to_string())))?
    {
        if field.name() != Some("db") {
            continue;
        }
        let bytes = field
            .bytes()
            .await
            .map_err(|error| api_error(AppError::Validation(error.to_string())))?;
        db_bytes = Some(bytes.to_vec());
        break;
    }

    let bytes =
        db_bytes.ok_or_else(|| api_error(AppError::Validation("missing db file".to_string())))?;
    state.settings.import_database(&bytes).await.map_err(api_error)?;
    schedule_core_reload(&state);
    Ok(Json(ApiMessage::success_without_obj()))
}

async fn check_outbound(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(query): Query<CheckOutboundQuery>,
) -> Result<Json<ApiMessage<Value>>, Json<ApiMessage<Value>>> {
    let _user = current_user(&state, &jar).await?;
    let result = state.core.check_outbound(&query.tag, query.link.as_deref()).await;
    Ok(Json(ApiMessage::success(result)))
}

async fn save(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Form(form): Form<SaveForm>,
) -> Result<Json<ApiMessage<Value>>, Json<ApiMessage<Value>>> {
    let user = current_user(&state, &jar).await?;
    let payload: Value =
        serde_json::from_str(&form.data).map_err(|error| api_error(error.into()))?;

    match form.object.as_str() {
        "settings" => {
            let settings: BTreeMap<String, String> =
                serde_json::from_value(payload.clone()).map_err(|error| api_error(error.into()))?;
            state.settings.save_public_settings(&settings).await.map_err(api_error)?;
            state
                .settings
                .record_change(&user.username, "settings", &form.action, &payload)
                .await
                .map_err(api_error)?;
            let settings = state.settings.public_settings().await.map_err(api_error)?;
            Ok(Json(ApiMessage::success(json!({ "settings": settings }))))
        }
        "config" => {
            state.settings.save_config(&payload).await.map_err(api_error)?;
            state
                .settings
                .record_change(&user.username, "config", &form.action, &payload)
                .await
                .map_err(api_error)?;
            schedule_core_reload(&state);
            let host = extract_host(&headers);
            let payload =
                state.settings.load_partial_payload(&["config"], &host).await.map_err(api_error)?;
            Ok(Json(ApiMessage::success(payload)))
        }
        _ => {
            let host = extract_host(&headers);
            let payload = state
                .settings
                .save_managed_object(
                    &form.object,
                    &form.action,
                    &payload,
                    form.init_users.as_deref(),
                    &user.username,
                    &host,
                )
                .await
                .map_err(api_error)?;
            schedule_core_reload(&state);
            Ok(Json(ApiMessage::success(payload)))
        }
    }
}

async fn get_subscription(
    State(state): State<AppState>,
    axum::extract::Path(sub_id): axum::extract::Path<String>,
    Query(query): Query<SubscriptionQuery>,
) -> Result<axum::response::Response, Json<ApiMessage<Value>>> {
    let document = match query.format.as_deref() {
        Some("json") => state
            .subscription
            .get_json_subscription(&sub_id, query.inbound.as_deref())
            .await
            .map_err(api_error)?,
        Some("clash") => state
            .subscription
            .get_clash_subscription(&sub_id, query.inbound.as_deref())
            .await
            .map_err(api_error)?,
        Some(other) => {
            return Err(Json(ApiMessage::failure(format!(
                "unsupported subscription format {other}"
            ))));
        }
        None => state.subscription.get_plain_subscription(&sub_id).await.map_err(api_error)?,
    };
    let mut response = axum::response::Response::new(document.body.into());
    let headers = response.headers_mut();
    headers.insert(
        "Subscription-Userinfo",
        document
            .headers
            .userinfo
            .parse::<axum::http::HeaderValue>()
            .map_err(|error| api_error(AppError::Validation(error.to_string())))?,
    );
    headers.insert(
        "Profile-Update-Interval",
        document
            .headers
            .update_interval
            .parse::<axum::http::HeaderValue>()
            .map_err(|error| api_error(AppError::Validation(error.to_string())))?,
    );
    headers.insert(
        "Profile-Title",
        document
            .headers
            .title
            .parse::<axum::http::HeaderValue>()
            .map_err(|error| api_error(AppError::Validation(error.to_string())))?,
    );
    Ok(response)
}

async fn head_subscription(
    State(state): State<AppState>,
    axum::extract::Path(sub_id): axum::extract::Path<String>,
    Query(query): Query<SubscriptionQuery>,
) -> Result<axum::response::Response, Json<ApiMessage<Value>>> {
    let document = match query.format.as_deref() {
        Some("json") => state
            .subscription
            .get_json_subscription(&sub_id, query.inbound.as_deref())
            .await
            .map_err(api_error)?,
        Some("clash") => state
            .subscription
            .get_clash_subscription(&sub_id, query.inbound.as_deref())
            .await
            .map_err(api_error)?,
        Some(other) => {
            return Err(Json(ApiMessage::failure(format!(
                "unsupported subscription format {other}"
            ))));
        }
        None => state.subscription.get_plain_subscription(&sub_id).await.map_err(api_error)?,
    };
    let mut response = axum::response::Response::new(axum::body::Body::empty());
    let headers = response.headers_mut();
    headers.insert(
        "Subscription-Userinfo",
        document
            .headers
            .userinfo
            .parse::<axum::http::HeaderValue>()
            .map_err(|error| api_error(AppError::Validation(error.to_string())))?,
    );
    headers.insert(
        "Profile-Update-Interval",
        document
            .headers
            .update_interval
            .parse::<axum::http::HeaderValue>()
            .map_err(|error| api_error(AppError::Validation(error.to_string())))?,
    );
    headers.insert(
        "Profile-Title",
        document
            .headers
            .title
            .parse::<axum::http::HeaderValue>()
            .map_err(|error| api_error(AppError::Validation(error.to_string())))?,
    );
    Ok(response)
}

fn api_error(error: AppError) -> Json<ApiMessage<Value>> {
    Json(ApiMessage::failure(error.message()))
}

fn download_response(
    content_type: &'static str,
    filename: &str,
    body: Vec<u8>,
) -> Result<Response, Json<ApiMessage<Value>>> {
    let mut response = Response::new(Body::from(body));
    let headers = response.headers_mut();
    headers.insert(axum::http::header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(
        axum::http::header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename={filename}"))
            .map_err(|error| api_error(AppError::Validation(error.to_string())))?,
    );
    Ok(response)
}

fn file_timestamp() -> Result<String, AppError> {
    OffsetDateTime::now_utc()
        .format(&format_description!("[year][month][day]-[hour][minute][second]"))
        .map_err(|error| AppError::Validation(error.to_string()))
}

async fn current_user(
    state: &AppState,
    jar: &CookieJar,
) -> Result<AuthenticatedUser, Json<ApiMessage<Value>>> {
    let Some(cookie) = jar.get(SESSION_COOKIE) else {
        return Err(Json(ApiMessage::failure("Invalid login")));
    };

    let Some(user) = state.auth.authenticate_session(cookie.value()).await.map_err(api_error)?
    else {
        return Err(Json(ApiMessage::failure("Invalid login")));
    };

    Ok(user)
}

fn extract_host(headers: &HeaderMap) -> String {
    split_host_port(&header_string(headers, "host").unwrap_or_else(|| "localhost".to_string()))
}

fn extract_remote_ip(headers: &HeaderMap) -> String {
    header_string(headers, "x-forwarded-for")
        .and_then(|value| value.split(',').next().map(str::trim).map(str::to_string))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "127.0.0.1".to_string())
}

fn header_string(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get(name).and_then(|value| value.to_str().ok()).map(ToOwned::to_owned)
}

fn parse_ids(value: Option<&str>) -> Result<Vec<i64>, Json<ApiMessage<Value>>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    value
        .split(',')
        .filter(|part| !part.trim().is_empty())
        .map(|part| {
            part.trim()
                .parse::<i64>()
                .map_err(|error| Json(ApiMessage::failure(format!("invalid numeric id: {error}"))))
        })
        .collect()
}

fn normalize_panel_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == "/" {
        return "/".to_string();
    }

    let without_trailing = trimmed.trim_end_matches('/');
    if without_trailing.starts_with('/') {
        format!("{without_trailing}/")
    } else {
        format!("/{without_trailing}/")
    }
}

#[derive(Debug, Deserialize)]
struct LoginForm {
    user: String,
    pass: String,
}

#[derive(Debug, Deserialize)]
struct ChangePassForm {
    id: String,
    #[serde(rename = "oldPass")]
    old_pass: String,
    #[serde(rename = "newUsername")]
    new_username: String,
    #[serde(rename = "newPass")]
    new_pass: String,
}

#[derive(Debug, Deserialize)]
struct AddTokenForm {
    desc: String,
    expiry: i64,
}

#[derive(Debug, Deserialize)]
struct DeleteTokenForm {
    id: i64,
}

#[derive(Debug, Deserialize)]
struct LinkForm {
    link: String,
}

#[derive(Debug, Deserialize)]
struct SaveForm {
    object: String,
    action: String,
    data: String,
    #[serde(rename = "initUsers")]
    init_users: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LoadQuery {
    lu: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct StatusQuery {
    r: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StatsQuery {
    resource: Option<String>,
    tag: Option<String>,
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct ChangesQuery {
    a: Option<String>,
    k: Option<String>,
    c: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct LogsQuery {
    c: Option<usize>,
    l: Option<String>,
}

#[derive(Debug, Deserialize)]
struct KeypairQuery {
    k: String,
    o: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BackupQuery {
    exclude: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IdQuery {
    id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CheckOutboundQuery {
    tag: String,
    link: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SubscriptionQuery {
    format: Option<String>,
    inbound: Option<String>,
}

pub async fn default_admin_password_for_display(auth: &AuthService) -> String {
    match auth.get_first_user().await {
        Ok(Some(user)) => maybe_default_password_display(&user.password, &user.username),
        _ => "<unavailable>".to_string(),
    }
}

fn schedule_core_reload(state: &AppState) {
    let state = state.clone();
    tokio::spawn(async move {
        let config = match state.settings.build_runtime_config().await {
            Ok(config) => config,
            Err(err) => {
                error!("failed to load config before core reload: {}", err.message());
                return;
            }
        };

        if let Err(err) = state.core.reload_with_config(config).await {
            error!("failed to reload sing-box after data change: {}", err.message());
        }
    });
}
