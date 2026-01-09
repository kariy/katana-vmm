use axum::{
    extract::{Extension, Path},
    response::Json,
};
use katana_core::{
    instance::InstanceStatus,
    qemu::config::{QemuConfig, SevSnpConfig},
};
use std::sync::Arc;
use tracing::info;

use crate::{
    error::{ApiError, ApiResult},
    models::{instance_state_to_response, InstanceResponse},
    state::DaemonState,
};

/// Start an instance
/// POST /api/v1/instances/{name}/start
pub async fn start_instance(
    Extension(state): Extension<Arc<DaemonState>>,
    Path(name): Path<String>,
) -> ApiResult<Json<InstanceResponse>> {
    info!(name = %name, "Starting instance via API");

    // Load instance from database
    let mut instance_state = state.db.get_instance(&name)?;

    // Check state
    match instance_state.status {
        InstanceStatus::Running => {
            info!(name = %name, "Instance already running");
            return Ok(Json(instance_state_to_response(instance_state)));
        }
        InstanceStatus::Starting => {
            return Err(ApiError::Conflict(format!(
                "Instance '{}' is already starting",
                name
            )));
        }
        _ => {}
    }

    // Check if boot components exist
    if !instance_state.config.kernel_path.exists() {
        return Err(ApiError::Internal(format!(
            "Kernel not found at {}",
            instance_state.config.kernel_path.display()
        )));
    }

    if !instance_state.config.initrd_path.exists() {
        return Err(ApiError::Internal(format!(
            "Initrd not found at {}",
            instance_state.config.initrd_path.display()
        )));
    }

    // Update status to Starting
    instance_state.status = InstanceStatus::Starting;
    state.db.save_instance(&instance_state)?;

    // Build katana arguments
    let katana_args = instance_state.config.build_katana_args();
    let kernel_cmdline = QemuConfig::build_kernel_cmdline(&katana_args);

    // Build SEV-SNP config if TEE mode is enabled
    let sev_snp_config = if instance_state.config.tee_mode {
        Some(SevSnpConfig {
            cbitpos: 51,
            reduced_phys_bits: 1,
            vcpu_type: instance_state.config.vcpu_type.clone(),
        })
    } else {
        None
    };

    // Build QEMU configuration
    let qemu_config = QemuConfig {
        memory_mb: instance_state.config.memory_mb,
        vcpus: instance_state.config.vcpus,
        cpu_type: instance_state.config.vcpu_type.clone(),
        kernel_path: instance_state.config.kernel_path.clone(),
        initrd_path: instance_state.config.initrd_path.clone(),
        bios_path: instance_state.config.ovmf_path.clone(),
        kernel_cmdline,
        rpc_port: instance_state.config.rpc_port,
        disk_image: instance_state.config.disk_image.clone(),
        qmp_socket: instance_state.qmp_socket.clone().unwrap(),
        serial_log: instance_state.serial_log.clone().unwrap(),
        pid_file: std::path::PathBuf::from(format!(
            "/tmp/katana-hypervisor-{}.pid",
            instance_state.id
        )),
        sev_snp: sev_snp_config,
        enable_kvm: true,
    };

    info!(
        name = %name,
        vcpus = %qemu_config.vcpus,
        memory_mb = %qemu_config.memory_mb,
        rpc_port = %qemu_config.rpc_port,
        "Launching VM"
    );

    // Launch VM
    let pid = state.vm_manager.launch_vm(&qemu_config).map_err(|e| {
        // Revert status on failure
        let error_msg = format!("Failed to launch VM: {}", e);
        instance_state.status = InstanceStatus::Failed {
            error: error_msg.clone(),
        };
        let _ = state.db.save_instance(&instance_state);
        ApiError::Internal(error_msg)
    })?;

    // Update instance state
    instance_state.status = InstanceStatus::Running;
    instance_state.vm_pid = Some(pid);
    state.db.save_instance(&instance_state)?;

    info!(name = %name, pid = %pid, "Instance started successfully");

    Ok(Json(instance_state_to_response(instance_state)))
}

/// Stop an instance
/// POST /api/v1/instances/{name}/stop
pub async fn stop_instance(
    Extension(state): Extension<Arc<DaemonState>>,
    Path(name): Path<String>,
) -> ApiResult<Json<InstanceResponse>> {
    info!(name = %name, "Stopping instance via API");

    // Load instance from database
    let mut instance_state = state.db.get_instance(&name)?;

    // Check state
    match instance_state.status {
        InstanceStatus::Stopped | InstanceStatus::Created | InstanceStatus::Failed { .. } => {
            info!(name = %name, status = ?instance_state.status, "Instance already stopped");
            return Ok(Json(instance_state_to_response(instance_state)));
        }
        InstanceStatus::Stopping => {
            return Err(ApiError::Conflict(format!(
                "Instance '{}' is already stopping",
                name
            )));
        }
        _ => {}
    }

    // Get PID
    let pid = instance_state
        .vm_pid
        .ok_or_else(|| ApiError::Internal(format!("Instance '{}' has no PID", name)))?;

    // Update status to Stopping
    instance_state.status = InstanceStatus::Stopping;
    state.db.save_instance(&instance_state)?;

    info!(name = %name, pid = %pid, "Stopping VM");

    // Stop VM
    state.vm_manager.stop_vm(pid, 30).map_err(|e| {
        // Revert status on failure
        instance_state.status = InstanceStatus::Running;
        let _ = state.db.save_instance(&instance_state);
        ApiError::Internal(format!("Failed to stop VM: {}", e))
    })?;

    // Update instance state
    instance_state.status = InstanceStatus::Stopped;
    instance_state.vm_pid = None;
    state.db.save_instance(&instance_state)?;

    info!(name = %name, "Instance stopped successfully");

    Ok(Json(instance_state_to_response(instance_state)))
}
