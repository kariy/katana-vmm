use crate::{qemu::QemuConfig, HypervisorError, Result};
use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use std::fs;
use std::process::{Command, Stdio};

/// Represents a single QEMU VM instance with its configuration and state.
///
/// Provides instance methods for lifecycle operations. Unlike `VmManager` (stateless),
/// `Vm` maintains the state of a specific VM instance.
///
/// # Drop Behavior
///
/// **The `Vm` struct does NOT automatically stop the VM when dropped.**
///
/// QEMU processes run independently in daemon mode and continue running after the
/// `Vm` instance is dropped. **Always explicitly call `stop()` or `kill()`** before dropping.
///
/// ```no_run
/// # use katana_core::qemu::{QemuConfig, Vm};
/// # fn example(config: QemuConfig) -> katana_core::Result<()> {
/// let mut vm = Vm::new(config);
/// vm.launch()?;
/// // ... operations ...
/// vm.stop(10)?;  // Required! Otherwise VM orphaned
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct Vm {
    /// Configuration for this VM instance
    config: QemuConfig,

    /// Process ID of the running QEMU process, None if not launched
    pid: Option<i32>,
}

impl Vm {
    /// Create a new VM instance. Not launched automatically.
    pub fn new(config: QemuConfig) -> Self {
        Self { config, pid: None }
    }

    /// Attach to an already running QEMU process by PID.
    ///
    /// Verifies that:
    /// 1. The PID exists and is accessible
    /// 2. The process is a QEMU instance
    /// 3. The QMP socket path matches this VM's configuration
    /// 4. The QMP socket is responsive
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use katana_core::qemu::{QemuConfig, Vm};
    /// # fn example(config: QemuConfig) -> katana_core::Result<()> {
    /// let pid_str = std::fs::read_to_string("/tmp/qemu.pid")?;
    /// let pid: i32 = pid_str.trim().parse()
    ///     .map_err(|e| katana_core::HypervisorError::QemuFailed(format!("Invalid PID: {}", e)))?;
    ///
    /// let mut vm = Vm::new(config);
    /// vm.attach(pid)?;  // Verifies it's the right QEMU instance
    /// vm.stop(10)?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - VM is already launched or attached
    /// - PID doesn't exist or isn't accessible
    /// - PID is not a QEMU process
    /// - QMP socket doesn't match config (wrong VM instance)
    /// - QMP socket is not responsive
    pub fn attach(&mut self, pid: i32) -> Result<()> {
        if self.pid.is_some() {
            return Err(HypervisorError::QemuFailed(
                "VM is already running or attached".to_string(),
            ));
        }

        tracing::info!("Attaching to QEMU process with PID: {}", pid);

        // Verify it's the expected QEMU instance
        self.verify_qemu_process(pid)?;

        // Verify QMP connectivity
        self.verify_qmp_connectivity()?;

        self.pid = Some(pid);

        tracing::info!("Successfully attached to VM with PID: {}", pid);
        Ok(())
    }

    /// Launch the VM. Spawns QEMU in daemon mode and stores PID.
    pub fn launch(&mut self) -> Result<()> {
        if self.pid.is_some() {
            return Err(HypervisorError::QemuFailed(
                "VM is already running".to_string(),
            ));
        }

        // Build QEMU command line
        let args = self.config.to_qemu_args();

        tracing::info!("Launching QEMU VM with command: {:?}", args);

        // Execute QEMU
        let output = Command::new(&args[0])
            .args(&args[1..])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?
            .wait_with_output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(HypervisorError::QemuFailed(format!(
                "QEMU launch failed: {}",
                stderr
            )));
        }

        // Read PID from PID file
        // Wait a bit for QEMU to write the PID file
        std::thread::sleep(std::time::Duration::from_millis(500));

        let pid = self.read_pid_file()?;
        self.pid = Some(pid);

        tracing::info!("QEMU VM launched with PID: {}", pid);

        Ok(())
    }

    /// Stop VM gracefully via SIGTERM. Force kills with SIGKILL after timeout.
    pub fn stop(&mut self, timeout_secs: u64) -> Result<()> {
        let pid = self.require_pid()?;

        tracing::info!("Stopping VM with PID: {}", pid);

        // Send SIGTERM for graceful shutdown
        kill(Pid::from_raw(pid), Signal::SIGTERM)
            .map_err(|e| HypervisorError::QemuFailed(format!("Failed to send SIGTERM: {}", e)))?;

        // Wait for process to exit
        let start = std::time::Instant::now();
        while start.elapsed().as_secs() < timeout_secs {
            if !self.is_running() {
                tracing::info!("VM stopped gracefully");
                self.pid = None;
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }

        // If still running, force kill
        tracing::warn!("VM did not stop gracefully, sending SIGKILL");
        self.kill()?;

        Ok(())
    }

    /// Force kill VM with SIGKILL (immediate termination).
    ///
    /// **Warning**: May cause data loss. Use `stop()` for graceful shutdown.
    pub fn kill(&mut self) -> Result<()> {
        let pid = self.require_pid()?;

        tracing::info!("Force killing VM with PID: {}", pid);

        kill(Pid::from_raw(pid), Signal::SIGKILL)
            .map_err(|e| HypervisorError::QemuFailed(format!("Failed to send SIGKILL: {}", e)))?;

        // Wait a bit to ensure process is dead
        std::thread::sleep(std::time::Duration::from_millis(200));

        self.pid = None;

        Ok(())
    }

    /// Check if VM process is currently running.
    pub fn is_running(&self) -> bool {
        match self.pid {
            Some(pid) => kill(Pid::from_raw(pid), None).is_ok(),
            None => false,
        }
    }

    /// Pause VM execution (freeze vCPUs via QMP).
    pub fn pause(&self) -> Result<()> {
        self.require_pid()?;

        tracing::info!("Pausing VM via QMP");

        let mut qmp_client = crate::qemu::QmpClient::new();
        qmp_client.connect(&self.config.qmp_socket)?;
        qmp_client.stop()?;

        tracing::info!("VM paused successfully");
        Ok(())
    }

    /// Resume VM execution (unfreeze vCPUs via QMP).
    pub fn resume(&self) -> Result<()> {
        self.require_pid()?;

        tracing::info!("Resuming VM via QMP");

        let mut qmp_client = crate::qemu::QmpClient::new();
        qmp_client.connect(&self.config.qmp_socket)?;
        qmp_client.cont()?;

        tracing::info!("VM resumed successfully");
        Ok(())
    }

    /// Suspend VM to RAM (ACPI S3 via QMP). Requires guest OS cooperation.
    pub fn suspend(&self) -> Result<()> {
        self.require_pid()?;

        tracing::info!("Suspending VM via QMP");

        let mut qmp_client = crate::qemu::QmpClient::new();
        qmp_client.connect(&self.config.qmp_socket)?;
        qmp_client.system_suspend()?;

        tracing::info!("VM suspend command sent");
        Ok(())
    }

    /// Wake VM from suspend (ACPI wakeup via QMP).
    pub fn wake(&self) -> Result<()> {
        self.require_pid()?;

        tracing::info!("Waking VM via QMP");

        let mut qmp_client = crate::qemu::QmpClient::new();
        qmp_client.connect(&self.config.qmp_socket)?;
        qmp_client.system_wakeup()?;

        tracing::info!("VM wakeup command sent");
        Ok(())
    }

    /// Reset VM (hard reboot via QMP).
    ///
    /// **Warning**: Hard reset without graceful shutdown. May cause data loss.
    pub fn reset(&self) -> Result<()> {
        self.require_pid()?;

        tracing::info!("Resetting VM via QMP");

        let mut qmp_client = crate::qemu::QmpClient::new();
        qmp_client.connect(&self.config.qmp_socket)?;
        qmp_client.system_reset()?;

        tracing::info!("VM reset command sent");
        Ok(())
    }

    /// Get the process ID (None if not launched or stopped).
    pub fn pid(&self) -> Option<i32> {
        self.pid
    }

    /// Get a reference to this VM's configuration.
    pub fn config(&self) -> &QemuConfig {
        &self.config
    }

    /// Get the QMP socket path for this VM.
    pub fn qmp_socket(&self) -> &std::path::Path {
        &self.config.qmp_socket
    }

    /// Get the PID file path for this VM.
    pub fn pid_file(&self) -> &std::path::Path {
        &self.config.pid_file
    }

    /// Get the serial log path for this VM.
    pub fn serial_log(&self) -> &std::path::Path {
        &self.config.serial_log
    }

    /// Helper to require a PID, returning an error if not launched.
    fn require_pid(&self) -> Result<i32> {
        self.pid
            .ok_or_else(|| HypervisorError::QemuFailed("VM is not running".to_string()))
    }

    /// Read PID from the VM's PID file.
    fn read_pid_file(&self) -> Result<i32> {
        let pid_file = &self.config.pid_file;

        if !pid_file.exists() {
            return Err(HypervisorError::QemuFailed(
                "PID file not found".to_string(),
            ));
        }

        let pid_str = fs::read_to_string(pid_file)?;
        let pid: i32 = pid_str
            .trim()
            .parse()
            .map_err(|e| HypervisorError::QemuFailed(format!("Invalid PID in file: {}", e)))?;

        Ok(pid)
    }

    /// Verify that a PID belongs to the expected QEMU instance.
    ///
    /// Checks:
    /// 1. Process exists
    /// 2. Process is a QEMU process
    /// 3. QMP socket path matches config
    fn verify_qemu_process(&self, pid: i32) -> Result<()> {
        // Check process exists
        kill(Pid::from_raw(pid), None).map_err(|_| {
            HypervisorError::QemuFailed(format!("Process {} not found or not accessible", pid))
        })?;

        // Read command line from /proc
        let cmdline_path = format!("/proc/{}/cmdline", pid);
        let cmdline = fs::read_to_string(&cmdline_path).map_err(|e| {
            HypervisorError::QemuFailed(format!("Failed to read {}: {}", cmdline_path, e))
        })?;

        // Parse cmdline (args are null-separated)
        let args: Vec<&str> = cmdline.split('\0').filter(|s| !s.is_empty()).collect();

        if args.is_empty() {
            return Err(HypervisorError::QemuFailed(
                "Failed to parse process command line".to_string(),
            ));
        }

        // Check it's a QEMU process
        let exe = args[0];
        if !exe.contains("qemu-system") {
            return Err(HypervisorError::QemuFailed(format!(
                "PID {} is not a QEMU process (executable: {})",
                pid, exe
            )));
        }

        // Verify QMP socket matches (most reliable unique identifier)
        let qmp_socket_str = self.config.qmp_socket.to_string_lossy();
        let qmp_pattern = format!("unix:{}", qmp_socket_str);

        let cmdline_str = args.join(" ");
        if !cmdline_str.contains(&qmp_pattern) {
            return Err(HypervisorError::QemuFailed(format!(
                "PID {} does not match expected QMP socket: {}",
                pid, qmp_socket_str
            )));
        }

        tracing::debug!("Verified PID {} is the expected QEMU instance", pid);
        Ok(())
    }

    /// Verify QMP socket connectivity to the VM.
    fn verify_qmp_connectivity(&self) -> Result<()> {
        let mut qmp_client = crate::qemu::QmpClient::new();
        qmp_client.connect(&self.config.qmp_socket).map_err(|e| {
            HypervisorError::QemuFailed(format!(
                "Failed to connect to QMP socket {}: {}",
                self.config.qmp_socket.display(),
                e
            ))
        })?;

        tracing::debug!("QMP socket connectivity verified");
        Ok(())
    }
}

impl Drop for Vm {
    /// Cleanup when the Vm instance is dropped.
    ///
    /// Note: This does NOT automatically stop the VM, as the QEMU process runs
    /// independently in daemon mode. Use `stop()` or `kill()` explicitly to
    /// terminate the VM before dropping.
    fn drop(&mut self) {
        // We intentionally don't stop the VM here, as it may be running independently
        // and the user may want to keep it running. Explicit cleanup is preferred.
        if self.pid.is_some() {
            tracing::debug!(
                "Vm instance dropped while VM (PID: {:?}) is still running. \
                VM will continue running in background.",
                self.pid
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn create_test_config() -> QemuConfig {
        QemuConfig {
            memory_mb: 2048,
            vcpus: 2,
            cpu_type: "host".to_string(),
            kernel_path: PathBuf::from("/test/vmlinuz"),
            initrd_path: PathBuf::from("/test/initrd.img"),
            bios_path: None,
            kernel_cmdline: "console=ttyS0".to_string(),
            rpc_port: 5050,
            disk_image: None,
            qmp_socket: PathBuf::from("/tmp/qmp.sock"),
            serial_log: PathBuf::from("/tmp/serial.log"),
            pid_file: PathBuf::from("/tmp/qemu.pid"),
            sev_snp: None,
            enable_kvm: true,
        }
    }

    #[test]
    fn test_new_vm() {
        let config = create_test_config();
        let vm = Vm::new(config);

        assert!(vm.pid().is_none());
        assert!(!vm.is_running());
    }

    #[test]
    fn test_attach_fails_without_process() {
        let config = create_test_config();
        let mut vm = Vm::new(config);

        // Attaching to a non-existent PID should fail
        let result = vm.attach(99999);
        assert!(result.is_err());
        assert!(vm.pid().is_none());
    }

    #[test]
    fn test_attach_fails_when_already_running() {
        let config = create_test_config();
        let mut vm = Vm::new(config);

        // Manually set PID to simulate already running
        vm.pid = Some(12345);

        // Trying to attach when already running should fail
        let result = vm.attach(67890);
        assert!(result.is_err());
        assert_eq!(vm.pid(), Some(12345)); // PID unchanged
    }

    #[test]
    fn test_accessors() {
        let config = create_test_config();
        let vm = Vm::new(config);

        assert_eq!(vm.config().memory_mb, 2048);
        assert_eq!(vm.config().vcpus, 2);
        assert_eq!(vm.qmp_socket(), std::path::Path::new("/tmp/qmp.sock"));
        assert_eq!(vm.pid_file(), std::path::Path::new("/tmp/qemu.pid"));
        assert_eq!(vm.serial_log(), std::path::Path::new("/tmp/serial.log"));
    }

    #[test]
    fn test_require_pid_fails_when_not_running() {
        let config = create_test_config();
        let vm = Vm::new(config);

        let result = vm.require_pid();
        assert!(result.is_err());
    }
}
