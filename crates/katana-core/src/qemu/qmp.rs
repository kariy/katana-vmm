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

    /// Initiate graceful shutdown of the VM via ACPI power button event.
    ///
    /// This command sends an ACPI power button press event to the guest operating system,
    /// which should trigger a graceful shutdown sequence if the guest supports ACPI.
    ///
    /// # Resource Effects
    /// - **CPU**: Remains allocated until guest completes shutdown
    /// - **Memory**: Remains allocated until guest completes shutdown
    /// - **Disk**: Guest can flush buffers and unmount filesystems cleanly
    /// - **Network**: Guest can close connections gracefully
    /// - **QEMU Process**: Continues running until guest completes shutdown
    ///
    /// # Behavior
    /// - Requires guest OS with ACPI support
    /// - Guest initiates shutdown sequence (systemd, init scripts, etc.)
    /// - Allows guest to perform cleanup operations
    /// - Non-blocking: returns immediately, shutdown happens asynchronously
    /// - If guest doesn't support ACPI or hangs, may need SIGTERM/SIGKILL
    ///
    /// # Use Cases
    /// - Graceful VM shutdown with proper cleanup
    /// - Scheduled maintenance with data integrity
    /// - Allowing services to terminate properly
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

    /// Immediately terminate the QEMU process.
    ///
    /// This command forcibly exits QEMU without any guest involvement or cleanup.
    /// The guest OS does not receive any notification and cannot perform shutdown procedures.
    ///
    /// # Resource Effects
    /// - **CPU**: Immediately released when QEMU exits
    /// - **Memory**: Immediately released, VM state lost
    /// - **Disk**: Dirty buffers NOT flushed, potential data corruption
    /// - **Network**: Connections abruptly closed without proper shutdown
    /// - **QEMU Process**: Terminates immediately
    ///
    /// # Behavior
    /// - No guest involvement - QEMU exits directly
    /// - Equivalent to "kill -9" from the hypervisor perspective
    /// - VM state is completely lost
    /// - Disk writes may be incomplete
    /// - Risk of filesystem corruption in guest
    ///
    /// # Use Cases
    /// - Emergency shutdown when VM is unresponsive
    /// - Testing crash recovery mechanisms
    /// - When graceful shutdown has already failed
    ///
    /// # Warning
    /// Use with caution - may cause data loss or corruption
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

    /// Pause (freeze) VM execution immediately at the hypervisor level.
    ///
    /// This command halts all vCPU execution without any guest involvement.
    /// The VM is frozen in place with all state preserved in memory.
    /// This is a hypervisor-level operation - the guest is not aware it was paused.
    ///
    /// # Resource Effects
    /// - **CPU**: vCPUs immediately stop executing, host CPU freed
    /// - **Memory**: Fully allocated and preserved, VM state frozen in RAM
    /// - **Disk**: No active I/O, pending operations remain queued
    /// - **Network**: Connections remain open but no packets processed
    /// - **QEMU Process**: Continues running but vCPUs idle
    ///
    /// # Behavior
    /// - Instant: takes effect immediately (microseconds)
    /// - No guest involvement - hypervisor-level freeze
    /// - VM state completely preserved in memory
    /// - Timers and clocks frozen from guest perspective
    /// - Can be resumed with `cont()` command
    /// - Network connections may timeout if paused too long
    ///
    /// # Use Cases
    /// - Quick debugging/inspection without shutdown
    /// - Temporary halt during live migration setup
    /// - Taking consistent snapshots
    /// - Pausing before attaching debugger
    /// - Testing time-sensitive code behavior
    ///
    /// # Notes
    /// - Guest clock stops, may cause time drift when resumed
    /// - External services may timeout during pause
    /// - Not suitable for long-term suspension (use `system_suspend` instead)
    pub fn stop(&mut self) -> Result<()> {
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| HypervisorError::QemuFailed("Not connected to QMP".to_string()))?;

        self.runtime.block_on(async {
            client
                .execute::<(), ()>("stop", None)
                .await
                .map_err(|e| {
                    HypervisorError::QemuFailed(format!("QMP stop failed: {}", e))
                })
        })?;

        Ok(())
    }

    /// Resume (unpause) VM execution after a pause operation.
    ///
    /// This command resumes vCPU execution from a paused state. The VM continues
    /// execution from the exact point where it was paused, as if no time had passed.
    ///
    /// # Resource Effects
    /// - **CPU**: vCPUs resume execution, consuming host CPU cycles
    /// - **Memory**: VM state remains intact, execution continues from frozen point
    /// - **Disk**: Pending I/O operations resume processing
    /// - **Network**: Packet processing resumes (connections may have timed out)
    /// - **QEMU Process**: vCPUs become active again
    ///
    /// # Behavior
    /// - Instant: vCPUs resume immediately
    /// - VM continues from exact instruction where paused
    /// - Guest clock resumes (may show time gap depending on guest clock source)
    /// - No guest awareness that pause occurred
    /// - Pending interrupts and I/O operations continue
    ///
    /// # Use Cases
    /// - Resuming after debugging/inspection
    /// - Continuing after snapshot operation
    /// - Unpausing after brief halt
    /// - Resuming after live migration setup
    ///
    /// # Notes
    /// - Only works if VM was previously paused with `stop()`
    /// - Guest may detect time drift if using wall clock
    /// - Network connections may have been closed by peers during pause
    /// - Complements `stop()` for pause/resume cycles
    pub fn cont(&mut self) -> Result<()> {
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| HypervisorError::QemuFailed("Not connected to QMP".to_string()))?;

        self.runtime.block_on(async {
            client
                .execute::<(), ()>("cont", None)
                .await
                .map_err(|e| {
                    HypervisorError::QemuFailed(format!("QMP cont failed: {}", e))
                })
        })?;

        Ok(())
    }

    /// Suspend VM to RAM (S3 sleep state) via guest ACPI.
    ///
    /// This command triggers a guest-initiated suspend-to-RAM operation through ACPI.
    /// The guest OS performs its suspend procedures (save state, stop services) before
    /// entering sleep mode. This is a cooperative operation requiring guest ACPI support.
    ///
    /// # Resource Effects
    /// - **CPU**: vCPUs enter sleep state, minimal host CPU usage
    /// - **Memory**: Fully allocated, VM state preserved in RAM (not released)
    /// - **Disk**: Guest flushes buffers before suspend, no I/O in suspended state
    /// - **Network**: Guest may close connections or enter low-power state
    /// - **QEMU Process**: Continues running but VM in suspended state
    ///
    /// # Behavior
    /// - Cooperative: guest OS performs suspend sequence
    /// - Guest saves state and enters ACPI S3 (suspend-to-RAM) state
    /// - Requires guest kernel with ACPI support (CONFIG_ACPI_SLEEP)
    /// - Guest userspace processes frozen, kernel in minimal state
    /// - Can be woken with `system_wakeup()` command
    /// - Returns immediately but suspend happens asynchronously
    ///
    /// # Use Cases
    /// - Long-term suspension while preserving VM state
    /// - Power saving when VM not actively needed
    /// - Testing suspend/resume functionality
    /// - Simulating laptop suspend scenarios
    ///
    /// # Requirements
    /// - Guest OS must support ACPI suspend (Linux: CONFIG_ACPI_SLEEP)
    /// - QEMU must be configured with ACPI support
    /// - Guest must have ACPI drivers loaded
    ///
    /// # Notes
    /// - Different from `stop()` - guest is aware and participates
    /// - Memory remains allocated on host (not swapped to disk)
    /// - Network connections typically closed by guest before suspend
    /// - May take several seconds for guest to complete suspend sequence
    /// - Fails if guest doesn't support ACPI suspend
    pub fn system_suspend(&mut self) -> Result<()> {
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| HypervisorError::QemuFailed("Not connected to QMP".to_string()))?;

        self.runtime.block_on(async {
            client
                .execute::<(), ()>("system_suspend", None)
                .await
                .map_err(|e| {
                    HypervisorError::QemuFailed(format!("QMP system_suspend failed: {}", e))
                })
        })?;

        Ok(())
    }

    /// Wake VM from ACPI suspend state (resume from S3).
    ///
    /// This command wakes a suspended VM by simulating a wakeup event (like pressing
    /// a power button). The guest OS performs its resume sequence, restoring state
    /// and reactivating devices.
    ///
    /// # Resource Effects
    /// - **CPU**: vCPUs resume execution, full CPU usage restored
    /// - **Memory**: VM state restored from RAM, full memory access resumed
    /// - **Disk**: Guest reinitializes disk I/O, resumes file operations
    /// - **Network**: Guest may need to reestablish connections
    /// - **QEMU Process**: VM becomes fully active again
    ///
    /// # Behavior
    /// - Triggers guest resume sequence through ACPI
    /// - Guest restores hardware state, resumes processes
    /// - Device drivers reinitialize in guest OS
    /// - Services and applications resume execution
    /// - Guest performs resume handlers (similar to laptop wake)
    /// - Returns immediately but wake happens asynchronously
    ///
    /// # Use Cases
    /// - Resuming from suspend-to-RAM state
    /// - Testing suspend/resume cycles
    /// - Simulating laptop wake-from-sleep
    /// - Restoring VM after power-saving period
    ///
    /// # Requirements
    /// - VM must be in suspended state (from `system_suspend()`)
    /// - Guest must support ACPI wake events
    /// - QEMU configured with ACPI support
    ///
    /// # Notes
    /// - Only works on VMs suspended via `system_suspend()`
    /// - Guest may take several seconds to complete resume
    /// - Network connections need to be reestablished
    /// - Guest clock typically synced via NTP after resume
    /// - Different from `cont()` - guest aware and participates
    pub fn system_wakeup(&mut self) -> Result<()> {
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| HypervisorError::QemuFailed("Not connected to QMP".to_string()))?;

        self.runtime.block_on(async {
            client
                .execute::<(), ()>("system_wakeup", None)
                .await
                .map_err(|e| {
                    HypervisorError::QemuFailed(format!("QMP system_wakeup failed: {}", e))
                })
        })?;

        Ok(())
    }

    /// Reset (reboot) the VM by simulating a hardware reset.
    ///
    /// This command triggers a hard reset of the VM, similar to pressing a physical
    /// reset button. The VM reboots immediately without graceful shutdown, starting
    /// fresh from the bootloader/BIOS.
    ///
    /// # Resource Effects
    /// - **CPU**: vCPUs reset and restart from BIOS/bootloader
    /// - **Memory**: Contents cleared/reinitialized, VM state lost
    /// - **Disk**: Persistent storage preserved but dirty buffers may be lost
    /// - **Network**: Connections abruptly closed, will reinitialize on boot
    /// - **QEMU Process**: Continues running, VM restarts internally
    ///
    /// # Behavior
    /// - Hard reset: immediate reboot without guest cooperation
    /// - Equivalent to physical reset button or power cycle
    /// - Guest does NOT perform graceful shutdown
    /// - Firmware (BIOS/UEFI) reinitializes from scratch
    /// - Kernel boots from beginning
    /// - All in-memory state lost (running processes, RAM contents)
    /// - Disk state preserved (filesystem on disk remains)
    ///
    /// # Use Cases
    /// - Quick VM reboot without full stop/start cycle
    /// - Testing boot sequences repeatedly
    /// - Recovering from guest OS hang or panic
    /// - Simulating unexpected power loss/recovery
    /// - Development workflow for kernel/boot testing
    ///
    /// # Risks
    /// - May cause data loss if buffers not flushed
    /// - Potential filesystem corruption (like unexpected power loss)
    /// - Running processes terminated without cleanup
    /// - Databases may need recovery on next boot
    ///
    /// # Notes
    /// - Faster than stop/start cycle (QEMU process remains running)
    /// - Guest has no opportunity to save state or cleanup
    /// - Prefer `system_powerdown()` for graceful reboot
    /// - Useful for testing but risky for production data
    pub fn system_reset(&mut self) -> Result<()> {
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| HypervisorError::QemuFailed("Not connected to QMP".to_string()))?;

        self.runtime.block_on(async {
            client
                .execute::<(), ()>("system_reset", None)
                .await
                .map_err(|e| {
                    HypervisorError::QemuFailed(format!("QMP system_reset failed: {}", e))
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
