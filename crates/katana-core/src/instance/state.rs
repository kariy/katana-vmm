use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum InstanceStatus {
    Created,
    Starting,
    Running,
    Pausing,
    Paused,
    Resuming,
    Suspending,
    Suspended,
    Stopping,
    Stopped,
    Failed { error: String },
}

impl std::fmt::Display for InstanceStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstanceStatus::Created => write!(f, "created"),
            InstanceStatus::Starting => write!(f, "starting"),
            InstanceStatus::Running => write!(f, "running"),
            InstanceStatus::Pausing => write!(f, "pausing"),
            InstanceStatus::Paused => write!(f, "paused"),
            InstanceStatus::Resuming => write!(f, "resuming"),
            InstanceStatus::Suspending => write!(f, "suspending"),
            InstanceStatus::Suspended => write!(f, "suspended"),
            InstanceStatus::Stopping => write!(f, "stopping"),
            InstanceStatus::Stopped => write!(f, "stopped"),
            InstanceStatus::Failed { error } => write!(f, "failed: {}", error),
        }
    }
}

impl InstanceStatus {
    /// Check if the instance can be paused
    pub fn can_pause(&self) -> bool {
        matches!(self, InstanceStatus::Running)
    }

    /// Check if the instance can be resumed from pause
    pub fn can_resume_from_pause(&self) -> bool {
        matches!(self, InstanceStatus::Paused)
    }

    /// Check if the instance can be suspended
    pub fn can_suspend(&self) -> bool {
        matches!(self, InstanceStatus::Running | InstanceStatus::Paused)
    }

    /// Check if the instance can be woken from suspend
    pub fn can_wake(&self) -> bool {
        matches!(self, InstanceStatus::Suspended)
    }

    /// Check if the instance can be reset
    pub fn can_reset(&self) -> bool {
        matches!(self, InstanceStatus::Running | InstanceStatus::Paused)
    }

    /// Check if the instance can be stopped
    pub fn can_stop(&self) -> bool {
        matches!(
            self,
            InstanceStatus::Running | InstanceStatus::Paused | InstanceStatus::Suspended
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceState {
    pub id: String,
    pub name: String,
    pub status: InstanceStatus,
    pub config: super::InstanceConfig,
    pub vm_pid: Option<i32>,
    pub qmp_socket: Option<PathBuf>,
    pub serial_log: Option<PathBuf>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl InstanceState {
    pub fn new(id: String, name: String, config: super::InstanceConfig) -> Self {
        let now = chrono::Utc::now().timestamp();
        Self {
            id,
            name,
            status: InstanceStatus::Created,
            config,
            vm_pid: None,
            qmp_socket: None,
            serial_log: None,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn update_status(&mut self, status: InstanceStatus) {
        self.status = status;
        self.updated_at = chrono::Utc::now().timestamp();
    }
}
