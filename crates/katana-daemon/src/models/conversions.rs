use chrono::DateTime;
use katana_core::instance::{InstanceState, InstanceStatus};
use katana_models::{EndpointsResponse, InstanceConfigResponse, InstanceResponse};

/// Convert InstanceState from core to InstanceResponse for API
pub fn instance_state_to_response(state: InstanceState) -> InstanceResponse {
    let status_str = match &state.status {
        InstanceStatus::Created => "Created",
        InstanceStatus::Starting => "Starting",
        InstanceStatus::Running => "Running",
        InstanceStatus::Pausing => "Pausing",
        InstanceStatus::Paused => "Paused",
        InstanceStatus::Resuming => "Resuming",
        InstanceStatus::Suspending => "Suspending",
        InstanceStatus::Suspended => "Suspended",
        InstanceStatus::Stopping => "Stopping",
        InstanceStatus::Stopped => "Stopped",
        InstanceStatus::Failed { error: _ } => "Failed",
    }
    .to_string();

    let endpoints = if matches!(state.status, InstanceStatus::Running) {
        Some(EndpointsResponse {
            rpc: format!("http://localhost:{}", state.config.rpc_port),
            metrics: state
                .config
                .metrics_port
                .map(|p| format!("http://localhost:{}", p)),
        })
    } else {
        None
    };

    InstanceResponse {
        id: state.id,
        name: state.name,
        status: status_str,
        config: InstanceConfigResponse {
            vcpus: state.config.vcpus,
            memory_mb: state.config.memory_mb,
            storage_bytes: state.config.storage_bytes,
            rpc_port: state.config.rpc_port,
            metrics_port: state.config.metrics_port,
            tee_mode: state.config.tee_mode,
        },
        created_at: DateTime::from_timestamp(state.created_at, 0)
            .unwrap_or_default()
            .to_rfc3339(),
        updated_at: DateTime::from_timestamp(state.updated_at, 0)
            .unwrap_or_default()
            .to_rfc3339(),
        endpoints,
    }
}
