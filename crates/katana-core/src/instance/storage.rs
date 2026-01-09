use crate::{HypervisorError, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Minimum storage size in bytes (50MB)
const MIN_STORAGE_SIZE: u64 = 50 * 1024 * 1024;

pub struct StorageManager {
    base_dir: PathBuf,
}

impl StorageManager {
    pub fn new(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    /// Create storage directory for an instance with a qcow2 disk image
    pub fn create_instance_storage(&self, instance_id: &str, quota_bytes: u64) -> Result<PathBuf> {
        // Validate minimum storage size
        if quota_bytes < MIN_STORAGE_SIZE {
            return Err(HypervisorError::InvalidConfig(format!(
                "Storage size must be at least {} MB (requested: {} bytes)",
                MIN_STORAGE_SIZE / (1024 * 1024),
                quota_bytes
            )));
        }

        let instance_dir = self.base_dir.join(instance_id);

        // Create instance directory
        fs::create_dir_all(&instance_dir)?;

        // Create qcow2 disk image for persistent storage
        let disk_image = instance_dir.join("katana-data.qcow2");

        // Calculate size in MB for better precision (minimum 50MB)
        let size_mb = quota_bytes / (1024 * 1024);

        tracing::info!(
            instance_id = %instance_id,
            size_mb = size_mb,
            size_bytes = quota_bytes,
            path = %disk_image.display(),
            "Creating qcow2 disk image"
        );

        let status = Command::new("qemu-img")
            .args(&[
                "create",
                "-f",
                "qcow2",
                disk_image.to_str().ok_or_else(|| {
                    HypervisorError::InvalidConfig("Invalid disk image path".into())
                })?,
                &format!("{size_mb}M"),
            ])
            .status()
            .map_err(|e| {
                HypervisorError::InvalidConfig(format!("Failed to run qemu-img: {}", e))
            })?;

        if !status.success() {
            return Err(HypervisorError::InvalidConfig(
                "Failed to create qcow2 disk image".into(),
            ));
        }

        tracing::info!(
            instance_id = %instance_id,
            "Successfully created qcow2 disk image"
        );

        // Format the disk image with ext4 filesystem using qemu-nbd
        tracing::info!(
            instance_id = %instance_id,
            "Formatting disk image with ext4 using qemu-nbd"
        );

        // Try to format using qemu-nbd (requires nbd kernel module)
        match Self::format_qcow2_with_nbd(&disk_image) {
            Ok(_) => {
                tracing::info!(
                    instance_id = %instance_id,
                    "Successfully formatted disk image"
                );
            }
            Err(e) => {
                tracing::warn!(
                    instance_id = %instance_id,
                    error = %e,
                    "Failed to format disk image, will format in VM"
                );
            }
        }

        Ok(instance_dir)
    }

    /// Format a qcow2 image using qemu-nbd
    fn format_qcow2_with_nbd(disk_image: &Path) -> Result<()> {
        // Find an available nbd device
        let nbd_device = Self::find_available_nbd()?;

        // Load nbd kernel module if not loaded (best effort)
        let _ = Command::new("modprobe").arg("nbd").status();

        // Connect qcow2 image to nbd device
        tracing::debug!("Connecting {} to {}", disk_image.display(), nbd_device);
        let status = Command::new("qemu-nbd")
            .args(&[
                "--connect",
                &nbd_device,
                disk_image.to_str().ok_or_else(|| {
                    HypervisorError::InvalidConfig("Invalid disk image path".into())
                })?,
            ])
            .status()
            .map_err(|e| {
                HypervisorError::InvalidConfig(format!("Failed to run qemu-nbd: {}", e))
            })?;

        if !status.success() {
            return Err(HypervisorError::InvalidConfig(
                "Failed to connect qcow2 image via qemu-nbd".into(),
            ));
        }

        // Wait a bit for device to be ready
        std::thread::sleep(std::time::Duration::from_millis(500));

        // Format the nbd device
        tracing::debug!("Formatting {} with ext4", nbd_device);
        let format_result = Command::new("mkfs.ext4")
            .args(&["-F", &nbd_device])
            .output()
            .map_err(|e| HypervisorError::InvalidConfig(format!("Failed to run mkfs.ext4: {}", e)));

        // Disconnect nbd device (always do this, even if formatting failed)
        tracing::debug!("Disconnecting {}", nbd_device);
        let disconnect_result = Command::new("qemu-nbd")
            .args(&["--disconnect", &nbd_device])
            .status();

        if let Err(e) = disconnect_result {
            tracing::warn!("Failed to disconnect NBD device {}: {}", nbd_device, e);
        } else if let Ok(status) = disconnect_result {
            if !status.success() {
                tracing::warn!("NBD disconnect command failed for {}", nbd_device);
            }
        }

        // Check formatting result
        match format_result {
            Ok(output) => {
                if !output.status.success() {
                    return Err(HypervisorError::InvalidConfig(format!(
                        "Failed to format disk: {}",
                        String::from_utf8_lossy(&output.stderr)
                    )));
                }
            }
            Err(e) => return Err(e),
        }

        Ok(())
    }

    /// Get the instance directory path
    pub fn get_instance_dir(&self, instance_id: &str) -> PathBuf {
        self.base_dir.join(instance_id)
    }

    /// Get disk usage for an instance (queries qcow2 actual size)
    pub fn get_disk_usage(&self, instance_id: &str) -> Result<u64> {
        let instance_dir = self.base_dir.join(instance_id);
        let disk_image = instance_dir.join("katana-data.qcow2");

        if !disk_image.exists() {
            return Ok(0);
        }

        // Use qemu-img info to get actual disk usage
        let output = Command::new("qemu-img")
            .args(&["info", "--output=json", disk_image.to_str().unwrap()])
            .output()
            .map_err(|e| {
                HypervisorError::InvalidConfig(format!("Failed to run qemu-img: {}", e))
            })?;

        if !output.status.success() {
            // Fallback to file size if qemu-img fails
            return Ok(fs::metadata(&disk_image)?.len());
        }

        // Parse JSON output to get actual-size
        let json_str = String::from_utf8_lossy(&output.stdout);
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&json_str) {
            if let Some(actual_size) = json.get("actual-size").and_then(|v| v.as_u64()) {
                return Ok(actual_size);
            }
        }

        // Fallback to file size
        Ok(fs::metadata(&disk_image)?.len())
    }

    /// Check if storage quota is exceeded
    pub fn check_quota(&self, instance_id: &str, quota_bytes: u64) -> Result<()> {
        let usage = self.get_disk_usage(instance_id)?;

        if usage > quota_bytes {
            return Err(HypervisorError::StorageQuotaExceeded {
                used: usage,
                limit: quota_bytes,
            });
        }

        Ok(())
    }

    /// Delete instance storage
    pub fn delete_instance_storage(&self, instance_id: &str) -> Result<()> {
        let instance_dir = self.base_dir.join(instance_id);

        if instance_dir.exists() {
            fs::remove_dir_all(&instance_dir)?;
        }

        Ok(())
    }

    /// Get paths for instance files
    pub fn get_paths(&self, instance_id: &str) -> InstancePaths {
        let instance_dir = self.base_dir.join(instance_id);

        InstancePaths {
            instance_dir: instance_dir.clone(),
            disk_image: instance_dir.join("katana-data.qcow2"),
            serial_log: instance_dir.join("serial.log"),
            qmp_socket: instance_dir.join("qmp.sock"),
            pid_file: instance_dir.join("qemu.pid"),
        }
    }

    /// Mount a qcow2 disk image to access its contents
    /// Returns (mount_point, nbd_device)
    pub fn mount_disk_image(&self, instance_id: &str) -> Result<(PathBuf, String)> {
        let paths = self.get_paths(instance_id);

        if !paths.disk_image.exists() {
            return Err(HypervisorError::InvalidConfig(
                "Disk image does not exist".into(),
            ));
        }

        // Find an available nbd device
        let nbd_device = Self::find_available_nbd()?;
        let mount_point = PathBuf::from(format!("/tmp/katana-mount-{}", instance_id));

        tracing::info!(
            instance_id = %instance_id,
            nbd_device = %nbd_device,
            mount_point = %mount_point.display(),
            "Mounting disk image"
        );

        // Connect qcow2 to nbd device
        let status = Command::new("qemu-nbd")
            .args(&["--connect", &nbd_device, paths.disk_image.to_str().unwrap()])
            .status()
            .map_err(|e| {
                HypervisorError::InvalidConfig(format!("Failed to run qemu-nbd: {}", e))
            })?;

        if !status.success() {
            return Err(HypervisorError::InvalidConfig(
                "Failed to connect qcow2 image via qemu-nbd".into(),
            ));
        }

        // Wait for device to be ready
        std::thread::sleep(std::time::Duration::from_millis(500));

        // Create mount point
        fs::create_dir_all(&mount_point)?;

        // Mount the device
        let status = Command::new("mount")
            .args(&["-t", "ext4", &nbd_device, mount_point.to_str().unwrap()])
            .status()
            .map_err(|e| {
                // Clean up nbd connection on mount failure
                let _ = Command::new("qemu-nbd")
                    .args(&["--disconnect", &nbd_device])
                    .status();
                HypervisorError::InvalidConfig(format!("Failed to mount disk: {}", e))
            })?;

        if !status.success() {
            // Clean up nbd connection
            let _ = Command::new("qemu-nbd")
                .args(&["--disconnect", &nbd_device])
                .status();
            return Err(HypervisorError::InvalidConfig(
                "Failed to mount disk image".into(),
            ));
        }

        tracing::info!(
            instance_id = %instance_id,
            mount_point = %mount_point.display(),
            nbd_device = %nbd_device,
            "Disk image mounted successfully"
        );

        Ok((mount_point, nbd_device))
    }

    /// Unmount a previously mounted disk image
    pub fn unmount_disk_image(
        &self,
        instance_id: &str,
        mount_point: &Path,
        nbd_device: &str,
    ) -> Result<()> {
        tracing::info!(
            instance_id = %instance_id,
            mount_point = %mount_point.display(),
            nbd_device = %nbd_device,
            "Unmounting disk image"
        );

        // Unmount
        let status = Command::new("umount")
            .arg(mount_point.to_str().unwrap())
            .status()
            .map_err(|e| {
                HypervisorError::InvalidConfig(format!("Failed to unmount disk: {}", e))
            })?;

        if !status.success() {
            tracing::warn!(
                instance_id = %instance_id,
                "Failed to unmount disk, attempting force unmount"
            );
            // Try force unmount
            let _ = Command::new("umount")
                .args(&["-f", mount_point.to_str().unwrap()])
                .status();
        }

        // Disconnect nbd device
        let status = Command::new("qemu-nbd")
            .args(&["--disconnect", nbd_device])
            .status()
            .map_err(|e| {
                HypervisorError::InvalidConfig(format!("Failed to disconnect nbd: {}", e))
            })?;

        if !status.success() {
            tracing::warn!(
                instance_id = %instance_id,
                nbd_device = %nbd_device,
                "Failed to disconnect nbd device"
            );
        }

        // Remove mount point
        let _ = fs::remove_dir(mount_point);

        tracing::info!(
            instance_id = %instance_id,
            "Disk image unmounted successfully"
        );

        Ok(())
    }

    /// Find an available nbd device
    fn find_available_nbd() -> Result<String> {
        // Try nbd0 through nbd15
        for i in 0..16 {
            let device = format!("/dev/nbd{}", i);
            let device_path = Path::new(&device);

            if !device_path.exists() {
                continue;
            }

            // Check if device is in use by trying to read its size
            let output = Command::new("blockdev")
                .args(&["--getsize64", &device])
                .output();

            if let Ok(output) = output {
                let size_str = String::from_utf8_lossy(&output.stdout);
                if let Ok(size) = size_str.trim().parse::<u64>() {
                    if size == 0 {
                        return Ok(device);
                    }
                }
            } else {
                // If blockdev fails, device is likely available
                return Ok(device);
            }
        }

        Err(HypervisorError::InvalidConfig(
            "No available nbd devices found. Try: sudo modprobe nbd max_part=8".into(),
        ))
    }
}

pub struct InstancePaths {
    pub instance_dir: PathBuf,
    pub disk_image: PathBuf,
    pub serial_log: PathBuf,
    pub qmp_socket: PathBuf,
    pub pid_file: PathBuf,
}

#[cfg(test)]
#[path = "storage_tests.rs"]
mod storage_tests;
