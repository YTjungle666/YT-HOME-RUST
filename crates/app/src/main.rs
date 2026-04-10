use std::{
    collections::BTreeMap,
    env,
    path::{Path, PathBuf},
};

use domain_auth::{AuthService, maybe_default_password_display};
use domain_config::SettingsService;
use domain_core::CoreService;
use domain_stats::StatsService;
use domain_subscription::SubscriptionService;
use http_api::{AppState, router};
use if_addrs::{IfAddr, get_if_addrs};
use infra_db::{Db, connect_sqlite, default_db_path, run_migrations};
use infra_observability::init_tracing;
use shared::{AppError, AppResult, settings::APP_VERSION};
use tokio::net::TcpListener;
use tracing::{error, info};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_tracing("info");

    let runtime = tokio::runtime::Builder::new_multi_thread().enable_io().enable_time().build()?;

    runtime.block_on(async_main())
}

async fn async_main() -> Result<(), Box<dyn std::error::Error>> {
    let executable = env::current_exe()?;
    let executable_dir =
        executable.parent().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let command = env::args().nth(1);

    match command.as_deref() {
        Some("-v") | Some("--version") => show_version(),
        Some("admin") => run_admin_command(env::args().skip(2).collect(), &executable_dir).await?,
        Some("setting") => {
            run_setting_command(env::args().skip(2).collect(), &executable_dir).await?
        }
        Some("uri") => run_uri_command(&executable_dir).await?,
        Some("migrate") => run_migrate_command(&executable_dir).await?,
        Some(other) => {
            return Err(AppError::Unsupported(format!("invalid subcommand {other}")).into());
        }
        None => run_server(executable_dir).await?,
    }

    Ok(())
}

async fn run_server(executable_dir: PathBuf) -> AppResult<()> {
    let db = open_database(&executable_dir).await?;

    let auth = AuthService::new(db.clone());
    let settings = SettingsService::new(db.clone());
    let core = CoreService::new();
    let stats = StatsService::new(db.clone());
    let subscription = SubscriptionService::new(db)?;

    settings.ensure_defaults().await?;
    auth.ensure_default_admin().await?;

    let config = settings.build_runtime_config().await?;
    if let Err(err) = core.start_with_config(config).await {
        error!("failed to start sing-box during boot: {}", err.message());
    }

    let port = env::var("SUI_WEB_PORT")
        .ok()
        .map(|value| value.parse::<u16>())
        .transpose()
        .map_err(|error| AppError::Validation(error.to_string()))?
        .unwrap_or(settings.panel_port().await?);
    let sub_port = env::var("SUI_SUB_PORT")
        .ok()
        .map(|value| value.parse::<u16>())
        .transpose()
        .map_err(|error| AppError::Validation(error.to_string()))?
        .unwrap_or(settings.subscription_port().await?);
    let app_state = AppState { auth, settings, core, stats, subscription };
    let sub_path = app_state.settings.subscription_path().await?;
    let panel_path = app_state.settings.panel_path().await?;
    let web_dir = resolve_web_dir(&executable_dir);
    let app = router(app_state, &sub_path, &panel_path, web_dir);

    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    info!("rust web backend listening on {}", listener.local_addr()?);
    if sub_port == port {
        axum::serve(listener, app)
            .await
            .map_err(|error| AppError::Unsupported(error.to_string()))?;
    } else {
        let sub_listener = TcpListener::bind(("0.0.0.0", sub_port)).await?;
        info!("rust subscription backend listening on {}", sub_listener.local_addr()?);
        let web_server = axum::serve(listener, app.clone());
        let sub_server = axum::serve(sub_listener, app);
        tokio::try_join!(web_server, sub_server)
            .map_err(|error| AppError::Unsupported(error.to_string()))?;
    }

    Ok(())
}

async fn run_admin_command(args: Vec<String>, executable_dir: &Path) -> AppResult<()> {
    let db = open_database(executable_dir).await?;
    let auth = AuthService::new(db);
    auth.ensure_default_admin().await?;

    let mut show = false;
    let mut reset = false;
    let mut username = None;
    let mut password = None;
    let mut iter = args.into_iter();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-show" => show = true,
            "-reset" => reset = true,
            "-username" => username = Some(next_arg(&mut iter, "-username")?),
            "-password" => password = Some(next_arg(&mut iter, "-password")?),
            other => {
                return Err(AppError::Validation(format!("unsupported admin flag {other}")));
            }
        }
    }

    if reset {
        auth.reset_first_user_to_default().await?;
        println!("reset admin credentials success");
    } else if username.is_some() || password.is_some() {
        auth.set_first_user_credentials(username.as_deref(), password.as_deref()).await?;
        println!("reset admin credentials success");
    }

    if show || reset || username.is_some() || password.is_some() {
        show_admin(&auth).await?;
    }

    Ok(())
}

async fn run_setting_command(args: Vec<String>, executable_dir: &Path) -> AppResult<()> {
    let db = open_database(executable_dir).await?;
    let settings = SettingsService::new(db);
    settings.ensure_defaults().await?;

    let mut show = false;
    let mut reset = false;
    let mut updates = BTreeMap::new();
    let mut iter = args.into_iter();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-show" => show = true,
            "-reset" => reset = true,
            "-port" => {
                let value = next_arg(&mut iter, "-port")?;
                updates.insert("webPort".to_string(), value);
            }
            "-path" => {
                let value = next_arg(&mut iter, "-path")?;
                updates.insert("webPath".to_string(), value);
            }
            "-subPort" => {
                let value = next_arg(&mut iter, "-subPort")?;
                updates.insert("subPort".to_string(), value);
            }
            "-subPath" => {
                let value = next_arg(&mut iter, "-subPath")?;
                updates.insert("subPath".to_string(), value);
            }
            other => {
                return Err(AppError::Validation(format!("unsupported setting flag {other}")));
            }
        }
    }

    if reset {
        settings.reset_defaults().await?;
        println!("reset setting success");
    } else if !updates.is_empty() {
        settings.save_public_settings(&updates).await?;
    }

    if show || reset || !updates.is_empty() {
        show_settings(&settings).await?;
    }

    Ok(())
}

async fn run_uri_command(executable_dir: &Path) -> AppResult<()> {
    let db = open_database(executable_dir).await?;
    let settings = SettingsService::new(db);
    settings.ensure_defaults().await?;

    for uri in panel_uris(&settings).await? {
        println!("{uri}");
    }

    Ok(())
}

async fn run_migrate_command(executable_dir: &Path) -> AppResult<()> {
    let db = open_database(executable_dir).await?;
    let auth = AuthService::new(db.clone());
    let settings = SettingsService::new(db);
    settings.ensure_defaults().await?;
    auth.ensure_default_admin().await?;
    println!("database migration completed");
    Ok(())
}

async fn open_database(executable_dir: &Path) -> AppResult<Db> {
    let db_path = env::var("SUI_DB_FOLDER")
        .map(|folder| format!("{folder}/s-ui.db"))
        .unwrap_or_else(|_| default_db_path(executable_dir));

    let db = connect_sqlite(&db_path).await?;
    run_migrations(&db).await?;
    Ok(db)
}

async fn show_admin(auth: &AuthService) -> AppResult<()> {
    let Some(user) = auth.get_first_user().await? else {
        return Err(AppError::NotFound("user not found".to_string()));
    };

    println!("First admin credentials:");
    println!("\tUsername:\t {}", user.username);
    println!("\tPassword:\t {}", maybe_default_password_display(&user.password, &user.username));
    Ok(())
}

async fn show_settings(settings: &SettingsService) -> AppResult<()> {
    let values = settings.public_settings().await?;

    println!("Current panel settings:");
    println!("\tPanel port:\t {}", values.get("webPort").map(String::as_str).unwrap_or("80"));
    println!("\tPanel path:\t {}", values.get("webPath").map(String::as_str).unwrap_or("/"));
    if let Some(listen) = values.get("webListen").filter(|value| !value.is_empty()) {
        println!("\tPanel IP:\t {listen}");
    }
    if let Some(domain) = values.get("webDomain").filter(|value| !value.is_empty()) {
        println!("\tPanel Domain:\t {domain}");
    }
    if let Some(uri) = values.get("webURI").filter(|value| !value.is_empty()) {
        println!("\tPanel URI:\t {uri}");
    }

    println!();
    println!("Current subscription settings:");
    println!("\tSub port:\t {}", values.get("subPort").map(String::as_str).unwrap_or("2096"));
    println!("\tSub path:\t {}", values.get("subPath").map(String::as_str).unwrap_or("/sub/"));
    if let Some(listen) = values.get("subListen").filter(|value| !value.is_empty()) {
        println!("\tSub IP:\t {listen}");
    }
    if let Some(domain) = values.get("subDomain").filter(|value| !value.is_empty()) {
        println!("\tSub Domain:\t {domain}");
    }
    if let Some(uri) = values.get("subURI").filter(|value| !value.is_empty()) {
        println!("\tSub URI:\t {uri}");
    }

    Ok(())
}

async fn panel_uris(settings: &SettingsService) -> AppResult<Vec<String>> {
    let explicit_uri = settings.get_string("webURI").await?;
    if !explicit_uri.is_empty() {
        return Ok(vec![explicit_uri]);
    }

    let domain = settings.get_string("webDomain").await?;
    let listen = settings.get_string("webListen").await?;
    let path = settings.panel_path().await?;
    let port = settings.panel_port().await?;
    let use_tls = !settings.get_string("webKeyFile").await?.is_empty()
        && !settings.get_string("webCertFile").await?.is_empty();
    let protocol = if use_tls { "https" } else { "http" };
    let port_suffix = if (port == 80 && !use_tls) || (port == 443 && use_tls) {
        String::new()
    } else {
        format!(":{port}")
    };

    if !domain.is_empty() {
        return Ok(vec![format!("{protocol}://{domain}{port_suffix}{path}")]);
    }
    if !listen.is_empty() {
        return Ok(vec![format!("{protocol}://{listen}{port_suffix}{path}")]);
    }

    let mut uris = Vec::new();
    for address in local_addresses() {
        let host = if address.contains(':') { format!("[{address}]") } else { address };
        uris.push(format!("{protocol}://{host}{port_suffix}{path}"));
    }
    if uris.is_empty() {
        uris.push(format!("{protocol}://127.0.0.1{port_suffix}{path}"));
    }
    Ok(uris)
}

fn local_addresses() -> Vec<String> {
    let Ok(addresses) = get_if_addrs() else {
        return Vec::new();
    };

    let mut result = Vec::new();
    for interface in addresses {
        if interface.is_loopback() {
            continue;
        }
        match interface.addr {
            IfAddr::V4(addr) => result.push(addr.ip.to_string()),
            IfAddr::V6(addr) => {
                let ip = addr.ip.to_string();
                if !ip.starts_with("fe80:") {
                    result.push(ip);
                }
            }
        }
    }
    result.sort();
    result.dedup();
    result
}

fn next_arg(iter: &mut impl Iterator<Item = String>, flag: &str) -> AppResult<String> {
    iter.next()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AppError::Validation(format!("missing value for {flag}")))
}

fn show_version() {
    println!("YT HOME Panel\t {APP_VERSION}");
}

fn resolve_web_dir(executable_dir: &Path) -> Option<PathBuf> {
    if let Ok(value) = env::var("SUI_WEB_DIR") {
        let path = PathBuf::from(value);
        if path.join("index.html").is_file() {
            return Some(path);
        }
    }

    let current_dir = env::current_dir().ok();
    let candidates = [
        executable_dir.join("web"),
        executable_dir.join("frontend").join("dist"),
        executable_dir.join("../frontend/dist"),
        executable_dir.join("../../frontend/dist"),
        current_dir.as_ref().map(|dir| dir.join("frontend").join("dist")).unwrap_or_default(),
        current_dir.as_ref().map(|dir| dir.join("web")).unwrap_or_default(),
    ];

    candidates.into_iter().find(|path| path.join("index.html").is_file())
}
