use axum::{
    extract::{Extension, Path},
    response::Json,
};
use katana_core::{
    instance::InstanceStatus,
    qemu::{config::{QemuConfig, SevSnpConfig}, ManagedVm},
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

    // Launch VM using ManagedVm (automatically handles state tracking)
    let mut managed_vm = ManagedVm::new(
        instance_state.id.clone(),
        qemu_config,
        state.db.clone(),
    );

    managed_vm.launch().map_err(|e| {
        ApiError::Internal(format!("Failed to launch VM: {}", e))
    })?;

    // Reload instance state from database (updated by ManagedVm)
    instance_state = state.db.get_instance(&name)?;

    info!(
        name = %name,
        pid = ?instance_state.vm_pid,
        "Instance started successfully"
    );

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

    // Get PID for logging
    let pid = instance_state.vm_pid;

    info!(name = %name, pid = ?pid, "Stopping VM");

    // Stop VM using ManagedVm (automatically handles state tracking)
    let mut managed_vm = ManagedVm::from_instance(&instance_state.id, &state.db)
        .map_err(|e| ApiError::Internal(format!("Failed to load VM instance: {}", e)))?;

    managed_vm.stop(30).map_err(|e| {
        ApiError::Internal(format!("Failed to stop VM: {}", e))
    })?;

    // Reload instance state from database (updated by ManagedVm)
    instance_state = state.db.get_instance(&name)?;

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

    // Pause VM using ManagedVm (automatically handles state tracking)
    let managed_vm = ManagedVm::from_instance(&instance_state.id, &state.db)
        .map_err(|e| ApiError::Internal(format!("Failed to load VM instance: {}", e)))?;

    managed_vm.pause().map_err(|e| {
        ApiError::Internal(format!("Failed to pause VM: {}", e))
    })?;

    // Reload instance state from database (updated by ManagedVm)
    instance_state = state.db.get_instance(&name)?;

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

    let is_suspended = matches!(instance_state.status, InstanceStatus::Suspended);

    // Resume VM using ManagedVm (automatically handles state tracking)
    let managed_vm = ManagedVm::from_instance(&instance_state.id, &state.db)
        .map_err(|e| ApiError::Internal(format!("Failed to load VM instance: {}", e)))?;

    // Resume VM (handles both pause and suspend resume)
    if is_suspended {
        managed_vm.wake().map_err(|e| {
            ApiError::Internal(format!("Failed to wake VM: {}", e))
        })?;
    } else {
        managed_vm.resume().map_err(|e| {
            ApiError::Internal(format!("Failed to resume VM: {}", e))
        })?;
    }

    // Reload instance state from database (updated by ManagedVm)
    instance_state = state.db.get_instance(&name)?;

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

    let is_paused = matches!(instance_state.status, InstanceStatus::Paused);

    // Suspend VM using ManagedVm (automatically handles state tracking)
    let managed_vm = ManagedVm::from_instance(&instance_state.id, &state.db)
        .map_err(|e| ApiError::Internal(format!("Failed to load VM instance: {}", e)))?;

    // If paused, need to resume first before suspending
    if is_paused {
        info!(name = %name, "Resuming from pause before suspend");
        managed_vm.resume().map_err(|e| {
            ApiError::Internal(format!("Failed to resume before suspend: {}", e))
        })?;
    }

    // Suspend VM
    managed_vm.suspend().map_err(|e| {
        ApiError::Internal(format!("Failed to suspend VM (guest may not support ACPI): {}", e))
    })?;

    // Reload instance state from database (updated by ManagedVm)
    instance_state = state.db.get_instance(&name)?;

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

    let is_paused = matches!(instance_state.status, InstanceStatus::Paused);

    // Reset VM using ManagedVm (automatically handles state tracking)
    let managed_vm = ManagedVm::from_instance(&instance_state.id, &state.db)
        .map_err(|e| ApiError::Internal(format!("Failed to load VM instance: {}", e)))?;

    // If paused, resume first
    if is_paused {
        info!(name = %name, "Resuming from pause before reset");
        managed_vm.resume().map_err(|e| {
            ApiError::Internal(format!("Failed to resume before reset: {}", e))
        })?;
    }

    // Reset VM
    managed_vm.reset().map_err(|e| {
        ApiError::Internal(format!("Failed to reset VM: {}", e))
    })?;

    // Reload instance state from database (updated by ManagedVm)
    instance_state = state.db.get_instance(&name)?;

    info!(name = %name, "Instance reset successfully");

    Ok(Json(instance_state_to_response(instance_state)))
}
