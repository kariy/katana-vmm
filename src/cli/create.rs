use crate::{
    instance::{BootComponents, InstanceConfig, InstanceState, QuotaManager, StorageManager},
    port::PortAllocator,
    qemu::VmManager,
    state::StateDatabase,
};
use anyhow::Result;
use byte_unit::Byte;

pub fn execute(
    name: &str,
    vcpus: u32,
    memory: &str,
    storage: &str,
    port: Option<u16>,
    dev: bool,
    tee: bool,
    vcpu_type: &str,
    db: &StateDatabase,
    storage_manager: &StorageManager,
    port_allocator: &PortAllocator,
    vm_manager: &VmManager,
) -> Result<()> {
    tracing::info!("Creating instance: {}", name);

    // Validate boot components exist before proceeding
    let boot_components = BootComponents::load()?;

    // Check if instance already exists
    if db.instance_exists(name)? {
        anyhow::bail!("Instance '{}' already exists", name);
    }

    // Generate instance ID
    let instance_id = uuid::Uuid::new_v4().to_string();

    // Parse memory size
    let memory_bytes = Byte::parse_str(memory, true)
        .map_err(|e| anyhow::anyhow!("Invalid memory size '{}': {}", memory, e))?
        .as_u64();
    let memory_mb = memory_bytes / 1024 / 1024;

    // Parse storage size
    let storage_bytes = Byte::parse_str(storage, true)
        .map_err(|e| anyhow::anyhow!("Invalid storage size '{}': {}", storage, e))?
        .as_u64();

    // Allocate port
    let rpc_port = if let Some(p) = port {
        if !port_allocator.is_port_available(p)? {
            anyhow::bail!("Port {} is not available", p);
        }
        p
    } else {
        port_allocator.allocate_port(5050)?
    };

    tracing::info!("Allocated RPC port: {}", rpc_port);

    // Create storage directory
    let instance_dir = storage_manager.create_instance_storage(&instance_id, storage_bytes)?;

    // Get paths for instance files
    let paths = storage_manager.get_paths(&instance_id);

    // Build extra args for katana
    let mut extra_args = vec![];
    if tee {
        extra_args.push("--tee.provider".to_string());
        extra_args.push("sev-snp".to_string());
    }

    // Create instance configuration using validated boot components
    let config = InstanceConfig {
        vcpus,
        memory_mb,
        storage_bytes,
        quota_project_id: Some(QuotaManager::derive_project_id(&instance_id)),
        rpc_port,
        metrics_port: None,
        tee_mode: tee,
        vcpu_type: if tee {
            vcpu_type.to_string()
        } else {
            "host".to_string()
        },
        expected_measurement: None,
        kernel_path: boot_components.kernel_path.clone(),
        initrd_path: boot_components.initrd_path.clone(),
        ovmf_path: Some(boot_components.ovmf_path.clone()),
        data_dir: paths.disk_image.parent().unwrap().to_path_buf(), // Keep for backwards compat
        disk_image: Some(paths.disk_image.clone()),
        chain_id: None,
        dev_mode: dev,
        block_time: None,
        accounts: Some(10),
        disable_fee: dev,
        extra_args,
    };

    // Create instance state
    let mut state = InstanceState::new(instance_id.clone(), name.to_string(), config);
    state.serial_log = Some(paths.serial_log.clone());
    state.qmp_socket = Some(paths.qmp_socket.clone());

    // Save to database
    db.save_instance(&state)?;

    // Reserve port in database
    db.allocate_port(&instance_id, rpc_port, "rpc")?;

    println!("✓ Created instance '{}'", name);
    println!("  ID: {}", instance_id);
    println!("  vCPUs: {}", vcpus);
    println!("  Memory: {} MB", memory_mb);
    println!("  Storage: {} GB", storage_bytes / 1024 / 1024 / 1024);
    println!("  RPC Port: {}", rpc_port);
    if tee {
        println!("  TEE Mode: AMD SEV-SNP ({})", vcpu_type);
    }
    println!("  Data Directory: {}", instance_dir.display());

    // Automatically start the instance
    println!("\nStarting instance...");
    crate::cli::start::execute(name, db, vm_manager)?;

    Ok(())
}
