use crate::{
    instance::InstanceStatus,
    qemu::{QemuConfig, QmpClient, VmManager},
    state::StateDatabase,
};
use anyhow::Result;
use std::thread;
use std::time::Duration;

pub fn execute(name: &str, db: &StateDatabase, vm_manager: &VmManager) -> Result<()> {
    tracing::info!("Starting instance: {}", name);

    // Load instance from database
    let mut state = db.get_instance(name)?;

    // Check state
    match state.status {
        InstanceStatus::Running => {
            println!("Instance '{}' is already running", name);
            return Ok(());
        }
        InstanceStatus::Starting => {
            anyhow::bail!("Instance '{}' is already starting", name);
        }
        _ => {}
    }

    // Check if boot components exist BEFORE updating status
    if !state.config.kernel_path.exists() {
        anyhow::bail!(
            "Kernel not found at {}. Please build boot components first:\n  cd /home/ubuntu/katana && make build-tee",
            state.config.kernel_path.display()
        );
    }

    if !state.config.initrd_path.exists() {
        anyhow::bail!(
            "Initrd not found at {}. Please build boot components first:\n  cd /home/ubuntu/katana && make build-tee",
            state.config.initrd_path.display()
        );
    }

    // Build katana arguments
    let katana_args = state.config.build_katana_args();
    let kernel_cmdline = QemuConfig::build_kernel_cmdline(&katana_args);

    // Build SEV-SNP config if TEE mode is enabled
    let sev_snp_config = if state.config.tee_mode {
        Some(crate::qemu::config::SevSnpConfig {
            cbitpos: 51,           // C-bit position for AMD EPYC
            reduced_phys_bits: 1,  // Reserved physical address bits
            vcpu_type: state.config.vcpu_type.clone(),
        })
    } else {
        None
    };

    // Build QEMU configuration
    let qemu_config = QemuConfig {
        memory_mb: state.config.memory_mb,
        vcpus: state.config.vcpus,
        cpu_type: state.config.vcpu_type.clone(),
        kernel_path: state.config.kernel_path.clone(),
        initrd_path: state.config.initrd_path.clone(),
        bios_path: state.config.ovmf_path.clone(),
        kernel_cmdline,
        rpc_port: state.config.rpc_port,
        disk_image: state.config.disk_image.clone(),
        qmp_socket: state.qmp_socket.clone().unwrap(),
        serial_log: state.serial_log.clone().unwrap(),
        pid_file: std::path::PathBuf::from(format!(
            "/tmp/katana-hypervisor-{}.pid",
            state.id
        )),
        sev_snp: sev_snp_config,
        enable_kvm: true,
    };

    println!("Starting QEMU VM...");
    println!("  Kernel: {}", qemu_config.kernel_path.display());
    println!("  Initrd: {}", qemu_config.initrd_path.display());
    println!("  vCPUs: {}", qemu_config.vcpus);
    println!("  Memory: {} MB", qemu_config.memory_mb);
    println!("  RPC Port: {}", qemu_config.rpc_port);
    if state.config.tee_mode {
        println!("  TEE Mode: AMD SEV-SNP ({})", state.config.vcpu_type);
    }

    // Launch VM
    let pid = vm_manager.launch_vm(&qemu_config)?;

    // Update state with PID
    state.vm_pid = Some(pid);
    state.update_status(InstanceStatus::Running);
    db.save_instance(&state)?;

    println!("\n✓ QEMU process started (PID: {})", pid);
    println!("  Waiting for VM to initialize...");

    // Wait for QMP socket to be created
    let qmp_socket = state.qmp_socket.clone().unwrap();
    let max_wait = 30; // seconds
    let mut waited = 0;

    while !qmp_socket.exists() && waited < max_wait {
        thread::sleep(Duration::from_millis(500));
        waited += 1;

        // Check if process is still alive
        if !vm_manager.is_process_running(pid) {
            anyhow::bail!("QEMU process died unexpectedly. Check logs at: {}",
                state.serial_log.unwrap().display());
        }
    }

    if !qmp_socket.exists() {
        anyhow::bail!("QMP socket not created after {} seconds", max_wait);
    }

    // Connect to QMP and verify VM is running
    print!("  Connecting to QMP...");
    std::io::Write::flush(&mut std::io::stdout())?;

    let mut qmp_client = QmpClient::new();
    let mut connected = false;

    for _ in 0..10 {
        if let Ok(_) = qmp_client.connect(&qmp_socket) {
            connected = true;
            break;
        }
        thread::sleep(Duration::from_millis(500));
    }

    if !connected {
        anyhow::bail!("Failed to connect to QMP socket");
    }

    // Query VM status
    match qmp_client.query_status() {
        Ok(status) => {
            if status.running {
                println!(" ✓");
            } else {
                println!(" VM not running (status: {})", status.status);
            }
        }
        Err(e) => {
            println!(" Warning: Could not query VM status: {}", e);
        }
    }

    // Wait for katana HTTP endpoint to be responsive
    print!("  Waiting for katana RPC...");
    std::io::Write::flush(&mut std::io::stdout())?;

    let rpc_url = format!("http://localhost:{}/", state.config.rpc_port);
    let mut katana_ready = false;

    for _ in 0..30 {
        if let Ok(response) = ureq::get(&rpc_url).timeout(Duration::from_secs(1)).call() {
            if response.status() == 200 {
                katana_ready = true;
                println!(" ✓");
                break;
            }
        }
        thread::sleep(Duration::from_secs(1));
    }

    if !katana_ready {
        println!(" Timed out");
        println!("\nWarning: Katana may still be initializing.");
        println!("Check logs with: katana-hypervisor logs {}", name);
    }

    println!("\n✓ Instance '{}' started successfully", name);
    println!("  PID: {}", pid);
    println!("  RPC Endpoint: http://localhost:{}", state.config.rpc_port);
    println!("  Health Check: http://localhost:{}/", state.config.rpc_port);
    println!("  Serial Log: {}", state.serial_log.unwrap().display());

    Ok(())
}
