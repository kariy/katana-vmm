use crate::{qemu::QemuConfig, HypervisorError, Result};
use std::process::{Command, Stdio};
use std::fs;
use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;

pub struct VmManager;

impl VmManager {
    pub fn new() -> Self {
        Self
    }

    /// Launch a QEMU VM with the given configuration
    pub fn launch_vm(&self, config: &QemuConfig) -> Result<i32> {
        // Build QEMU command line
        let args = config.to_qemu_args();

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

        let pid = self.read_pid_file(&config.pid_file)?;

        tracing::info!("QEMU VM launched with PID: {}", pid);

        Ok(pid)
    }

    /// Stop a VM gracefully via signal
    pub fn stop_vm(&self, pid: i32, timeout_secs: u64) -> Result<()> {
        tracing::info!("Stopping VM with PID: {}", pid);

        // Send SIGTERM for graceful shutdown
        kill(Pid::from_raw(pid), Signal::SIGTERM)
            .map_err(|e| HypervisorError::QemuFailed(format!("Failed to send SIGTERM: {}", e)))?;

        // Wait for process to exit
        let start = std::time::Instant::now();
        while start.elapsed().as_secs() < timeout_secs {
            if !self.is_process_running(pid) {
                tracing::info!("VM stopped gracefully");
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }

        // If still running, force kill
        tracing::warn!("VM did not stop gracefully, sending SIGKILL");
        self.kill_vm(pid)?;

        Ok(())
    }

    /// Force kill a VM
    pub fn kill_vm(&self, pid: i32) -> Result<()> {
        tracing::info!("Force killing VM with PID: {}", pid);

        kill(Pid::from_raw(pid), Signal::SIGKILL)
            .map_err(|e| HypervisorError::QemuFailed(format!("Failed to send SIGKILL: {}", e)))?;

        // Wait a bit to ensure process is dead
        std::thread::sleep(std::time::Duration::from_millis(200));

        Ok(())
    }

    /// Check if a process is running
    pub fn is_process_running(&self, pid: i32) -> bool {
        // Try to send signal 0 (does not actually send a signal, just checks if process exists)
        kill(Pid::from_raw(pid), None).is_ok()
    }

    /// Read PID from PID file
    fn read_pid_file(&self, pid_file: &std::path::Path) -> Result<i32> {
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

    /// Pause VM execution by connecting to QMP and issuing a stop command.
    ///
    /// High-level wrapper that handles QMP connection and invokes the underlying
    /// pause operation. This provides a simplified interface for pausing VMs.
    ///
    /// # Parameters
    /// - `qmp_socket`: Path to the VM's QEMU Machine Protocol Unix socket
    ///
    /// # Operation
    /// 1. Establishes QMP client connection to the socket
    /// 2. Calls [`QmpClient::stop()`](crate::qemu::QmpClient::stop) to freeze vCPU execution
    /// 3. Returns after pause command is acknowledged
    ///
    /// For detailed information about resource effects, behavior, and use cases,
    /// see [`QmpClient::stop()`](crate::qemu::QmpClient::stop).
    ///
    /// # Errors
    /// - QMP socket connection failures
    /// - Underlying QMP command errors
    pub fn pause_vm(&self, qmp_socket: &std::path::Path) -> Result<()> {
        tracing::info!("Pausing VM via QMP");

        let mut qmp_client = crate::qemu::QmpClient::new();
        qmp_client.connect(qmp_socket)?;
        qmp_client.stop()?;

        tracing::info!("VM paused successfully");
        Ok(())
    }

    /// Resume VM execution by connecting to QMP and issuing a continue command.
    ///
    /// High-level wrapper that handles QMP connection and invokes the underlying
    /// resume operation. This restores vCPU execution after a pause.
    ///
    /// # Parameters
    /// - `qmp_socket`: Path to the VM's QEMU Machine Protocol Unix socket
    ///
    /// # Operation
    /// 1. Establishes QMP client connection to the socket
    /// 2. Calls [`QmpClient::cont()`](crate::qemu::QmpClient::cont) to resume vCPU execution
    /// 3. Returns after resume command is acknowledged
    ///
    /// For detailed information about resource effects, behavior, and use cases,
    /// see [`QmpClient::cont()`](crate::qemu::QmpClient::cont).
    ///
    /// # Errors
    /// - QMP socket connection failures
    /// - Underlying QMP command errors
    pub fn resume_vm(&self, qmp_socket: &std::path::Path) -> Result<()> {
        tracing::info!("Resuming VM via QMP");

        let mut qmp_client = crate::qemu::QmpClient::new();
        qmp_client.connect(qmp_socket)?;
        qmp_client.cont()?;

        tracing::info!("VM resumed successfully");
        Ok(())
    }

    /// Suspend VM to RAM by connecting to QMP and triggering ACPI S3 sleep.
    ///
    /// High-level wrapper that handles QMP connection and invokes the underlying
    /// suspend operation. Unlike `pause_vm()`, this is a guest-cooperative operation
    /// where the guest OS participates in the suspend sequence.
    ///
    /// # Parameters
    /// - `qmp_socket`: Path to the VM's QEMU Machine Protocol Unix socket
    ///
    /// # Operation
    /// 1. Establishes QMP client connection to the socket
    /// 2. Calls [`QmpClient::system_suspend()`](crate::qemu::QmpClient::system_suspend) to trigger ACPI S3
    /// 3. Returns after suspend command is sent (guest suspends asynchronously)
    ///
    /// For detailed information about ACPI requirements, resource effects, and use cases,
    /// see [`QmpClient::system_suspend()`](crate::qemu::QmpClient::system_suspend).
    ///
    /// # Errors
    /// - QMP socket connection failures
    /// - Underlying QMP command errors (especially if guest lacks ACPI support)
    pub fn suspend_vm(&self, qmp_socket: &std::path::Path) -> Result<()> {
        tracing::info!("Suspending VM via QMP");

        let mut qmp_client = crate::qemu::QmpClient::new();
        qmp_client.connect(qmp_socket)?;
        qmp_client.system_suspend()?;

        tracing::info!("VM suspend command sent");
        Ok(())
    }

    /// Wake VM from suspend by connecting to QMP and triggering ACPI wakeup.
    ///
    /// High-level wrapper that handles QMP connection and invokes the underlying
    /// wakeup operation. This brings a suspended VM back to running state through
    /// the guest's ACPI resume handlers.
    ///
    /// # Parameters
    /// - `qmp_socket`: Path to the VM's QEMU Machine Protocol Unix socket
    ///
    /// # Operation
    /// 1. Establishes QMP client connection to the socket
    /// 2. Calls [`QmpClient::system_wakeup()`](crate::qemu::QmpClient::system_wakeup) to trigger ACPI wake event
    /// 3. Returns after wake command is sent (guest resumes asynchronously)
    ///
    /// For detailed information about ACPI requirements, resource effects, and use cases,
    /// see [`QmpClient::system_wakeup()`](crate::qemu::QmpClient::system_wakeup).
    ///
    /// # Errors
    /// - QMP socket connection failures
    /// - Underlying QMP command errors (especially if VM not in suspended state)
    pub fn wake_vm(&self, qmp_socket: &std::path::Path) -> Result<()> {
        tracing::info!("Waking VM via QMP");

        let mut qmp_client = crate::qemu::QmpClient::new();
        qmp_client.connect(qmp_socket)?;
        qmp_client.system_wakeup()?;

        tracing::info!("VM wakeup command sent");
        Ok(())
    }

    /// Reset VM by connecting to QMP and triggering a hard reboot.
    ///
    /// High-level wrapper that handles QMP connection and invokes the underlying
    /// reset operation. This performs an immediate hardware reset without graceful
    /// shutdown - equivalent to pressing a physical reset button.
    ///
    /// # Parameters
    /// - `qmp_socket`: Path to the VM's QEMU Machine Protocol Unix socket
    ///
    /// # Operation
    /// 1. Establishes QMP client connection to the socket
    /// 2. Calls [`QmpClient::system_reset()`](crate::qemu::QmpClient::system_reset) to trigger hard reset
    /// 3. Returns after reset command is sent (VM reboots immediately)
    ///
    /// For detailed information about risks, resource effects, and use cases,
    /// see [`QmpClient::system_reset()`](crate::qemu::QmpClient::system_reset).
    ///
    /// # Warning
    /// This is a hard reset without graceful shutdown. May cause data loss or corruption.
    ///
    /// # Errors
    /// - QMP socket connection failures
    /// - Underlying QMP command errors
    pub fn reset_vm(&self, qmp_socket: &std::path::Path) -> Result<()> {
        tracing::info!("Resetting VM via QMP");

        let mut qmp_client = crate::qemu::QmpClient::new();
        qmp_client.connect(qmp_socket)?;
        qmp_client.system_reset()?;

        tracing::info!("VM reset command sent");
        Ok(())
    }
}
