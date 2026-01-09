use crate::{error::ApiError, state::DaemonState};
use axum::{extract::Path, response::Json, Extension};
use katana_core::{instance::InstanceStatus, qemu::{ManagedVm, QmpClient}};
use serde::Serialize;
use std::sync::Arc;

type ApiResult<T> = Result<T, ApiError>;

#[derive(Debug, Serialize)]
pub struct StatsResponse {
    pub instance_name: String,
    pub status: StatusInfo,
    pub config: ConfigInfo,
    pub resources: ResourcesInfo,
    pub network: NetworkInfo,
}

#[derive(Debug, Serialize)]
pub struct StatusInfo {
    pub state: String,
    pub running: bool,
    pub pid: Option<i32>,
    pub uptime: String,
}

#[derive(Debug, Serialize)]
pub struct ConfigInfo {
    pub vcpus: u32,
    pub memory_mb: u64,
    pub rpc_port: u16,
    pub tee_mode: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ResourcesInfo {
    pub cpu_count: usize,
    pub cpus: Vec<CpuInfo>,
    pub memory_mb: u64,
}

#[derive(Debug, Serialize)]
pub struct CpuInfo {
    pub cpu_index: u64,
    pub thread_id: u64,
}

#[derive(Debug, Serialize)]
pub struct NetworkInfo {
    pub rpc_url: String,
    pub health_url: String,
}

/// Get stats for an instance
pub async fn get_stats(
    Extension(state): Extension<Arc<DaemonState>>,
    Path(name): Path<String>,
) -> ApiResult<Json<StatsResponse>> {
    // Load instance from database
    let instance_state = state.db.get_instance(&name)?;

    // Verify instance is running
    if !matches!(instance_state.status, InstanceStatus::Running) {
        return Err(ApiError::BadRequest(format!(
            "Instance '{}' is not running (status: {:?})",
            name, instance_state.status
        )));
    }

    // Verify VM process is actually running using ManagedVm
    let managed_vm = ManagedVm::from_instance(&instance_state.id, &state.db)
        .map_err(|e| ApiError::Internal(format!("Failed to load VM instance: {}", e)))?;

    if !managed_vm.is_running() {
        return Err(ApiError::BadRequest(format!(
            "Instance '{}' VM process is not running",
            name
        )));
    }

    let pid = managed_vm.pid();

    let qmp_socket = instance_state.qmp_socket.ok_or_else(|| {
        ApiError::NotFound("Instance has no QMP socket".to_string())
    })?;

    // Connect to QMP socket and query stats in a blocking thread pool
    // (QMP client uses its own runtime and can't be called from within async context)
    let qmp_socket_clone = qmp_socket.clone();
    let (vm_status, cpus, memory) = tokio::task::spawn_blocking(move || {
        let mut qmp_client = QmpClient::new();
        qmp_client.connect(&qmp_socket_clone).map_err(|e| {
            ApiError::Internal(format!("Failed to connect to QMP socket: {}", e))
        })?;

        let vm_status = qmp_client.query_status().map_err(|e| {
            ApiError::Internal(format!("Failed to query VM status: {}", e))
        })?;

        let cpus = qmp_client.query_cpus().map_err(|e| {
            ApiError::Internal(format!("Failed to query CPUs: {}", e))
        })?;

        let memory = qmp_client.query_memory().map_err(|e| {
            ApiError::Internal(format!("Failed to query memory: {}", e))
        })?;

        Ok::<_, ApiError>((vm_status, cpus, memory))
    })
    .await
    .map_err(|e| ApiError::Internal(format!("Failed to spawn blocking task: {}", e)))??;

    // Get process uptime
    let uptime = if let Some(pid) = pid {
        get_process_uptime(pid).unwrap_or_else(|_| "unknown".to_string())
    } else {
        "unknown".to_string()
    };

    // Build response
    let tee_mode = if instance_state.config.tee_mode {
        Some(format!("AMD SEV-SNP ({})", instance_state.config.vcpu_type))
    } else {
        None
    };

    let response = StatsResponse {
        instance_name: name,
        status: StatusInfo {
            state: vm_status.status,
            running: vm_status.running,
            pid,
            uptime,
        },
        config: ConfigInfo {
            vcpus: instance_state.config.vcpus,
            memory_mb: instance_state.config.memory_mb,
            rpc_port: instance_state.config.rpc_port,
            tee_mode,
        },
        resources: ResourcesInfo {
            cpu_count: cpus.len(),
            cpus: cpus
                .into_iter()
                .map(|cpu| CpuInfo {
                    cpu_index: cpu.cpu_index,
                    thread_id: cpu.thread_id,
                })
                .collect(),
            memory_mb: memory.base_memory / 1024 / 1024,
        },
        network: NetworkInfo {
            rpc_url: format!("http://localhost:{}", instance_state.config.rpc_port),
            health_url: format!("http://localhost:{}/", instance_state.config.rpc_port),
        },
    };

    Ok(Json(response))
}

fn get_process_uptime(pid: i32) -> Result<String, std::io::Error> {
    // Read process start time from /proc/<pid>/stat
    let stat_path = format!("/proc/{}/stat", pid);
    let stat_content = std::fs::read_to_string(&stat_path)?;

    // Parse stat file (22nd field is starttime in clock ticks)
    let fields: Vec<&str> = stat_content.split_whitespace().collect();
    if fields.len() < 22 {
        return Ok("unknown".to_string());
    }

    let starttime_ticks: u64 = fields[21].parse().unwrap_or(0);

    // Get system uptime
    let uptime_content = std::fs::read_to_string("/proc/uptime")?;
    let uptime_secs: f64 = uptime_content
        .split_whitespace()
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);

    // Get clock ticks per second
    let ticks_per_sec = 100; // Usually 100 on Linux

    // Calculate process uptime
    let process_start_secs = starttime_ticks as f64 / ticks_per_sec as f64;
    let process_uptime_secs = uptime_secs - process_start_secs;

    // Format uptime
    let hours = (process_uptime_secs / 3600.0) as u64;
    let minutes = ((process_uptime_secs % 3600.0) / 60.0) as u64;
    let seconds = (process_uptime_secs % 60.0) as u64;

    if hours > 0 {
        Ok(format!("{}h {}m {}s", hours, minutes, seconds))
    } else if minutes > 0 {
        Ok(format!("{}m {}s", minutes, seconds))
    } else {
        Ok(format!("{}s", seconds))
    }
}
