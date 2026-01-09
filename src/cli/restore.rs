use crate::{
    instance::{BootComponents, InstanceConfig, InstanceState, QuotaManager, StorageManager},
    port::PortAllocator,
    state::StateDatabase,
};
use anyhow::Result;
use byte_unit::Byte;
use std::fs;
use std::path::PathBuf;

pub fn execute(
    backup_dir: PathBuf,
    instance_name: &str,
    vcpus: Option<u32>,
    memory: Option<&str>,
    storage: Option<&str>,
    port: Option<u16>,
    db: &StateDatabase,
    storage_manager: &StorageManager,
    port_allocator: &PortAllocator,
) -> Result<()> {
    tracing::info!("Restoring instance from backup: {}", backup_dir.display());

    // Check if backup directory exists
    if !backup_dir.exists() || !backup_dir.is_dir() {
        anyhow::bail!("Backup directory not found: {}", backup_dir.display());
    }

    // Load metadata
    let metadata_file = backup_dir.join("metadata.json");
    if !metadata_file.exists() {
        anyhow::bail!(
            "metadata.json not found in backup directory. Is this a valid backup?"
        );
    }

    let metadata_content = fs::read_to_string(&metadata_file)?;
    let metadata: serde_json::Value = serde_json::from_str(&metadata_content)?;

    println!("Restoring from backup...");
    println!("  Original instance: {}", metadata["instance_name"]);
    println!(
        "  Backup timestamp: {}",
        metadata["backup_timestamp"].as_str().unwrap_or("unknown")
    );

    // Check if katana-db exists in backup
    let katana_db_src = backup_dir.join("katana-db");
    if !katana_db_src.exists() {
        anyhow::bail!("katana-db directory not found in backup");
    }

    // Validate boot components exist
    let boot_components = BootComponents::load()?;

    // Check if new instance name already exists
    if db.instance_exists(instance_name)? {
        anyhow::bail!("Instance '{}' already exists", instance_name);
    }

    // Use provided config or fall back to original config
    let original_config = &metadata["config"];

    let vcpus_value = vcpus.unwrap_or_else(|| {
        original_config["vcpus"].as_u64().unwrap_or(4) as u32
    });

    let memory_mb = if let Some(mem_str) = memory {
        let memory_bytes = Byte::parse_str(mem_str, true)
            .map_err(|e| anyhow::anyhow!("Invalid memory size '{}': {}", mem_str, e))?
            .as_u64();
        memory_bytes / 1024 / 1024
    } else {
        original_config["memory_mb"].as_u64().unwrap_or(4096)
    };

    let storage_bytes = if let Some(stor_str) = storage {
        Byte::parse_str(stor_str, true)
            .map_err(|e| anyhow::anyhow!("Invalid storage size '{}': {}", stor_str, e))?
            .as_u64()
    } else {
        original_config["storage_bytes"].as_u64().unwrap_or(10 * 1024 * 1024 * 1024)
    };

    println!("\nNew instance configuration:");
    println!("  Name: {}", instance_name);
    println!("  vCPUs: {}", vcpus_value);
    println!("  Memory: {} MB", memory_mb);
    println!("  Storage: {} GB", storage_bytes / 1024 / 1024 / 1024);

    // Generate new instance ID
    let instance_id = uuid::Uuid::new_v4().to_string();

    // Allocate port
    let rpc_port = if let Some(p) = port {
        if !port_allocator.is_port_available(p)? {
            anyhow::bail!("Port {} is not available", p);
        }
        p
    } else {
        port_allocator.allocate_port(5050)?
    };

    println!("  RPC Port: {}", rpc_port);

    // Create storage directory and disk image
    println!("\nCreating storage...");
    let instance_dir = storage_manager.create_instance_storage(&instance_id, storage_bytes)?;
    let paths = storage_manager.get_paths(&instance_id);

    // Get TEE settings from original config
    let tee_mode = original_config["tee_mode"].as_bool().unwrap_or(false);
    let vcpu_type = original_config["vcpu_type"]
        .as_str()
        .unwrap_or("host")
        .to_string();
    let dev_mode = original_config["dev_mode"].as_bool().unwrap_or(false);

    // Build extra args if TEE mode
    let mut extra_args = vec![];
    if tee_mode {
        extra_args.push("--tee.provider".to_string());
        extra_args.push("sev-snp".to_string());
    }

    // Create instance configuration
    let config = InstanceConfig {
        vcpus: vcpus_value,
        memory_mb,
        storage_bytes,
        quota_project_id: Some(QuotaManager::derive_project_id(&instance_id)),
        rpc_port,
        metrics_port: None,
        tee_mode,
        vcpu_type: if tee_mode {
            vcpu_type.clone()
        } else {
            "host".to_string()
        },
        expected_measurement: None,
        kernel_path: boot_components.kernel_path.clone(),
        initrd_path: boot_components.initrd_path.clone(),
        ovmf_path: Some(boot_components.ovmf_path.clone()),
        data_dir: paths.disk_image.parent().unwrap().to_path_buf(),
        disk_image: Some(paths.disk_image.clone()),
        chain_id: None,
        dev_mode,
        block_time: None,
        accounts: Some(10),
        disable_fee: dev_mode,
        extra_args,
    };

    // Create instance state
    let mut state = InstanceState::new(instance_id.clone(), instance_name.to_string(), config);
    state.serial_log = Some(paths.serial_log.clone());
    state.qmp_socket = Some(paths.qmp_socket.clone());

    // Save to database
    db.save_instance(&state)?;
    db.allocate_port(&instance_id, rpc_port, "rpc")?;

    println!("  ✓ Instance created");

    // Mount the disk image
    println!("\nRestoring data...");
    println!("  Mounting disk image...");
    let (mount_point, nbd_device) = storage_manager.mount_disk_image(&instance_id)?;

    // Copy the katana database
    let katana_db_dst = mount_point.join("katana-db");
    println!("  Copying katana database...");

    // Create destination directory
    fs::create_dir_all(&katana_db_dst)?;

    // Copy files
    copy_dir_recursive(&katana_db_src, &katana_db_dst)?;

    // Get restored size
    let restored_size = get_dir_size(&katana_db_dst)?;
    println!(
        "  ✓ Database restored ({:.2} MB)",
        restored_size as f64 / 1024.0 / 1024.0
    );

    // Unmount
    println!("  Unmounting disk image...");
    storage_manager.unmount_disk_image(&instance_id, &mount_point, &nbd_device)?;

    println!("\n✓ Instance '{}' restored successfully", instance_name);
    println!("  ID: {}", instance_id);
    println!("  Data Directory: {}", instance_dir.display());
    println!("\nYou can now start the instance with:");
    println!("  katana-hypervisor start {}", instance_name);

    Ok(())
}

/// Recursively copy a directory
fn copy_dir_recursive(src: &PathBuf, dst: &PathBuf) -> Result<()> {
    if !dst.exists() {
        fs::create_dir_all(dst)?;
    }

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let entry_path = entry.path();
        let file_name = entry.file_name();
        let dest_path = dst.join(&file_name);

        if entry_path.is_dir() {
            copy_dir_recursive(&entry_path, &dest_path)?;
        } else {
            fs::copy(&entry_path, &dest_path)?;
        }
    }

    Ok(())
}

/// Calculate the total size of a directory
fn get_dir_size(path: &PathBuf) -> Result<u64> {
    let mut total = 0u64;

    if path.is_file() {
        return Ok(fs::metadata(path)?.len());
    }

    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let entry_path = entry.path();

        if entry_path.is_file() {
            total += fs::metadata(&entry_path)?.len();
        } else if entry_path.is_dir() {
            total += get_dir_size(&entry_path)?;
        }
    }

    Ok(total)
}
