use crate::{
    instance::{InstanceState, InstanceStatus},
    qemu::{QemuConfig, Vm},
    state::StateDatabase,
    Result,
};

/// A database-tracked wrapper around `Vm` that automatically updates instance state.
///
/// `ManagedVm` combines low-level QEMU operations with automatic database state tracking.
/// Every lifecycle operation updates the corresponding `InstanceState` in the database.
///
/// # State Transitions
///
/// - **Launch**: `Starting` -> `Running` (with PID)
/// - **Pause**: `Running` -> `Pausing` -> `Paused`
/// - **Resume**: `Paused` -> `Resuming` -> `Running`
/// - **Suspend**: `Running`/`Paused` -> `Suspending` -> `Suspended`
/// - **Wake**: `Suspended` -> `Running`
/// - **Reset**: Stays `Running` (VM reboots)
/// - **Stop**: `Current` -> `Stopping` -> `Stopped` (PID cleared)
/// - **Kill**: `Current` -> `Stopped` (immediate, PID cleared)
///
/// # When to Use
///
/// - ✅ Use in daemon/API code with database
/// - ✅ Use when state persistence is required
/// - ❌ Don't use in unit tests (use raw `Vm`)
/// - ❌ Don't use for one-off scripts without database
pub struct ManagedVm {
    /// The underlying QEMU VM instance
    vm: Vm,

    /// Database instance ID for state tracking
    instance_id: String,

    /// Database connection for persisting state
    db: StateDatabase,
}

impl ManagedVm {
    /// Create a new managed VM instance.
    ///
    /// The VM is not launched automatically.
    pub fn new(instance_id: String, config: QemuConfig, db: StateDatabase) -> Self {
        Self {
            vm: Vm::new(config),
            instance_id,
            db,
        }
    }

    /// Load an existing managed VM from the database.
    ///
    /// Reconstructs a `ManagedVm` from database state, reattaching to a running VM if it has a PID.
    pub fn from_instance(instance_id: &str, db: &StateDatabase) -> Result<Self> {
        let state = db.get_instance_by_id(instance_id)?;
        let config = instance_state_to_qemu_config(&state)?;

        let vm = if let Some(pid) = state.vm_pid {
            Vm::from_running(config, pid)
        } else {
            Vm::new(config)
        };

        Ok(Self {
            vm,
            instance_id: instance_id.to_string(),
            db: db.clone(),
        })
    }

    /// Launch the VM with database state tracking.
    ///
    /// Updates state: `Starting` -> `Running` (stores PID)
    pub fn launch(&mut self) -> Result<()> {
        tracing::info!("ManagedVm: Launching instance {}", self.instance_id);

        // Update status to Starting
        self.update_status(InstanceStatus::Starting)?;

        // Launch the VM
        match self.vm.launch() {
            Ok(()) => {
                // Update status to Running with PID
                let mut state = self.get_state()?;
                state.update_status(InstanceStatus::Running);
                state.vm_pid = self.vm.pid();
                self.db.save_instance(&state)?;

                tracing::info!("ManagedVm: Instance {} launched successfully", self.instance_id);
                Ok(())
            }
            Err(e) => {
                // Mark as failed
                self.mark_failed(&format!("Launch failed: {}", e))?;
                Err(e)
            }
        }
    }

    /// Pause VM execution with database state tracking.
    ///
    /// Updates state: `Running` -> `Pausing` -> `Paused`
    pub fn pause(&self) -> Result<()> {
        tracing::info!("ManagedVm: Pausing instance {}", self.instance_id);

        self.update_status(InstanceStatus::Pausing)?;

        match self.vm.pause() {
            Ok(()) => {
                self.update_status(InstanceStatus::Paused)?;
                tracing::info!("ManagedVm: Instance {} paused successfully", self.instance_id);
                Ok(())
            }
            Err(e) => {
                self.mark_failed(&format!("Pause failed: {}", e))?;
                Err(e)
            }
        }
    }

    /// Resume VM execution with database state tracking.
    ///
    /// Updates state: `Paused` -> `Resuming` -> `Running`
    pub fn resume(&self) -> Result<()> {
        tracing::info!("ManagedVm: Resuming instance {}", self.instance_id);

        self.update_status(InstanceStatus::Resuming)?;

        match self.vm.resume() {
            Ok(()) => {
                self.update_status(InstanceStatus::Running)?;
                tracing::info!("ManagedVm: Instance {} resumed successfully", self.instance_id);
                Ok(())
            }
            Err(e) => {
                self.mark_failed(&format!("Resume failed: {}", e))?;
                Err(e)
            }
        }
    }

    /// Suspend VM to RAM with database state tracking (ACPI S3).
    ///
    /// Updates state: `Current` -> `Suspending` -> `Suspended`
    pub fn suspend(&self) -> Result<()> {
        tracing::info!("ManagedVm: Suspending instance {}", self.instance_id);

        self.update_status(InstanceStatus::Suspending)?;

        match self.vm.suspend() {
            Ok(()) => {
                self.update_status(InstanceStatus::Suspended)?;
                tracing::info!("ManagedVm: Instance {} suspended successfully", self.instance_id);
                Ok(())
            }
            Err(e) => {
                self.mark_failed(&format!("Suspend failed: {}", e))?;
                Err(e)
            }
        }
    }

    /// Wake VM from suspend with database state tracking.
    ///
    /// Updates state: `Suspended` -> `Running`
    pub fn wake(&self) -> Result<()> {
        tracing::info!("ManagedVm: Waking instance {}", self.instance_id);

        match self.vm.wake() {
            Ok(()) => {
                self.update_status(InstanceStatus::Running)?;
                tracing::info!("ManagedVm: Instance {} woken successfully", self.instance_id);
                Ok(())
            }
            Err(e) => {
                self.mark_failed(&format!("Wake failed: {}", e))?;
                Err(e)
            }
        }
    }

    /// Reset VM (hard reboot). State remains `Running`.
    ///
    /// **Warning**: Hard reset without graceful shutdown. May cause data loss.
    pub fn reset(&self) -> Result<()> {
        tracing::info!("ManagedVm: Resetting instance {}", self.instance_id);

        match self.vm.reset() {
            Ok(()) => {
                // VM is still running after reset
                self.update_status(InstanceStatus::Running)?;
                tracing::info!("ManagedVm: Instance {} reset successfully", self.instance_id);
                Ok(())
            }
            Err(e) => {
                self.mark_failed(&format!("Reset failed: {}", e))?;
                Err(e)
            }
        }
    }

    /// Stop VM gracefully with database state tracking.
    ///
    /// Updates state: `Current` -> `Stopping` -> `Stopped` (clears PID)
    pub fn stop(&mut self, timeout_secs: u64) -> Result<()> {
        tracing::info!("ManagedVm: Stopping instance {}", self.instance_id);

        self.update_status(InstanceStatus::Stopping)?;

        match self.vm.stop(timeout_secs) {
            Ok(()) => {
                // Clear PID and update status to Stopped
                let mut state = self.get_state()?;
                state.update_status(InstanceStatus::Stopped);
                state.vm_pid = None;
                self.db.save_instance(&state)?;

                tracing::info!("ManagedVm: Instance {} stopped successfully", self.instance_id);
                Ok(())
            }
            Err(e) => {
                // Even if stop fails, try to clear PID if VM is not running
                if !self.vm.is_running() {
                    let mut state = self.get_state()?;
                    state.vm_pid = None;
                    state.update_status(InstanceStatus::Stopped);
                    let _ = self.db.save_instance(&state);
                } else {
                    self.mark_failed(&format!("Stop failed: {}", e))?;
                }
                Err(e)
            }
        }
    }

    /// Force kill VM immediately. Updates state to `Stopped` and clears PID.
    ///
    /// **Warning**: May cause data loss. Use `stop()` for graceful shutdown.
    pub fn kill(&mut self) -> Result<()> {
        tracing::info!("ManagedVm: Force killing instance {}", self.instance_id);

        match self.vm.kill() {
            Ok(()) => {
                // Clear PID and update status to Stopped
                let mut state = self.get_state()?;
                state.update_status(InstanceStatus::Stopped);
                state.vm_pid = None;
                self.db.save_instance(&state)?;

                tracing::info!("ManagedVm: Instance {} killed successfully", self.instance_id);
                Ok(())
            }
            Err(e) => {
                // Even if kill fails, try to clear PID if VM is not running
                if !self.vm.is_running() {
                    let mut state = self.get_state()?;
                    state.vm_pid = None;
                    state.update_status(InstanceStatus::Stopped);
                    let _ = self.db.save_instance(&state);
                }
                Err(e)
            }
        }
    }

    /// Check if the VM is currently running.
    ///
    /// This checks the actual QEMU process, not the database state.
    pub fn is_running(&self) -> bool {
        self.vm.is_running()
    }

    /// Get the process ID of the running VM.
    pub fn pid(&self) -> Option<i32> {
        self.vm.pid()
    }

    /// Get the instance ID.
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    /// Get a reference to the underlying `Vm`.
    ///
    /// Use this to access low-level VM details or configuration.
    pub fn vm(&self) -> &Vm {
        &self.vm
    }

    /// Get a mutable reference to the underlying `Vm`.
    ///
    /// **Warning**: Direct operations bypass database state tracking.
    pub fn vm_mut(&mut self) -> &mut Vm {
        &mut self.vm
    }

    /// Get the current instance state from the database (fresh fetch).
    pub fn get_state(&self) -> Result<InstanceState> {
        self.db.get_instance_by_id(&self.instance_id)
    }

    /// Update the instance status in the database.
    fn update_status(&self, status: InstanceStatus) -> Result<()> {
        let mut state = self.get_state()?;
        state.update_status(status);
        self.db.save_instance(&state)?;
        Ok(())
    }

    /// Mark the instance as failed with an error message.
    fn mark_failed(&self, error: &str) -> Result<()> {
        let mut state = self.get_state()?;
        state.update_status(InstanceStatus::Failed {
            error: error.to_string(),
        });
        self.db.save_instance(&state)?;
        Ok(())
    }
}

/// Convert an InstanceState to QemuConfig.
///
/// This helper function builds a QEMU configuration from database instance state.
/// It constructs paths for QMP socket, serial log, and PID file based on the instance's data directory.
///
/// # Errors
/// - If SEV-SNP configuration is invalid
/// - If required paths are missing
fn instance_state_to_qemu_config(state: &InstanceState) -> Result<QemuConfig> {
    let config = &state.config;

    // Build paths in data directory
    let qmp_socket = config.data_dir.join("qmp.sock");
    let serial_log = config.data_dir.join("serial.log");
    let pid_file = config.data_dir.join("qemu.pid");

    // Build SEV-SNP config if in TEE mode
    let sev_snp = if config.tee_mode {
        Some(crate::qemu::config::SevSnpConfig {
            cbitpos: 51,           // Standard for AMD SEV
            reduced_phys_bits: 1,   // Standard for AMD SEV
            vcpu_type: config.vcpu_type.clone(),
        })
    } else {
        None
    };

    // Build kernel command line with Katana arguments
    let katana_args = config.build_katana_args();
    let kernel_cmdline = QemuConfig::build_kernel_cmdline(&katana_args);

    Ok(QemuConfig {
        memory_mb: config.memory_mb,
        vcpus: config.vcpus,
        cpu_type: config.vcpu_type.clone(),
        kernel_path: config.kernel_path.clone(),
        initrd_path: config.initrd_path.clone(),
        bios_path: config.ovmf_path.clone(),
        kernel_cmdline,
        rpc_port: config.rpc_port,
        disk_image: config.disk_image.clone(),
        qmp_socket,
        serial_log,
        pid_file,
        sev_snp,
        enable_kvm: true, // Always enable KVM for production
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance::InstanceConfig;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn create_test_db() -> (StateDatabase, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = StateDatabase::new(&db_path).unwrap();
        (db, temp_dir)
    }

    fn create_test_instance(name: &str, data_dir: PathBuf) -> InstanceState {
        let config = InstanceConfig {
            vcpus: 2,
            memory_mb: 2048,
            storage_bytes: 5 * 1024 * 1024 * 1024,
            rpc_port: 5050,
            metrics_port: None,
            tee_mode: false,
            vcpu_type: "host".to_string(),
            expected_measurement: None,
            kernel_path: data_dir.join("vmlinuz"),
            initrd_path: data_dir.join("initrd.img"),
            ovmf_path: None,
            data_dir,
            disk_image: None,
            chain_id: None,
            dev_mode: true,
            block_time: None,
            accounts: Some(10),
            disable_fee: true,
            extra_args: vec![],
        };

        let id = format!("test-id-{}", name);
        InstanceState::new(id, name.to_string(), config)
    }

    #[test]
    fn test_instance_state_to_qemu_config() {
        let temp_dir = TempDir::new().unwrap();
        let instance = create_test_instance("test1", temp_dir.path().to_path_buf());

        let qemu_config = instance_state_to_qemu_config(&instance).unwrap();

        assert_eq!(qemu_config.vcpus, 2);
        assert_eq!(qemu_config.memory_mb, 2048);
        assert_eq!(qemu_config.cpu_type, "host");
        assert_eq!(qemu_config.rpc_port, 5050);
        assert!(qemu_config.enable_kvm);
        assert!(qemu_config.sev_snp.is_none());
    }

    #[test]
    fn test_instance_state_to_qemu_config_with_tee() {
        let temp_dir = TempDir::new().unwrap();
        let mut instance = create_test_instance("test1", temp_dir.path().to_path_buf());
        instance.config.tee_mode = true;
        instance.config.vcpu_type = "EPYC-v4".to_string();

        let qemu_config = instance_state_to_qemu_config(&instance).unwrap();

        assert!(qemu_config.sev_snp.is_some());
        let sev_config = qemu_config.sev_snp.unwrap();
        assert_eq!(sev_config.vcpu_type, "EPYC-v4");
        assert_eq!(sev_config.cbitpos, 51);
        assert_eq!(sev_config.reduced_phys_bits, 1);
    }

    #[test]
    fn test_managed_vm_new() {
        let (db, temp_dir) = create_test_db();
        let instance = create_test_instance("test1", temp_dir.path().to_path_buf());
        db.save_instance(&instance).unwrap();

        let qemu_config = instance_state_to_qemu_config(&instance).unwrap();
        let managed_vm = ManagedVm::new(instance.id.clone(), qemu_config, db);

        assert_eq!(managed_vm.instance_id(), instance.id);
        assert!(!managed_vm.is_running());
        assert!(managed_vm.pid().is_none());
    }

    #[test]
    fn test_managed_vm_from_instance() {
        let (db, temp_dir) = create_test_db();
        let instance = create_test_instance("test1", temp_dir.path().to_path_buf());
        db.save_instance(&instance).unwrap();

        let managed_vm = ManagedVm::from_instance(&instance.id, &db).unwrap();

        assert_eq!(managed_vm.instance_id(), instance.id);
        assert!(!managed_vm.is_running());
    }

    #[test]
    fn test_managed_vm_from_instance_not_found() {
        let (db, _temp_dir) = create_test_db();

        let result = ManagedVm::from_instance("nonexistent", &db);
        assert!(result.is_err());
    }

    #[test]
    fn test_get_state() {
        let (db, temp_dir) = create_test_db();
        let instance = create_test_instance("test1", temp_dir.path().to_path_buf());
        db.save_instance(&instance).unwrap();

        let qemu_config = instance_state_to_qemu_config(&instance).unwrap();
        let managed_vm = ManagedVm::new(instance.id.clone(), qemu_config, db);

        let state = managed_vm.get_state().unwrap();
        assert_eq!(state.name, "test1");
        assert!(matches!(state.status, InstanceStatus::Created));
    }

    #[test]
    fn test_update_status() {
        let (db, temp_dir) = create_test_db();
        let instance = create_test_instance("test1", temp_dir.path().to_path_buf());
        db.save_instance(&instance).unwrap();

        let qemu_config = instance_state_to_qemu_config(&instance).unwrap();
        let managed_vm = ManagedVm::new(instance.id.clone(), qemu_config, db.clone());

        managed_vm.update_status(InstanceStatus::Starting).unwrap();

        let state = db.get_instance_by_id(&instance.id).unwrap();
        assert!(matches!(state.status, InstanceStatus::Starting));
    }

    #[test]
    fn test_mark_failed() {
        let (db, temp_dir) = create_test_db();
        let instance = create_test_instance("test1", temp_dir.path().to_path_buf());
        db.save_instance(&instance).unwrap();

        let qemu_config = instance_state_to_qemu_config(&instance).unwrap();
        let managed_vm = ManagedVm::new(instance.id.clone(), qemu_config, db.clone());

        managed_vm.mark_failed("test error").unwrap();

        let state = db.get_instance_by_id(&instance.id).unwrap();
        match state.status {
            InstanceStatus::Failed { error } => {
                assert_eq!(error, "test error");
            }
            _ => panic!("Expected Failed status"),
        }
    }
}
