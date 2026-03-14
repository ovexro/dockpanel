use axum::{extract::State, routing::get, Json, Router};
use serde::Serialize;
use sysinfo::{Components, Networks, System};

use super::AppState;

#[derive(Serialize)]
struct SystemInfo {
    hostname: String,
    os: String,
    kernel: String,
    uptime_secs: u64,
    cpu_count: usize,
    cpu_usage: f32,
    cpu_model: String,
    cpu_temp: Option<f32>,
    mem_total_mb: u64,
    mem_used_mb: u64,
    mem_usage_pct: f32,
    swap_total_mb: u64,
    swap_used_mb: u64,
    disk_total_gb: f64,
    disk_used_gb: f64,
    disk_usage_pct: f32,
    load_avg_1: f64,
    load_avg_5: f64,
    load_avg_15: f64,
    process_count: usize,
}

#[derive(Serialize)]
struct ProcessInfo {
    pid: u32,
    name: String,
    cpu_pct: f32,
    mem_mb: u64,
}

#[derive(Serialize)]
struct NetworkInfo {
    name: String,
    rx_bytes: u64,
    tx_bytes: u64,
}

async fn system_info(State(state): State<AppState>) -> Json<SystemInfo> {
    let mut sys = state.system.lock().await;
    sys.refresh_cpu_usage();
    sys.refresh_memory();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

    let cpu_usage = sys.global_cpu_usage();
    let mem_total = sys.total_memory();
    let mem_used = sys.used_memory();

    // Disk info for root partition
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let (disk_total, disk_used) = disks
        .iter()
        .find(|d| d.mount_point() == std::path::Path::new("/"))
        .map(|d| (d.total_space(), d.total_space() - d.available_space()))
        .unwrap_or((0, 0));

    let load_avg = System::load_average();

    // CPU model from first core
    let cpu_model = sys
        .cpus()
        .first()
        .map(|c| c.brand().to_string())
        .unwrap_or_default();

    // CPU temperature — find the highest package/core/tctl reading
    let components = Components::new_with_refreshed_list();
    let cpu_temp = components
        .iter()
        .filter(|c| {
            let label = c.label().to_lowercase();
            label.contains("core")
                || label.contains("cpu")
                || label.contains("package")
                || label.contains("tctl")
        })
        .filter_map(|c| c.temperature())
        .reduce(|a, b| a.max(b));

    // Swap memory
    let swap_total = sys.total_swap();
    let swap_used = sys.used_swap();

    // Process count
    let process_count = sys.processes().len();

    Json(SystemInfo {
        hostname: System::host_name().unwrap_or_default(),
        os: System::long_os_version().unwrap_or_default(),
        kernel: System::kernel_version().unwrap_or_default(),
        uptime_secs: System::uptime(),
        cpu_count: sys.cpus().len(),
        cpu_usage,
        cpu_model,
        cpu_temp,
        mem_total_mb: mem_total / 1_048_576,
        mem_used_mb: mem_used / 1_048_576,
        mem_usage_pct: if mem_total > 0 {
            (mem_used as f32 / mem_total as f32) * 100.0
        } else {
            0.0
        },
        swap_total_mb: swap_total / 1_048_576,
        swap_used_mb: swap_used / 1_048_576,
        disk_total_gb: disk_total as f64 / 1_073_741_824.0,
        disk_used_gb: disk_used as f64 / 1_073_741_824.0,
        disk_usage_pct: if disk_total > 0 {
            (disk_used as f32 / disk_total as f32) * 100.0
        } else {
            0.0
        },
        load_avg_1: load_avg.one,
        load_avg_5: load_avg.five,
        load_avg_15: load_avg.fifteen,
        process_count,
    })
}

async fn processes() -> Json<Vec<ProcessInfo>> {
    let mut sys = System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    // Brief pause then refresh again for accurate CPU readings
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

    let mut procs: Vec<ProcessInfo> = sys
        .processes()
        .values()
        .map(|p| ProcessInfo {
            pid: p.pid().as_u32(),
            name: p.name().to_string_lossy().to_string(),
            cpu_pct: p.cpu_usage(),
            mem_mb: p.memory() / 1_048_576,
        })
        .collect();

    procs.sort_by(|a, b| b.cpu_pct.partial_cmp(&a.cpu_pct).unwrap_or(std::cmp::Ordering::Equal));
    procs.truncate(20);

    Json(procs)
}

async fn network() -> Json<Vec<NetworkInfo>> {
    let networks = Networks::new_with_refreshed_list();

    let interfaces: Vec<NetworkInfo> = networks
        .iter()
        .map(|(name, data)| NetworkInfo {
            name: name.to_string(),
            rx_bytes: data.total_received(),
            tx_bytes: data.total_transmitted(),
        })
        .collect();

    Json(interfaces)
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/system/info", get(system_info))
        .route("/system/processes", get(processes))
        .route("/system/network", get(network))
}
