use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceConfig {
    // Resource limits
    pub vcpus: u32,
    pub memory_mb: u64,
    pub storage_bytes: u64,
    pub quota_project_id: Option<u32>,

    // Network
    pub rpc_port: u16,
    pub metrics_port: Option<u16>,

    // TEE configuration
    pub tee_mode: bool,
    pub vcpu_type: String,
    pub expected_measurement: Option<String>,

    // Boot components
    pub kernel_path: PathBuf,
    pub initrd_path: PathBuf,
    pub ovmf_path: Option<PathBuf>,

    // Storage
    pub data_dir: PathBuf,
    pub disk_image: Option<PathBuf>,

    // Katana-specific configuration
    pub chain_id: Option<String>,
    pub dev_mode: bool,
    pub block_time: Option<u64>,
    pub accounts: Option<u16>,
    pub disable_fee: bool,
    pub extra_args: Vec<String>,
}

impl Default for InstanceConfig {
    fn default() -> Self {
        Self {
            vcpus: 4,
            memory_mb: 4096,
            storage_bytes: 10 * 1024 * 1024 * 1024, // 10GB
            quota_project_id: None,
            rpc_port: 5050,
            metrics_port: None,
            tee_mode: false,
            vcpu_type: "host".to_string(),
            expected_measurement: None,
            kernel_path: PathBuf::new(),
            initrd_path: PathBuf::new(),
            ovmf_path: None,
            data_dir: PathBuf::new(),
            disk_image: None,
            chain_id: None,
            dev_mode: false,
            block_time: None,
            accounts: Some(10),
            disable_fee: false,
            extra_args: vec![],
        }
    }
}

impl InstanceConfig {
    pub fn build_katana_args(&self) -> Vec<String> {
        let mut args = vec![
            "--http.addr=0.0.0.0".to_string(),
            "--http.port=5050".to_string(),
        ];

        if let Some(chain_id) = &self.chain_id {
            args.push(format!("--chain-id={}", chain_id));
        }

        if self.dev_mode {
            args.push("--dev".to_string());
        }

        if let Some(block_time) = self.block_time {
            args.push(format!("--block-time={}", block_time));
        }

        if let Some(accounts) = self.accounts {
            args.push(format!("--accounts={}", accounts));
        }

        if self.disable_fee {
            args.push("--disable-fee".to_string());
        }

        args.extend(self.extra_args.clone());
        args
    }
}
