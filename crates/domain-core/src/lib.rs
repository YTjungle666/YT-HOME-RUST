use std::{
    collections::VecDeque,
    env, fs,
    net::{Ipv4Addr, SocketAddrV4, TcpListener as StdTcpListener},
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
};

use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use rcgen::{CertifiedKey, generate_simple_self_signed};
use reqwest::{Method, Proxy, redirect::Policy};
use serde_json::{Map, Value, json};
use shared::{AppError, AppResult};
use time::OffsetDateTime;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, BufReader},
    net::TcpStream,
    process::{Child, Command},
    sync::Mutex,
    time::{Duration, Instant, sleep, timeout},
};
use uuid::Uuid;
use x25519_dalek::{PublicKey, StaticSecret};

const LOG_LIMIT: usize = 10_240;
const PROCESS_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
const PROCESS_BOOT_TIMEOUT: Duration = Duration::from_secs(20);
const PROCESS_MONITOR_INTERVAL: Duration = Duration::from_secs(1);
const OUTBOUND_CHECK_TIMEOUT: Duration = Duration::from_secs(15);
const OUTBOUND_DEFAULT_URL: &str = "https://www.gstatic.com/generate_204";
const MAIN_CONFIG_FILE: &str = "config.json";
const PROBE_INBOUND_TAG: &str = "rust-check-socks";

struct ProcessHandle {
    child: Child,
    pid: u32,
    config_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum LogLevel {
    Debug,
    Info,
    Warning,
    Error,
}

impl LogLevel {
    fn parse(filter: Option<&str>) -> Self {
        match filter.unwrap_or("info").trim().to_ascii_lowercase().as_str() {
            "debug" => Self::Debug,
            "warning" | "warn" => Self::Warning,
            "err" | "error" => Self::Error,
            _ => Self::Info,
        }
    }

    fn detect(line: &str) -> Self {
        let upper = line.to_ascii_uppercase();
        if upper.contains("ERROR") || upper.contains("FATAL") || upper.contains("ERR ") {
            Self::Error
        } else if upper.contains("WARNING") || upper.contains("WARN") {
            Self::Warning
        } else if upper.contains("DEBUG") {
            Self::Debug
        } else {
            Self::Info
        }
    }

    fn allows(self, level: Self) -> bool {
        level >= self
    }
}

#[derive(Debug, Clone)]
struct LogEntry {
    level: LogLevel,
    text: String,
}

#[derive(Default)]
struct CoreState {
    generation: u64,
    running: bool,
    started_at: Option<i64>,
    last_config: Option<String>,
    logs: VecDeque<LogEntry>,
    process: Option<ProcessHandle>,
}

#[derive(Clone, Default)]
pub struct CoreService {
    state: Arc<Mutex<CoreState>>,
}

impl CoreService {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn start_with_config(&self, config: String) -> AppResult<()> {
        let normalized = normalize_config(config)?;
        self.refresh_process_state().await?;

        {
            let state = self.state.lock().await;
            if state.running {
                return Ok(());
            }
        }

        let binary = resolve_sing_box_binary()?;
        let runtime_dir = resolve_runtime_dir()?;
        let config_path = runtime_dir.join(MAIN_CONFIG_FILE);
        write_config_file(&config_path, &normalized).await?;
        check_config(&binary, &config_path).await?;

        let process = self.spawn_process(&binary, &config_path, true).await?;
        let generation = {
            let mut state = self.state.lock().await;
            state.generation = state.generation.saturating_add(1);
            state.running = true;
            state.started_at = Some(OffsetDateTime::now_utc().unix_timestamp());
            state.last_config = Some(normalized);
            state.process = Some(process);
            let pid = state.process.as_ref().map(|item| item.pid).unwrap_or_default();
            push_log(&mut state.logs, LogLevel::Info, format!("sing-box started (pid: {pid})"));
            state.generation
        };

        self.spawn_monitor(generation);
        Ok(())
    }

    pub async fn reload_with_config(&self, config: String) -> AppResult<()> {
        let normalized = normalize_config(config)?;
        self.refresh_process_state().await?;

        let maybe_reload = {
            let state = self.state.lock().await;
            state
                .process
                .as_ref()
                .map(|process| (process.pid, process.config_path.clone(), state.running))
        };

        let Some((pid, config_path, running)) = maybe_reload else {
            return self.start_with_config(normalized).await;
        };
        if !running {
            return self.start_with_config(normalized).await;
        }

        #[cfg(not(unix))]
        {
            return self.restart_with_config(normalized).await;
        }

        #[cfg(unix)]
        {
            let binary = resolve_sing_box_binary()?;
            let temp_path = config_path.with_extension(format!("{}.tmp", Uuid::new_v4()));
            write_config_file(&temp_path, &normalized).await?;
            check_config(&binary, &temp_path).await?;
            tokio::fs::rename(&temp_path, &config_path).await?;
            send_signal("HUP", pid).await?;

            let mut state = self.state.lock().await;
            state.last_config = Some(normalized);
            push_log(&mut state.logs, LogLevel::Info, format!("sing-box reloaded (pid: {pid})"));
            Ok(())
        }
    }

    pub async fn restart_with_config(&self, config: String) -> AppResult<()> {
        self.stop().await?;
        self.start_with_config(config).await
    }

    pub async fn stop(&self) -> AppResult<()> {
        let process = {
            let mut state = self.state.lock().await;
            state.generation = state.generation.saturating_add(1);
            state.running = false;
            state.started_at = None;
            state.process.take()
        };

        let Some(process) = process else {
            return Ok(());
        };
        let pid = process.pid;

        stop_process(process).await?;
        let mut state = self.state.lock().await;
        push_log(&mut state.logs, LogLevel::Info, format!("sing-box stopped (pid: {pid})"));
        Ok(())
    }

    pub async fn status(&self) -> Value {
        let _ = self.refresh_process_state().await;
        let state = self.state.lock().await;
        let uptime = state
            .started_at
            .map(|started_at| OffsetDateTime::now_utc().unix_timestamp().saturating_sub(started_at))
            .unwrap_or_default();
        let (alloc, threads) =
            state.process.as_ref().map(|process| read_process_stats(process.pid)).unwrap_or((0, 0));

        json!({
            "running": state.running,
            "stats": {
                "NumGoroutine": threads,
                "Alloc": alloc,
                "Uptime": uptime,
            }
        })
    }

    pub async fn logs(&self, count: usize, level: Option<&str>) -> Vec<String> {
        let _ = self.refresh_process_state().await;
        let filter = LogLevel::parse(level);
        let state = self.state.lock().await;
        state
            .logs
            .iter()
            .rev()
            .filter(|entry| filter.allows(entry.level))
            .take(count)
            .map(|entry| entry.text.clone())
            .collect()
    }

    pub async fn current_config(&self) -> Option<String> {
        let state = self.state.lock().await;
        state.last_config.clone()
    }

    pub async fn check_outbound(&self, tag: &str, link: Option<&str>) -> Value {
        if tag.trim().is_empty() {
            return json!({
                "OK": false,
                "Delay": 0,
                "Error": "missing query parameter: tag",
            });
        }

        let _ = self.refresh_process_state().await;
        let config = {
            let state = self.state.lock().await;
            if !state.running {
                return json!({
                    "OK": false,
                    "Delay": 0,
                    "Error": "core not running",
                });
            }
            state.last_config.clone()
        };

        let Some(config) = config else {
            return json!({
                "OK": false,
                "Delay": 0,
                "Error": "core config unavailable",
            });
        };

        match self.perform_outbound_check(config, tag, link.unwrap_or_default()).await {
            Ok(delay) => json!({
                "OK": true,
                "Delay": delay,
                "Error": "",
            }),
            Err(error) => json!({
                "OK": false,
                "Delay": 0,
                "Error": error.message(),
            }),
        }
    }

    pub fn generate_keypair(&self, key_type: &str, option: Option<&str>) -> AppResult<Vec<String>> {
        match key_type {
            "tls" => generate_tls_keypair(normalize_option(option, "localhost")),
            "reality" => Ok(generate_reality_keypair()),
            "wireguard" => generate_wireguard_keypair(option),
            "ech" => {
                let server_name = normalize_option(option, "localhost");
                run_sing_box_generate(&["generate", "ech-keypair", server_name.as_str()])
            }
            "" => Err(AppError::Validation("missing keypair type".to_string())),
            other => Err(AppError::Unsupported(format!("unsupported keypair type {other}"))),
        }
    }

    async fn spawn_process(
        &self,
        binary: &Path,
        config_path: &Path,
        attach_logs: bool,
    ) -> AppResult<ProcessHandle> {
        let mut command = Command::new(binary);
        command
            .arg("--disable-color")
            .arg("-c")
            .arg(config_path)
            .arg("run")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = command.spawn()?;
        let pid = child.id().ok_or_else(|| {
            AppError::Unsupported("failed to capture sing-box process id".to_string())
        })?;

        if attach_logs {
            if let Some(stdout) = child.stdout.take() {
                spawn_log_reader(self.clone(), pid, stdout);
            }
            if let Some(stderr) = child.stderr.take() {
                spawn_log_reader(self.clone(), pid, stderr);
            }
        }

        Ok(ProcessHandle { child, pid, config_path: config_path.to_path_buf() })
    }

    fn spawn_monitor(&self, generation: u64) {
        let service = self.clone();
        tokio::spawn(async move {
            loop {
                sleep(PROCESS_MONITOR_INTERVAL).await;
                match service.refresh_process_state().await {
                    Ok(keep_running) => {
                        if !keep_running {
                            break;
                        }
                        let current_generation = {
                            let state = service.state.lock().await;
                            state.generation
                        };
                        if current_generation != generation {
                            break;
                        }
                    }
                    Err(error) => {
                        service
                            .push_log_message(
                                LogLevel::Error,
                                format!("failed to monitor sing-box process: {}", error.message()),
                            )
                            .await;
                        break;
                    }
                }
            }
        });
    }

    async fn refresh_process_state(&self) -> AppResult<bool> {
        let mut exit = None;
        {
            let mut state = self.state.lock().await;
            if let Some(process) = state.process.as_mut() {
                if let Some(status) = process.child.try_wait()? {
                    exit = Some((process.pid, status.success(), status.to_string()));
                }
            } else {
                return Ok(false);
            }

            if let Some((pid, success, status_text)) = exit.as_ref() {
                state.process = None;
                state.running = false;
                state.started_at = None;
                let level = if *success { LogLevel::Info } else { LogLevel::Error };
                let message = if *success {
                    format!("sing-box exited (pid: {pid})")
                } else {
                    format!("sing-box exited abnormally (pid: {pid}, status: {status_text})")
                };
                push_log(&mut state.logs, level, message);
                return Ok(false);
            }

            Ok(state.running)
        }
    }

    async fn perform_outbound_check(
        &self,
        config: String,
        tag: &str,
        link: &str,
    ) -> AppResult<u16> {
        let binary = resolve_sing_box_binary()?;
        let runtime_dir = resolve_runtime_dir()?;
        let port = reserve_local_port()?;
        let probe_config = build_probe_config(config, tag, port)?;
        let probe_path = runtime_dir.join(format!("probe-{}.json", Uuid::new_v4()));
        write_config_file(&probe_path, &probe_config).await?;
        check_config(&binary, &probe_path).await?;

        let logs = Arc::new(Mutex::new(Vec::<String>::new()));
        let mut child = spawn_probe_process(&binary, &probe_path, logs.clone()).await?;
        wait_for_probe_ready(&mut child, port).await.map_err(|error| {
            AppError::Unsupported(format!(
                "failed to boot outbound probe for {tag}: {}",
                enrich_probe_error(error, &logs)
            ))
        })?;

        let delay = match probe_request(port, link).await {
            Ok(delay) => delay,
            Err(error) => {
                let message = enrich_probe_error(error, &logs);
                let _ = stop_probe_process(&mut child).await;
                let _ = tokio::fs::remove_file(&probe_path).await;
                return Err(AppError::Unsupported(message));
            }
        };

        stop_probe_process(&mut child).await?;
        tokio::fs::remove_file(&probe_path).await?;
        Ok(delay)
    }

    async fn push_log_message(&self, level: LogLevel, message: String) {
        let mut state = self.state.lock().await;
        push_log(&mut state.logs, level, message);
    }
}

fn normalize_option(option: Option<&str>, default: &str) -> String {
    option
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "''" && *value != "\"\"")
        .unwrap_or(default)
        .to_string()
}

fn normalize_config(config: String) -> AppResult<String> {
    if config.trim().is_empty() {
        return Err(AppError::Validation("config can not be empty".to_string()));
    }
    serde_json::from_str::<Value>(&config)?;
    Ok(config)
}

async fn write_config_file(path: &Path, config: &str) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(path, config).await?;
    Ok(())
}

async fn check_config(binary: &Path, config_path: &Path) -> AppResult<()> {
    let output = Command::new(binary)
        .arg("--disable-color")
        .arg("-c")
        .arg(config_path)
        .arg("check")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let message = [stderr, stdout]
        .into_iter()
        .find(|value| !value.is_empty())
        .unwrap_or_else(|| "sing-box check failed".to_string());
    Err(AppError::Validation(message))
}

fn resolve_runtime_dir() -> AppResult<PathBuf> {
    if let Ok(value) = env::var("SUI_RUNTIME_DIR") {
        let path = PathBuf::from(value);
        fs::create_dir_all(&path)?;
        return Ok(path);
    }

    let executable_dir = env::current_exe().ok().and_then(|path| path.parent().map(PathBuf::from));
    let current_dir = env::current_dir().ok();
    let candidates = [
        executable_dir.as_ref().map(|dir| dir.join("runtime")),
        current_dir.as_ref().map(|dir| dir.join("runtime")),
    ];

    if let Some(candidate) = candidates.into_iter().flatten().next() {
        fs::create_dir_all(&candidate)?;
        return Ok(candidate);
    }

    Err(AppError::NotFound("unable to resolve runtime directory".to_string()))
}

fn resolve_sing_box_binary() -> AppResult<PathBuf> {
    if let Ok(value) = env::var("SUI_SING_BOX_BIN") {
        let path = PathBuf::from(value);
        if path.is_file() {
            return Ok(path);
        }
    }

    let binary_name = if cfg!(windows) { "sing-box.exe" } else { "sing-box" };
    let executable_dir = env::current_exe().ok().and_then(|path| path.parent().map(PathBuf::from));
    let current_dir = env::current_dir().ok();
    let candidates = [
        executable_dir.as_ref().map(|dir| dir.join(binary_name)),
        current_dir.as_ref().map(|dir| dir.join(binary_name)),
    ];

    for candidate in candidates.into_iter().flatten() {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    if let Some(path) = find_in_path(binary_name) {
        return Ok(path);
    }

    Err(AppError::NotFound(format!(
        "sing-box binary not found, set SUI_SING_BOX_BIN or place {binary_name} next to the app"
    )))
}

fn find_in_path(binary_name: &str) -> Option<PathBuf> {
    env::var_os("PATH").and_then(|paths| {
        env::split_paths(&paths)
            .map(|path| path.join(binary_name))
            .find(|candidate| candidate.is_file())
    })
}

async fn stop_process(process: ProcessHandle) -> AppResult<()> {
    let pid = process.pid;
    let mut child = process.child;

    #[cfg(unix)]
    {
        let _ = send_signal("TERM", pid).await;
    }

    #[cfg(windows)]
    {
        let _ = Command::new("taskkill").args(["/PID", &pid.to_string(), "/T"]).status().await;
    }

    match timeout(PROCESS_SHUTDOWN_TIMEOUT, child.wait()).await {
        Ok(wait_result) => {
            let _ = wait_result?;
            Ok(())
        }
        Err(_) => {
            child.kill().await?;
            let _ = child.wait().await?;
            Ok(())
        }
    }
}

#[cfg(unix)]
async fn send_signal(signal: &str, pid: u32) -> AppResult<()> {
    let status = Command::new("kill")
        .arg(format!("-{signal}"))
        .arg(pid.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?;
    if status.success() {
        Ok(())
    } else {
        Err(AppError::Unsupported(format!("failed to send signal {signal} to sing-box pid {pid}")))
    }
}

fn reserve_local_port() -> AppResult<u16> {
    let listener = StdTcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

fn build_probe_config(config: String, tag: &str, port: u16) -> AppResult<String> {
    let mut root = serde_json::from_str::<Value>(&config)?;
    let object = root
        .as_object_mut()
        .ok_or_else(|| AppError::Validation("config root must be a JSON object".to_string()))?;

    ensure_array_field(object, "inbounds").push(json!({
        "type": "socks",
        "tag": PROBE_INBOUND_TAG,
        "listen": "127.0.0.1",
        "listen_port": port,
    }));

    let route = ensure_object_field(object, "route");
    ensure_array_field(route, "rules").insert(
        0,
        json!({
            "inbound": [PROBE_INBOUND_TAG],
            "action": "route",
            "outbound": tag,
        }),
    );

    serde_json::to_string_pretty(&root).map_err(Into::into)
}

fn ensure_array_field<'a>(object: &'a mut Map<String, Value>, key: &str) -> &'a mut Vec<Value> {
    if !object.get(key).is_some_and(Value::is_array) {
        object.insert(key.to_string(), Value::Array(Vec::new()));
    }
    object.get_mut(key).and_then(Value::as_array_mut).expect("array field must exist")
}

fn ensure_object_field<'a>(
    object: &'a mut Map<String, Value>,
    key: &str,
) -> &'a mut Map<String, Value> {
    if !object.get(key).is_some_and(Value::is_object) {
        object.insert(key.to_string(), Value::Object(Map::new()));
    }
    object.get_mut(key).and_then(Value::as_object_mut).expect("object field must exist")
}

async fn spawn_probe_process(
    binary: &Path,
    config_path: &Path,
    logs: Arc<Mutex<Vec<String>>>,
) -> AppResult<Child> {
    let mut command = Command::new(binary);
    command
        .arg("--disable-color")
        .arg("-c")
        .arg(config_path)
        .arg("run")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = command.spawn()?;
    if let Some(stdout) = child.stdout.take() {
        spawn_probe_reader(logs.clone(), stdout);
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_probe_reader(logs, stderr);
    }
    Ok(child)
}

fn spawn_probe_reader<R>(logs: Arc<Mutex<Vec<String>>>, stream: R)
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(stream).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    if line.trim().is_empty() {
                        continue;
                    }
                    let mut buffer = logs.lock().await;
                    if buffer.len() >= 40 {
                        let _ = buffer.remove(0);
                    }
                    buffer.push(line);
                }
                Ok(None) => break,
                Err(error) => {
                    let mut buffer = logs.lock().await;
                    buffer.push(format!("failed to read sing-box probe log: {error}"));
                    break;
                }
            }
        }
    });
}

async fn wait_for_probe_ready(child: &mut Child, port: u16) -> AppResult<()> {
    let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Err(AppError::Unsupported(format!(
                "probe process exited before becoming ready: {status}"
            )));
        }
        if TcpStream::connect(address).await.is_ok() {
            return Ok(());
        }
        if started.elapsed() >= PROCESS_BOOT_TIMEOUT {
            return Err(AppError::Unsupported("probe startup timed out".to_string()));
        }
        sleep(Duration::from_millis(200)).await;
    }
}

async fn probe_request(port: u16, link: &str) -> AppResult<u16> {
    let target = if link.trim().is_empty() { OUTBOUND_DEFAULT_URL } else { link };
    let proxy = Proxy::all(format!("socks5h://127.0.0.1:{port}"))
        .map_err(|error| AppError::Unsupported(error.to_string()))?;
    let client = reqwest::Client::builder()
        .proxy(proxy)
        .redirect(Policy::none())
        .timeout(OUTBOUND_CHECK_TIMEOUT)
        .build()
        .map_err(|error| AppError::Unsupported(error.to_string()))?;

    let started = Instant::now();
    client
        .request(Method::HEAD, target)
        .send()
        .await
        .map_err(|error| AppError::Unsupported(error.to_string()))?;
    Ok(started.elapsed().as_millis().min(u16::MAX as u128) as u16)
}

async fn stop_probe_process(child: &mut Child) -> AppResult<()> {
    match timeout(PROCESS_SHUTDOWN_TIMEOUT, child.wait()).await {
        Ok(wait_result) => {
            let _ = wait_result?;
            Ok(())
        }
        Err(_) => {
            child.kill().await?;
            let _ = child.wait().await?;
            Ok(())
        }
    }
}

fn enrich_probe_error(error: AppError, logs: &Arc<Mutex<Vec<String>>>) -> String {
    let mut message = error.message();
    if let Ok(buffer) = logs.try_lock() {
        if !buffer.is_empty() {
            message.push_str(" | probe logs: ");
            message.push_str(&buffer.join(" | "));
        }
    }
    message
}

fn generate_tls_keypair(server_name: String) -> AppResult<Vec<String>> {
    let CertifiedKey { cert, signing_key } = generate_simple_self_signed(vec![server_name])
        .map_err(|error| {
            AppError::Validation(format!("failed to generate TLS keypair: {error}"))
        })?;
    Ok(signing_key
        .serialize_pem()
        .lines()
        .chain(cert.pem().lines())
        .map(ToOwned::to_owned)
        .collect())
}

fn generate_wireguard_keypair(option: Option<&str>) -> AppResult<Vec<String>> {
    if let Some(private_key) = option
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "''" && *value != "\"\"")
    {
        let bytes = STANDARD.decode(private_key).map_err(|error| {
            AppError::Validation(format!("invalid wireguard private key: {error}"))
        })?;
        let secret = StaticSecret::from(decode_key_bytes(&bytes)?);
        let public_key = PublicKey::from(&secret);
        return Ok(vec![STANDARD.encode(public_key.to_bytes())]);
    }

    let private_key = StaticSecret::random();
    let public_key = PublicKey::from(&private_key);
    Ok(vec![
        format!("PrivateKey: {}", STANDARD.encode(private_key.to_bytes())),
        format!("PublicKey: {}", STANDARD.encode(public_key.to_bytes())),
    ])
}

fn generate_reality_keypair() -> Vec<String> {
    let private_key = StaticSecret::random();
    let public_key = PublicKey::from(&private_key);
    vec![
        format!("PrivateKey: {}", URL_SAFE_NO_PAD.encode(private_key.to_bytes())),
        format!("PublicKey: {}", URL_SAFE_NO_PAD.encode(public_key.to_bytes())),
    ]
}

fn run_sing_box_generate(args: &[&str]) -> AppResult<Vec<String>> {
    let binary = resolve_sing_box_binary()?;
    let output = std::process::Command::new(binary)
        .arg("--disable-color")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(AppError::Unsupported(if stderr.is_empty() {
            "sing-box generate failed".to_string()
        } else {
            stderr
        }));
    }

    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

fn decode_key_bytes(bytes: &[u8]) -> AppResult<[u8; 32]> {
    bytes.try_into().map_err(|_| {
        AppError::Validation("wireguard private key must be exactly 32 bytes".to_string())
    })
}

fn read_process_stats(pid: u32) -> (u64, u64) {
    let Ok(status) = fs::read_to_string(format!("/proc/{pid}/status")) else {
        return (0, 0);
    };

    let mut rss = 0_u64;
    let mut threads = 0_u64;
    for line in status.lines() {
        if let Some(value) = line.strip_prefix("VmRSS:") {
            rss = parse_status_kib(value);
        } else if let Some(value) = line.strip_prefix("Threads:") {
            threads = value.trim().parse::<u64>().unwrap_or_default();
        }
    }
    (rss, threads)
}

fn parse_status_kib(value: &str) -> u64 {
    value
        .split_whitespace()
        .next()
        .and_then(|part| part.parse::<u64>().ok())
        .map(|kib| kib.saturating_mul(1024))
        .unwrap_or_default()
}

fn push_log(logs: &mut VecDeque<LogEntry>, level: LogLevel, entry: String) {
    let text = entry.trim().to_string();
    if text.is_empty() {
        return;
    }
    if logs.len() >= LOG_LIMIT {
        let _ = logs.pop_front();
    }
    logs.push_back(LogEntry { level, text });
}

fn spawn_log_reader<R>(service: CoreService, pid: u32, stream: R)
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(stream).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    service
                        .push_log_message(
                            LogLevel::detect(line),
                            format!("[sing-box:{pid}] {line}"),
                        )
                        .await;
                }
                Ok(None) => break,
                Err(error) => {
                    service
                        .push_log_message(
                            LogLevel::Warning,
                            format!("failed to read sing-box log stream for pid {pid}: {error}"),
                        )
                        .await;
                    break;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{
        CoreService, LogLevel, build_probe_config, ensure_array_field, normalize_config,
        parse_status_kib,
    };
    use serde_json::{Map, Value, json};

    #[test]
    fn tls_keypair_contains_expected_pem_blocks() {
        let service = CoreService::new();
        let lines = service
            .generate_keypair("tls", Some("example.com"))
            .expect("tls keypair should generate");
        let body = lines.join("\n");
        assert!(body.contains("-----BEGIN PRIVATE KEY-----"));
        assert!(body.contains("-----BEGIN CERTIFICATE-----"));
    }

    #[test]
    fn wireguard_keypair_derives_public_key_from_private_key() {
        let service = CoreService::new();
        let generated =
            service.generate_keypair("wireguard", None).expect("wireguard keypair should generate");
        let private_key = generated[0].trim_start_matches("PrivateKey: ");
        let derived = service
            .generate_keypair("wireguard", Some(private_key))
            .expect("wireguard public key should derive");
        assert_eq!(generated[1].trim_start_matches("PublicKey: "), derived[0]);
    }

    #[test]
    fn probe_config_injects_local_socks_inbound_and_route() {
        let config = r#"{
          "inbounds": [],
          "route": { "rules": [{ "action": "sniff" }] }
        }"#;
        let probe = build_probe_config(config.to_string(), "proxy-out", 17080)
            .expect("probe config should build");
        let json = serde_json::from_str::<Value>(&probe).expect("probe config must be valid json");
        let inbounds = json["inbounds"].as_array().expect("inbounds must be array");
        assert_eq!(inbounds[0]["tag"], "rust-check-socks");
        assert_eq!(inbounds[0]["listen_port"], 17080);
        let rules = json["route"]["rules"].as_array().expect("rules must be array");
        assert_eq!(rules[0]["action"], "route");
        assert_eq!(rules[0]["outbound"], "proxy-out");
    }

    #[test]
    fn normalize_config_rejects_empty_payload() {
        let error = normalize_config("   ".to_string()).expect_err("empty config must fail");
        assert_eq!(error.message(), "config can not be empty");
    }

    #[test]
    fn parse_status_kib_converts_to_bytes() {
        assert_eq!(parse_status_kib("1234 kB"), 1_263_616);
    }

    #[test]
    fn ensure_array_field_initializes_missing_values() {
        let mut object = Map::new();
        ensure_array_field(&mut object, "rules").push(json!({"action": "route"}));
        assert_eq!(object["rules"].as_array().expect("rules array").len(), 1);
    }

    #[test]
    fn log_level_filter_behaves_like_threshold() {
        assert!(LogLevel::Info.allows(LogLevel::Warning));
        assert!(!LogLevel::Warning.allows(LogLevel::Info));
    }
}
