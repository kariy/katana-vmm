use thiserror::Error;

#[derive(Error, Debug)]
pub enum HypervisorError {
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("QMP error: {0}")]
    Qmp(String),

    #[error("Instance not found: {0}")]
    InstanceNotFound(String),

    #[error("Instance already exists: {0}")]
    InstanceAlreadyExists(String),

    #[error("Invalid state transition: from {from} to {to}")]
    InvalidStateTransition { from: String, to: String },

    #[error("Port unavailable: {0}")]
    PortUnavailable(u16),

    #[error("No ports available in range")]
    NoPortsAvailable,

    #[error("VM process not found: {0}")]
    VmProcessNotFound(i32),

    #[error("QEMU execution failed: {0}")]
    QemuFailed(String),

    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("Storage quota exceeded: {used}/{limit} bytes")]
    StorageQuotaExceeded { used: u64, limit: u64 },

    #[error("Quota operation failed: {0}")]
    QuotaOperationFailed(String),

    #[error("Filesystem does not support quotas: {0}")]
    QuotaNotSupported(String),

    #[error("Insufficient permissions for quota operations: {0}")]
    QuotaPermissionDenied(String),

    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Attestation verification failed: {0}")]
    AttestationFailed(String),

    #[error("Measurement mismatch: expected {expected}, got {actual}")]
    MeasurementMismatch { expected: String, actual: String },
}

pub type Result<T> = std::result::Result<T, HypervisorError>;
