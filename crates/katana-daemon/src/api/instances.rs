use axum::{
    extract::{Extension, Path},
    http::StatusCode,
    response::Json,
};
use byte_unit::Byte;
use katana_core::instance::{BootComponents, InstanceConfig, InstanceState};
use std::sync::Arc;
use tracing::info;
use uuid::Uuid;

use crate::{
    error::{ApiError, ApiResult},
    models::{
        instance_state_to_response, CreateInstanceRequest, InstanceResponse, ListInstancesResponse,
    },
    state::DaemonState,
};

/// Create a new instance
/// POST /api/v1/instances
pub async fn create_instance(
    Extension(state): Extension<Arc<DaemonState>>,
    Json(req): Json<CreateInstanceRequest>,
) -> ApiResult<(StatusCode, Json<InstanceResponse>)> {
    info!(name = %req.name, "Creating instance via API");

    // Validate boot components
    let boot_components = BootComponents::load()
        .map_err(|e| ApiError::Internal(format!("Failed to load boot components: {}", e)))?;

    // Check if instance exists
    if state.db.instance_exists(&req.name)? {
        return Err(ApiError::Conflict(format!(
            "Instance '{}' already exists",
            req.name
        )));
    }

    // Generate instance ID
    let instance_id = Uuid::new_v4().to_string();

    // Parse memory size
    let memory_bytes = Byte::parse_str(&req.memory, true)
        .map_err(|e| ApiError::BadRequest(format!("Invalid memory size '{}': {}", req.memory, e)))?
        .as_u64();
    let memory_mb = memory_bytes / 1024 / 1024;

    // Parse storage size
    let storage_bytes = Byte::parse_str(&req.storage, true)
        .map_err(|e| {
            ApiError::BadRequest(format!("Invalid storage size '{}': {}", req.storage, e))
        })?
        .as_u64();

    // Allocate port
    let rpc_port = if let Some(port) = req.port {
        if !state.port_allocator.is_port_available(port)? {
            return Err(ApiError::Conflict(format!(
                "Port {} is not available",
                port
            )));
        }
        port
    } else {
        state.port_allocator.allocate_port(5050)?
    };

    info!(rpc_port = %rpc_port, "RPC port allocated");

    // Create storage
    state
        .storage
        .create_instance_storage(&instance_id, storage_bytes)?;

    // Get paths
    let paths = state.storage.get_paths(&instance_id);

    // Build extra args
    let mut extra_args = req.extra_args.clone();
    if req.tee {
        extra_args.push("--tee.provider".to_string());
        extra_args.push("sev-snp".to_string());
    }

    // Create instance configuration
    let config = InstanceConfig {
        vcpus: req.vcpus,
        memory_mb,
        storage_bytes,
        rpc_port,
        metrics_port: None,
        tee_mode: req.tee,
        vcpu_type: if req.tee {
            req.vcpu_type.clone()
        } else {
            "host".to_string()
        },
        expected_measurement: None,
        kernel_path: boot_components.kernel_path.clone(),
        initrd_path: boot_components.initrd_path.clone(),
        ovmf_path: Some(boot_components.ovmf_path.clone()),
        data_dir: paths.disk_image.parent().unwrap().to_path_buf(),
        disk_image: Some(paths.disk_image.clone()),
        chain_id: req.chain_id,
        dev_mode: req.dev,
        block_time: req.block_time,
        accounts: req.accounts.or(Some(10)),
        disable_fee: req.disable_fee,
        extra_args,
    };

    // Create instance state
    let mut instance_state = InstanceState::new(instance_id.clone(), req.name.clone(), config);
    instance_state.serial_log = Some(paths.serial_log.clone());
    instance_state.qmp_socket = Some(paths.qmp_socket.clone());

    // Save to database
    state.db.save_instance(&instance_state)?;

    // Reserve port
    state.db.allocate_port(&instance_id, rpc_port, "rpc")?;

    info!(id = %instance_id, name = %req.name, "Instance created successfully");

    Ok((
        StatusCode::CREATED,
        Json(instance_state_to_response(instance_state)),
    ))
}

/// List all instances
/// GET /api/v1/instances
pub async fn list_instances(
    Extension(state): Extension<Arc<DaemonState>>,
) -> ApiResult<Json<ListInstancesResponse>> {
    info!("Listing instances via API");

    let instances = state.db.list_instances()?;
    let total = instances.len();

    let response = ListInstancesResponse {
        instances: instances
            .into_iter()
            .map(instance_state_to_response)
            .collect(),
        total,
    };

    Ok(Json(response))
}

/// Get instance by name
/// GET /api/v1/instances/{name}
pub async fn get_instance(
    Extension(state): Extension<Arc<DaemonState>>,
    Path(name): Path<String>,
) -> ApiResult<Json<InstanceResponse>> {
    info!(name = %name, "Getting instance via API");

    let instance = state.db.get_instance(&name)?;
    Ok(Json(instance_state_to_response(instance)))
}

/// Delete instance
/// DELETE /api/v1/instances/{name}
pub async fn delete_instance(
    Extension(state): Extension<Arc<DaemonState>>,
    Path(name): Path<String>,
) -> ApiResult<StatusCode> {
    info!(name = %name, "Deleting instance via API");

    let instance = state.db.get_instance(&name)?;

    // TODO: Add force parameter via query string
    // For now, don't allow deletion of running instances
    if matches!(
        instance.status,
        katana_core::instance::InstanceStatus::Running
            | katana_core::instance::InstanceStatus::Starting
    ) {
        return Err(ApiError::BadRequest(format!(
            "Cannot delete running instance '{}'. Stop it first.",
            name
        )));
    }

    // Delete storage
    state.storage.delete_instance_storage(&instance.id)?;

    // Delete from database (cascades to ports)
    state.db.delete_instance(&name)?;

    info!(name = %name, "Instance deleted successfully");

    Ok(StatusCode::NO_CONTENT)
}
