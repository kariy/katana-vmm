use crate::{
    instance::InstanceStatus,
    qemu::{QmpClient, VmManager},
    state::StateDatabase,
};
use anyhow::Result;
use std::io::{self, Write};

pub fn execute(
    name: &str,
    watch: bool,
    interval: u64,
    db: &StateDatabase,
    vm_manager: &VmManager,
) -> Result<()> {
    // Load instance from database
    let state = db.get_instance(name)?;

    // Verify instance is running
    if !matches!(state.status, InstanceStatus::Running) {
        anyhow::bail!("Instance '{}' is not running (status: {:?})", name, state.status);
    }

    // Verify VM process is actually running
    if let Some(pid) = state.vm_pid {
        if !vm_manager.is_process_running(pid) {
            anyhow::bail!("Instance '{}' VM process (PID: {}) is not running", name, pid);
        }
    } else {
        anyhow::bail!("Instance '{}' has no PID recorded", name);
    }

    let qmp_socket = state
        .qmp_socket
        .ok_or_else(|| anyhow::anyhow!("Instance has no QMP socket"))?;

    if watch {
        // Watch mode - continuously update stats
        println!("Watching stats for instance '{}' (Ctrl+C to exit)", name);
        println!("Update interval: {} seconds\n", interval);

        loop {
            // Clear screen (move cursor to top)
            print!("\x1B[2J\x1B[1;1H");
            io::stdout().flush()?;

            display_stats(name, &state.config, &qmp_socket, state.vm_pid)?;

            std::thread::sleep(std::time::Duration::from_secs(interval));
        }
    } else {
        // One-time stats display
        display_stats(name, &state.config, &qmp_socket, state.vm_pid)?;
    }

    Ok(())
}

fn display_stats(
    name: &str,
    config: &crate::instance::InstanceConfig,
    qmp_socket: &std::path::PathBuf,
    pid: Option<i32>,
) -> Result<()> {
    // Connect to QMP socket
    let mut qmp_client = QmpClient::new();
    qmp_client.connect(qmp_socket)?;

    // Query VM status
    let vm_status = qmp_client.query_status()?;

    // Query CPU info
    let cpus = qmp_client.query_cpus()?;

    // Query memory info
    let memory = qmp_client.query_memory()?;

    // Get process stats
    let uptime = if let Some(pid) = pid {
        get_process_uptime(pid)?
    } else {
        "unknown".to_string()
    };

    // Display stats
    println!("===========================================");
    println!(" Instance Statistics: {}", name);
    println!("===========================================");
    println!();
    println!("Status:");
    println!("  State:       {}", vm_status.status);
    println!("  Running:     {}", vm_status.running);
    println!("  PID:         {}", pid.map(|p| p.to_string()).unwrap_or_else(|| "N/A".to_string()));
    println!("  Uptime:      {}", uptime);
    println!();
    println!("Configuration:");
    println!("  vCPUs:       {}", config.vcpus);
    println!("  Memory:      {} MB", config.memory_mb);
    println!("  RPC Port:    {}", config.rpc_port);
    if config.tee_mode {
        println!("  TEE Mode:    AMD SEV-SNP ({})", config.vcpu_type);
    }
    println!();
    println!("Resources:");
    println!("  CPU Count:   {}", cpus.len());
    for cpu in &cpus {
        println!("    CPU {}:     Thread ID {}", cpu.cpu_index, cpu.thread_id);
    }
    println!("  Memory:      {} MB", memory.base_memory / 1024 / 1024);
    println!();
    println!("Network:");
    println!("  RPC:         http://localhost:{}", config.rpc_port);
    println!("  Health:      http://localhost:{}/", config.rpc_port);
    println!();

    Ok(())
}

fn get_process_uptime(pid: i32) -> Result<String> {
    // Read process start time from /proc/<pid>/stat
    let stat_path = format!("/proc/{}/stat", pid);
    let stat_content = std::fs::read_to_string(&stat_path).map_err(|e| {
        anyhow::anyhow!("Failed to read process stats: {}", e)
    })?;

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
