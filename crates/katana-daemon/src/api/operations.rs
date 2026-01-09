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
        InstanceStatus::Running | InstanceStatus::Paused | InstanceStatus::Suspended => {
            // Valid states for stopping
        }
        _ => {
            return Err(ApiError::BadRequest(format!(
                "Cannot stop instance '{}' from state: {}",
                name, instance_state.status
            )));
        }
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

/// Pause an instance
/// POST /api/v1/instances/{name}/pause
pub async fn pause_instance(
    Extension(state): Extension<Arc<DaemonState>>,
    Path(name): Path<String>,
) -> ApiResult<Json<InstanceResponse>> {
    info!(name = %name, "Pausing instance via API");

    // Load instance from database
    let mut instance_state = state.db.get_instance(&name)?;

    // Check if already paused (idempotent)
    if matches!(instance_state.status, InstanceStatus::Paused) {
        info!(name = %name, "Instance already paused");
        return Ok(Json(instance_state_to_response(instance_state)));
    }

    // Check if pausing (conflict)
    if matches!(instance_state.status, InstanceStatus::Pausing) {
        return Err(ApiError::Conflict(format!(
            "Instance '{}' is already pausing",
            name
        )));
    }

    // Validate state transition
    if !instance_state.status.can_pause() {
        return Err(ApiError::BadRequest(format!(
            "Cannot pause instance '{}' from state: {}",
            name, instance_state.status
        )));
    }

    // Get QMP socket
    let qmp_socket = instance_state
        .qmp_socket
        .clone()
        .ok_or_else(|| ApiError::Internal(format!("Instance '{}' has no QMP socket", name)))?;

    // Update status to Pausing
    instance_state.status = InstanceStatus::Pausing;
    state.db.save_instance(&instance_state)?;

    // Pause VM
    state.vm_manager.pause_vm(&qmp_socket).map_err(|e| {
        // Revert status on failure
        instance_state.status = InstanceStatus::Running;
        let _ = state.db.save_instance(&instance_state);
        ApiError::Internal(format!("Failed to pause VM: {}", e))
    })?;

    // Update instance state
    instance_state.status = InstanceStatus::Paused;
    state.db.save_instance(&instance_state)?;

    info!(name = %name, "Instance paused successfully");

    Ok(Json(instance_state_to_response(instance_state)))
}

/// Resume an instance from pause or suspend
/// POST /api/v1/instances/{name}/resume
pub async fn resume_instance(
    Extension(state): Extension<Arc<DaemonState>>,
    Path(name): Path<String>,
) -> ApiResult<Json<InstanceResponse>> {
    info!(name = %name, "Resuming instance via API");

    // Load instance from database
    let mut instance_state = state.db.get_instance(&name)?;

    // Check if already running (idempotent)
    if matches!(instance_state.status, InstanceStatus::Running) {
        info!(name = %name, "Instance already running");
        return Ok(Json(instance_state_to_response(instance_state)));
    }

    // Check if resuming (conflict)
    if matches!(instance_state.status, InstanceStatus::Resuming) {
        return Err(ApiError::Conflict(format!(
            "Instance '{}' is already resuming",
            name
        )));
    }

    // Validate state transition
    if !instance_state.status.can_resume_from_pause() && !instance_state.status.can_wake() {
        return Err(ApiError::BadRequest(format!(
            "Cannot resume instance '{}' from state: {}",
            name, instance_state.status
        )));
    }

    // Get QMP socket
    let qmp_socket = instance_state
        .qmp_socket
        .clone()
        .ok_or_else(|| ApiError::Internal(format!("Instance '{}' has no QMP socket", name)))?;

    let previous_state = instance_state.status.clone();

    // Update status to Resuming
    instance_state.status = InstanceStatus::Resuming;
    state.db.save_instance(&instance_state)?;

    // Resume VM (handles both pause and suspend resume)
    let result = if matches!(previous_state, InstanceStatus::Suspended) {
        state.vm_manager.wake_vm(&qmp_socket)
    } else {
        state.vm_manager.resume_vm(&qmp_socket)
    };

    result.map_err(|e| {
        // Revert status on failure
        instance_state.status = previous_state;
        let _ = state.db.save_instance(&instance_state);
        ApiError::Internal(format!("Failed to resume VM: {}", e))
    })?;

    // Update instance state
    instance_state.status = InstanceStatus::Running;
    state.db.save_instance(&instance_state)?;

    info!(name = %name, "Instance resumed successfully");

    Ok(Json(instance_state_to_response(instance_state)))
}

/// Suspend an instance to RAM
/// POST /api/v1/instances/{name}/suspend
pub async fn suspend_instance(
    Extension(state): Extension<Arc<DaemonState>>,
    Path(name): Path<String>,
) -> ApiResult<Json<InstanceResponse>> {
    info!(name = %name, "Suspending instance via API");

    // Load instance from database
    let mut instance_state = state.db.get_instance(&name)?;

    // Check if already suspended (idempotent)
    if matches!(instance_state.status, InstanceStatus::Suspended) {
        info!(name = %name, "Instance already suspended");
        return Ok(Json(instance_state_to_response(instance_state)));
    }

    // Check if suspending (conflict)
    if matches!(instance_state.status, InstanceStatus::Suspending) {
        return Err(ApiError::Conflict(format!(
            "Instance '{}' is already suspending",
            name
        )));
    }

    // Validate state transition
    if !instance_state.status.can_suspend() {
        return Err(ApiError::BadRequest(format!(
            "Cannot suspend instance '{}' from state: {}",
            name, instance_state.status
        )));
    }

    // Get QMP socket
    let qmp_socket = instance_state
        .qmp_socket
        .clone()
        .ok_or_else(|| ApiError::Internal(format!("Instance '{}' has no QMP socket", name)))?;

    let previous_state = instance_state.status.clone();

    // If paused, need to resume first before suspending
    if matches!(previous_state, InstanceStatus::Paused) {
        info!(name = %name, "Resuming from pause before suspend");
        state.vm_manager.resume_vm(&qmp_socket).map_err(|e| {
            ApiError::Internal(format!("Failed to resume before suspend: {}", e))
        })?;
    }

    // Update status to Suspending
    instance_state.status = InstanceStatus::Suspending;
    state.db.save_instance(&instance_state)?;

    // Suspend VM
    state.vm_manager.suspend_vm(&qmp_socket).map_err(|e| {
        // Revert status on failure
        instance_state.status = previous_state;
        let _ = state.db.save_instance(&instance_state);
        ApiError::Internal(format!("Failed to suspend VM (guest may not support ACPI): {}", e))
    })?;

    // Update instance state
    instance_state.status = InstanceStatus::Suspended;
    state.db.save_instance(&instance_state)?;

    info!(name = %name, "Instance suspended successfully");

    Ok(Json(instance_state_to_response(instance_state)))
}

/// Reset/reboot an instance
/// POST /api/v1/instances/{name}/reset
pub async fn reset_instance(
    Extension(state): Extension<Arc<DaemonState>>,
    Path(name): Path<String>,
) -> ApiResult<Json<InstanceResponse>> {
    info!(name = %name, "Resetting instance via API");

    // Load instance from database
    let mut instance_state = state.db.get_instance(&name)?;

    // Validate state transition
    if !instance_state.status.can_reset() {
        return Err(ApiError::BadRequest(format!(
            "Cannot reset instance '{}' from state: {}",
            name, instance_state.status
        )));
    }

    // Get QMP socket
    let qmp_socket = instance_state
        .qmp_socket
        .clone()
        .ok_or_else(|| ApiError::Internal(format!("Instance '{}' has no QMP socket", name)))?;

    let previous_state = instance_state.status.clone();

    // If paused, resume first
    if matches!(previous_state, InstanceStatus::Paused) {
        info!(name = %name, "Resuming from pause before reset");
        instance_state.status = InstanceStatus::Resuming;
        state.db.save_instance(&instance_state)?;

        state.vm_manager.resume_vm(&qmp_socket).map_err(|e| {
            instance_state.status = previous_state;
            let _ = state.db.save_instance(&instance_state);
            ApiError::Internal(format!("Failed to resume before reset: {}", e))
        })?;
    }

    // Update status to Starting (VM is rebooting)
    instance_state.status = InstanceStatus::Starting;
    state.db.save_instance(&instance_state)?;

    // Reset VM
    state.vm_manager.reset_vm(&qmp_socket).map_err(|e| {
        // On failure, mark as Failed
        let error_msg = format!("Failed to reset VM: {}", e);
        instance_state.status = InstanceStatus::Failed {
            error: error_msg.clone(),
        };
        let _ = state.db.save_instance(&instance_state);
        ApiError::Internal(error_msg)
    })?;

    // VM will reboot - update to Running after reset completes
    instance_state.status = InstanceStatus::Running;
    state.db.save_instance(&instance_state)?;

    info!(name = %name, "Instance reset successfully");

    Ok(Json(instance_state_to_response(instance_state)))
}
