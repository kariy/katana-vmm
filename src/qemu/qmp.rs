use crate::{HypervisorError, Result};
use qmp::{Client, Endpoint};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::runtime::Runtime;

pub struct QmpClient {
    runtime: Runtime,
    client: Option<Client>,
}

impl QmpClient {
    pub fn new() -> Self {
        let runtime = Runtime::new().expect("Failed to create tokio runtime");
        Self {
            runtime,
            client: None,
        }
    }

    /// Connect to QMP socket
    pub fn connect(&mut self, socket_path: &Path) -> Result<()> {
        let socket_path = socket_path.to_path_buf();

        let client = self.runtime.block_on(async {
            Client::connect(Endpoint::unix(socket_path))
                .await
                .map_err(|e| {
                    HypervisorError::QemuFailed(format!("Failed to connect to QMP socket: {}", e))
                })
        })?;

        self.client = Some(client);
        Ok(())
    }

    /// Query VM status
    pub fn query_status(&mut self) -> Result<VmStatus> {
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| HypervisorError::QemuFailed("Not connected to QMP".to_string()))?;

        let response: serde_json::Value = self.runtime.block_on(async {
            client
                .execute("query-status", Option::<()>::None)
                .await
                .map_err(|e| HypervisorError::QemuFailed(format!("QMP query-status failed: {}", e)))
        })?;

        let status: VmStatus = serde_json::from_value(response).map_err(|e| {
            HypervisorError::QemuFailed(format!("Failed to parse VM status: {}", e))
        })?;

        Ok(status)
    }

    /// Query CPU information
    pub fn query_cpus(&mut self) -> Result<Vec<CpuInfo>> {
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| HypervisorError::QemuFailed("Not connected to QMP".to_string()))?;

        let response: serde_json::Value = self.runtime.block_on(async {
            client
                .execute("query-cpus-fast", Option::<()>::None)
                .await
                .map_err(|e| HypervisorError::QemuFailed(format!("QMP query-cpus-fast failed: {}", e)))
        })?;

        let cpus: Vec<CpuInfo> = serde_json::from_value(response).map_err(|e| {
            HypervisorError::QemuFailed(format!("Failed to parse CPU info: {}", e))
        })?;

        Ok(cpus)
    }

    /// Query memory information
    pub fn query_memory(&mut self) -> Result<MemoryInfo> {
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| HypervisorError::QemuFailed("Not connected to QMP".to_string()))?;

        let response: serde_json::Value = self.runtime.block_on(async {
            client
                .execute("query-memory-size-summary", Option::<()>::None)
                .await
                .map_err(|e| {
                    HypervisorError::QemuFailed(format!("QMP query-memory-size-summary failed: {}", e))
                })
        })?;

        let memory: MemoryInfo = serde_json::from_value(response).map_err(|e| {
            HypervisorError::QemuFailed(format!("Failed to parse memory info: {}", e))
        })?;

        Ok(memory)
    }

    /// Send system_powerdown command
    pub fn system_powerdown(&mut self) -> Result<()> {
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| HypervisorError::QemuFailed("Not connected to QMP".to_string()))?;

        self.runtime.block_on(async {
            client
                .execute::<(), ()>("system_powerdown", None)
                .await
                .map_err(|e| {
                    HypervisorError::QemuFailed(format!("QMP system_powerdown failed: {}", e))
                })
        })?;

        Ok(())
    }

    /// Send quit command
    pub fn quit(&mut self) -> Result<()> {
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| HypervisorError::QemuFailed("Not connected to QMP".to_string()))?;

        self.runtime.block_on(async {
            client
                .execute::<(), ()>("quit", None)
                .await
                .map_err(|e| {
                    HypervisorError::QemuFailed(format!("QMP quit failed: {}", e))
                })
        })?;

        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct VmStatus {
    pub status: String,
    pub running: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CpuInfo {
    #[serde(rename = "cpu-index")]
    pub cpu_index: u64,
    #[serde(rename = "qom-path")]
    pub qom_path: Option<String>,
    #[serde(rename = "thread-id")]
    pub thread_id: u64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct MemoryInfo {
    #[serde(rename = "base-memory")]
    pub base_memory: u64,
}
