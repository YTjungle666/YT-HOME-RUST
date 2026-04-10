use std::{
    collections::{BTreeMap, HashSet},
    fs,
    sync::Arc,
};

use domain_core::CoreService;
use if_addrs::{IfAddr, get_if_addrs};
use infra_db::Db;
use serde_json::{Map, Value, json};
use shared::{AppResult, settings::APP_VERSION};
use sqlx::Row;
use sysinfo::{Disks, Networks, System, get_current_pid};
use tokio::sync::RwLock;

#[derive(Debug)]
struct RuntimeStats {
    system: System,
    disks: Disks,
    networks: Networks,
}

impl RuntimeStats {
    fn new() -> Self {
        let mut system = System::new_all();
        system.refresh_cpu_usage();
        Self {
            system,
            disks: Disks::new_with_refreshed_list(),
            networks: Networks::new_with_refreshed_list(),
        }
    }
}

#[derive(Clone)]
pub struct StatsService {
    pool: Db,
    runtime: Arc<RwLock<RuntimeStats>>,
}

impl StatsService {
    pub fn new(pool: Db) -> Self {
        Self { pool, runtime: Arc::new(RwLock::new(RuntimeStats::new())) }
    }

    pub async fn get_onlines(&self) -> Value {
        json!({
            "inbound": [],
            "outbound": [],
            "user": [],
        })
    }

    pub async fn get_stats(
        &self,
        resource: Option<&str>,
        tag: Option<&str>,
        limit: i64,
    ) -> AppResult<Vec<Value>> {
        let mut sql = String::from(
            "SELECT id, date_time, resource, tag, direction, traffic FROM stats WHERE id > 0",
        );
        let mut binds: Vec<String> = Vec::new();

        if let Some(resource) = resource.filter(|value| !value.is_empty()) {
            sql.push_str(" AND resource = ?");
            binds.push(resource.to_string());
        }
        if let Some(tag) = tag.filter(|value| !value.is_empty()) {
            sql.push_str(" AND tag = ?");
            binds.push(tag.to_string());
        }
        sql.push_str(" ORDER BY id DESC LIMIT ?");

        let mut query = sqlx::query(&sql);
        for bind in binds {
            query = query.bind(bind);
        }
        let rows = query.bind(if limit > 0 { limit } else { 100 }).fetch_all(&self.pool).await?;

        Ok(rows
            .into_iter()
            .map(|row| {
                json!({
                    "id": row.get::<i64, _>("id"),
                    "dateTime": row.get::<i64, _>("date_time"),
                    "resource": row.get::<String, _>("resource"),
                    "tag": row.get::<String, _>("tag"),
                    "direction": row.get::<bool, _>("direction"),
                    "traffic": row.get::<i64, _>("traffic"),
                })
            })
            .collect())
    }

    pub async fn get_status(
        &self,
        request: &str,
        db_info: BTreeMap<String, i64>,
        core: &CoreService,
    ) -> Value {
        let mut runtime = self.runtime.write().await;
        runtime.system.refresh_cpu_usage();
        runtime.system.refresh_memory();
        runtime.disks.refresh(false);
        runtime.networks.refresh(false);

        let cpu = runtime.system.global_cpu_usage() as f64;
        let mem = json!({
            "current": runtime.system.used_memory(),
            "total": runtime.system.total_memory(),
        });
        let swap = json!({
            "current": runtime.system.used_swap(),
            "total": runtime.system.total_swap(),
        });
        let disk = disk_snapshot(&runtime.disks);
        let disk_io = disk_io_snapshot();
        let net = network_snapshot(&runtime.networks);
        let sys = system_snapshot(&runtime.system);

        let mut result = Map::new();
        for item in request.split(',').filter(|item| !item.is_empty()) {
            match item {
                "cpu" => {
                    result.insert("cpu".to_string(), json!(cpu));
                }
                "mem" => {
                    result.insert("mem".to_string(), mem.clone());
                }
                "dsk" => {
                    result.insert("dsk".to_string(), disk.clone());
                }
                "dio" => {
                    result.insert("dio".to_string(), disk_io.clone());
                }
                "swp" => {
                    result.insert("swp".to_string(), swap.clone());
                }
                "net" => {
                    result.insert("net".to_string(), net.clone());
                }
                "sys" => {
                    result.insert("sys".to_string(), sys.clone());
                }
                "sbd" => {
                    result.insert("sbd".to_string(), core.status().await);
                }
                "db" => {
                    result.insert("db".to_string(), json!(db_info));
                }
                _ => {}
            }
        }
        Value::Object(result)
    }
}

fn disk_snapshot(disks: &Disks) -> Value {
    let Some(disk) = disks
        .list()
        .iter()
        .find(|disk| disk.mount_point() == std::path::Path::new("/"))
        .or_else(|| disks.list().iter().max_by_key(|disk| disk.total_space()))
    else {
        return json!({ "current": 0_u64, "total": 0_u64 });
    };

    let total = disk.total_space();
    let current = total.saturating_sub(disk.available_space());
    json!({
        "current": current,
        "total": total,
    })
}

fn network_snapshot(networks: &Networks) -> Value {
    let mut sent = 0_u64;
    let mut recv = 0_u64;
    let mut psent = 0_u64;
    let mut precv = 0_u64;

    for network in networks.list().values() {
        sent = sent.saturating_add(network.total_transmitted());
        recv = recv.saturating_add(network.total_received());
        psent = psent.saturating_add(network.total_packets_transmitted());
        precv = precv.saturating_add(network.total_packets_received());
    }

    json!({
        "sent": sent,
        "recv": recv,
        "psent": psent,
        "precv": precv,
    })
}

fn system_snapshot(system: &System) -> Value {
    let (ipv4, ipv6) = interface_addresses();
    let (app_mem, app_threads) = current_process_snapshot();
    let cpu_type = system
        .cpus()
        .first()
        .map(|cpu| cpu.brand().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let boot_time = instance_boot_time(system);

    json!({
        "appMem": app_mem,
        "appThreads": app_threads,
        "cpuType": cpu_type,
        "cpuCount": system.cpus().len(),
        "hostName": System::host_name().unwrap_or_else(|| "localhost".to_string()),
        "appVersion": APP_VERSION,
        "ipv4": ipv4,
        "ipv6": ipv6,
        "bootTime": boot_time,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ProcessTreeNode {
    start_time: u64,
    parent: Option<u32>,
}

fn resolve_instance_boot_time<F>(current_pid: u32, mut lookup: F) -> Option<u64>
where
    F: FnMut(u32) -> Option<ProcessTreeNode>,
{
    let mut pid = current_pid;
    let mut visited = HashSet::new();
    let mut boot_time = None;

    while visited.insert(pid) {
        let Some(node) = lookup(pid) else {
            break;
        };
        if node.start_time > 0 {
            boot_time = Some(
                boot_time.map_or(node.start_time, |current: u64| current.min(node.start_time)),
            );
        }

        match node.parent {
            Some(parent) if parent != pid => pid = parent,
            _ => break,
        }
    }

    boot_time
}

fn instance_boot_time(system: &System) -> u64 {
    let Ok(current_pid) = get_current_pid() else {
        return System::boot_time();
    };

    resolve_instance_boot_time(current_pid.as_u32(), |pid| {
        system.process(sysinfo::Pid::from_u32(pid)).map(|process| ProcessTreeNode {
            start_time: process.start_time(),
            parent: process.parent().map(|parent| parent.as_u32()),
        })
    })
    .or_else(|| {
        system
            .process(current_pid)
            .map(|process| process.start_time())
            .filter(|start_time| *start_time > 0)
    })
    .unwrap_or_else(System::boot_time)
}

fn interface_addresses() -> (Vec<String>, Vec<String>) {
    let Ok(addresses) = get_if_addrs() else {
        return (Vec::new(), Vec::new());
    };

    let mut ipv4 = Vec::new();
    let mut ipv6 = Vec::new();
    for interface in addresses {
        if interface.is_loopback() {
            continue;
        }
        match interface.addr {
            IfAddr::V4(addr) => ipv4.push(addr.ip.to_string()),
            IfAddr::V6(addr) => {
                let ip = addr.ip.to_string();
                if !ip.starts_with("fe80:") {
                    ipv6.push(ip);
                }
            }
        }
    }
    ipv4.sort();
    ipv4.dedup();
    ipv6.sort();
    ipv6.dedup();
    (ipv4, ipv6)
}

fn current_process_snapshot() -> (u64, u64) {
    let Ok(status) = fs::read_to_string("/proc/self/status") else {
        return (0, 0);
    };

    let mut app_mem = 0_u64;
    let mut app_threads = 0_u64;
    for line in status.lines() {
        if let Some(value) = line.strip_prefix("VmRSS:") {
            app_mem = parse_status_kib(value);
        } else if let Some(value) = line.strip_prefix("Threads:") {
            app_threads = value.trim().parse::<u64>().unwrap_or_default();
        }
    }
    (app_mem, app_threads)
}

fn parse_status_kib(value: &str) -> u64 {
    value
        .split_whitespace()
        .next()
        .and_then(|part| part.parse::<u64>().ok())
        .map(|kib| kib.saturating_mul(1024))
        .unwrap_or_default()
}

fn disk_io_snapshot() -> Value {
    let Ok(content) = fs::read_to_string("/proc/diskstats") else {
        return json!({ "read": 0_u64, "write": 0_u64 });
    };

    let mut read = 0_u64;
    let mut write = 0_u64;
    for line in content.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 14 {
            continue;
        }
        let Some(name) = fields.get(2) else {
            continue;
        };
        if name.starts_with("loop")
            || name.starts_with("ram")
            || name.starts_with("fd")
            || name.starts_with("sr")
        {
            continue;
        }

        let sectors_read =
            fields.get(5).and_then(|value| value.parse::<u64>().ok()).unwrap_or_default();
        let sectors_written =
            fields.get(9).and_then(|value| value.parse::<u64>().ok()).unwrap_or_default();
        read = read.saturating_add(sectors_read.saturating_mul(512));
        write = write.saturating_add(sectors_written.saturating_mul(512));
    }

    json!({
        "read": read,
        "write": write,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{ProcessTreeNode, resolve_instance_boot_time};

    #[test]
    fn resolve_instance_boot_time_uses_oldest_ancestor_start() {
        let nodes = HashMap::from([
            (301_u32, ProcessTreeNode { start_time: 3000, parent: Some(201) }),
            (201_u32, ProcessTreeNode { start_time: 2000, parent: Some(101) }),
            (101_u32, ProcessTreeNode { start_time: 1000, parent: None }),
        ]);

        let boot_time = resolve_instance_boot_time(301, |pid| nodes.get(&pid).copied());

        assert_eq!(boot_time, Some(1000));
    }

    #[test]
    fn resolve_instance_boot_time_keeps_last_known_when_parent_is_missing() {
        let nodes = HashMap::from([
            (301_u32, ProcessTreeNode { start_time: 3000, parent: Some(201) }),
            (201_u32, ProcessTreeNode { start_time: 2000, parent: Some(101) }),
        ]);

        let boot_time = resolve_instance_boot_time(301, |pid| nodes.get(&pid).copied());

        assert_eq!(boot_time, Some(2000));
    }

    #[test]
    fn resolve_instance_boot_time_breaks_parent_cycles() {
        let nodes = HashMap::from([
            (301_u32, ProcessTreeNode { start_time: 3000, parent: Some(201) }),
            (201_u32, ProcessTreeNode { start_time: 2000, parent: Some(301) }),
        ]);

        let boot_time = resolve_instance_boot_time(301, |pid| nodes.get(&pid).copied());

        assert_eq!(boot_time, Some(2000));
    }
}
