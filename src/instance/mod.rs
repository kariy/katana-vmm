// Instance management module
pub mod config;
pub mod quota;
pub mod state;
pub mod storage;

pub use config::InstanceConfig;
pub use quota::QuotaManager;
pub use state::{InstanceState, InstanceStatus};
pub use storage::StorageManager;

use anyhow::Result;
use std::path::PathBuf;

/// Boot components used by all instances
#[derive(Debug, Clone)]
pub struct BootComponents {
    pub kernel_path: PathBuf,
    pub initrd_path: PathBuf,
    pub ovmf_path: PathBuf,
}

impl BootComponents {
    /// Get the boot components directory path
    /// Looks for boot-components/ relative to the executable
    pub fn get_boot_components_dir() -> PathBuf {
        // Try to find boot-components relative to current directory (development)
        let dev_path = PathBuf::from("boot-components");
        if dev_path.exists() {
            return dev_path;
        }

        // Try relative to executable location
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                let exe_relative = exe_dir.join("boot-components");
                if exe_relative.exists() {
                    return exe_relative;
                }

                // Try one level up (for target/debug/katana-hypervisor structure)
                if let Some(parent_dir) = exe_dir.parent() {
                    let parent_relative = parent_dir.join("boot-components");
                    if parent_relative.exists() {
                        return parent_relative;
                    }
                }
            }
        }

        // Default to /usr/share/katana-hypervisor/boot-components for system install
        PathBuf::from("/usr/share/katana-hypervisor/boot-components")
    }

    /// Load boot components and validate they exist
    pub fn load() -> Result<Self> {
        let boot_dir = Self::get_boot_components_dir();

        let kernel_path = boot_dir.join("vmlinuz");
        let initrd_path = boot_dir.join("initrd.img");
        let ovmf_path = boot_dir.join("ovmf.fd");

        // Validate all components exist
        if !kernel_path.exists() {
            anyhow::bail!(
                "Kernel not found at: {}\n\nBoot components must be built first. See boot-components/README.md",
                kernel_path.display()
            );
        }

        if !initrd_path.exists() {
            anyhow::bail!(
                "Initrd not found at: {}\n\nBoot components must be built first. See boot-components/README.md",
                initrd_path.display()
            );
        }

        if !ovmf_path.exists() {
            anyhow::bail!(
                "OVMF not found at: {}\n\nBoot components must be built first. See boot-components/README.md",
                ovmf_path.display()
            );
        }

        Ok(Self {
            kernel_path,
            initrd_path,
            ovmf_path,
        })
    }
}
