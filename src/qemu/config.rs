use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct QemuConfig {
    // Resource limits
    pub memory_mb: u64,
    pub vcpus: u32,
    pub cpu_type: String,

    // Boot components
    pub kernel_path: PathBuf,
    pub initrd_path: PathBuf,
    pub bios_path: Option<PathBuf>,

    // Kernel command line
    pub kernel_cmdline: String,

    // Network
    pub rpc_port: u16,

    // Storage
    pub disk_image: Option<PathBuf>,

    // Paths
    pub qmp_socket: PathBuf,
    pub serial_log: PathBuf,
    pub pid_file: PathBuf,

    // TEE configuration
    pub sev_snp: Option<SevSnpConfig>,

    // Enable KVM acceleration
    pub enable_kvm: bool,
}

#[derive(Debug, Clone)]
pub struct SevSnpConfig {
    pub cbitpos: u8,
    pub reduced_phys_bits: u8,
    pub vcpu_type: String,
}

impl QemuConfig {
    /// Build QEMU command line arguments
    pub fn to_qemu_args(&self) -> Vec<String> {
        let mut args = vec!["qemu-system-x86_64".to_string()];

        // Enable KVM if requested
        if self.enable_kvm {
            args.push("-enable-kvm".to_string());
        }

        // CPU configuration
        if let Some(ref sev_snp) = self.sev_snp {
            // SEV-SNP mode
            args.push("-cpu".to_string());
            args.push(sev_snp.vcpu_type.clone());

            // Machine type with confidential guest support
            args.push("-machine".to_string());
            args.push("q35,confidential-guest-support=sev0".to_string());

            // SEV-SNP guest object
            args.push("-object".to_string());
            args.push(format!(
                "sev-snp-guest,id=sev0,cbitpos={},reduced-phys-bits={}",
                sev_snp.cbitpos, sev_snp.reduced_phys_bits
            ));

            // BIOS (OVMF) is required for SEV
            if let Some(ref bios_path) = self.bios_path {
                args.push("-bios".to_string());
                args.push(bios_path.to_string_lossy().to_string());
            }
        } else {
            // Non-TEE mode
            args.push("-cpu".to_string());
            args.push(self.cpu_type.clone());

            args.push("-machine".to_string());
            args.push("q35".to_string());
        }

        // Memory and vCPUs
        args.push("-smp".to_string());
        args.push(self.vcpus.to_string());

        args.push("-m".to_string());
        args.push(format!("{}M", self.memory_mb));

        // Kernel boot
        args.push("-kernel".to_string());
        args.push(self.kernel_path.to_string_lossy().to_string());

        args.push("-initrd".to_string());
        args.push(self.initrd_path.to_string_lossy().to_string());

        args.push("-append".to_string());
        args.push(self.kernel_cmdline.clone());

        // Network - user networking with port forwarding
        args.push("-netdev".to_string());
        args.push(format!(
            "user,id=net0,hostfwd=tcp::{}-:5050",
            self.rpc_port
        ));

        args.push("-device".to_string());
        args.push("virtio-net-pci,netdev=net0".to_string());

        // Storage - virtio-blk disk image
        if let Some(ref disk_path) = self.disk_image {
            args.push("-drive".to_string());
            args.push(format!(
                "file={},if=virtio,format=qcow2",
                disk_path.to_string_lossy()
            ));
        }

        // No graphics (use -display none instead of -nographic for compatibility with -daemonize)
        args.push("-display".to_string());
        args.push("none".to_string());

        // Serial console to file
        args.push("-serial".to_string());
        args.push(format!("file:{}", self.serial_log.to_string_lossy()));

        // QMP socket
        args.push("-qmp".to_string());
        args.push(format!(
            "unix:{},server,nowait",
            self.qmp_socket.to_string_lossy()
        ));

        // Daemonize
        args.push("-daemonize".to_string());

        // PID file
        args.push("-pidfile".to_string());
        args.push(self.pid_file.to_string_lossy().to_string());

        args
    }

    /// Build kernel command line with katana arguments
    pub fn build_kernel_cmdline(katana_args: &[String]) -> String {
        let katana_args_str = katana_args.join(" ");

        format!(
            "console=ttyS0 loglevel=4 katana.args={}",
            katana_args_str
        )
    }
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod config_tests;
