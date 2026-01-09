use crate::{
    instance::{InstanceStatus, StorageManager},
    state::StateDatabase,
};
use anyhow::Result;
use std::fs;
use std::path::PathBuf;

pub fn execute(
    name: &str,
    output_dir: PathBuf,
    db: &StateDatabase,
    storage_manager: &StorageManager,
) -> Result<()> {
    tracing::info!("Backing up instance: {}", name);

    // Load instance from database
    let state = db.get_instance(name)?;

    // Check if instance is running
    let was_running = state.status == InstanceStatus::Running;
    if was_running {
        anyhow::bail!(
            "Instance '{}' is running. Please stop it first:\n  katana-hypervisor stop {}",
            name,
            name
        );
    }

    // Create output directory
    fs::create_dir_all(&output_dir)?;

    println!("Backing up instance '{}'...", name);
    println!("  Output directory: {}", output_dir.display());

    // Mount the disk image
    println!("  Mounting disk image...");
    let (mount_point, nbd_device) = storage_manager.mount_disk_image(&state.id)?;

    // Copy the katana database
    let katana_db_src = mount_point.join("katana-db");
    let katana_db_dst = output_dir.join("katana-db");

    if !katana_db_src.exists() {
        tracing::warn!("Katana database directory not found in disk image");
        println!("  Warning: No katana database found in disk image");
    } else {
        println!("  Copying katana database...");
        copy_dir_recursive(&katana_db_src, &katana_db_dst)?;

        // Get backup size
        let backup_size = get_dir_size(&katana_db_dst)?;
        println!(
            "  ✓ Database backed up ({:.2} MB)",
            backup_size as f64 / 1024.0 / 1024.0
        );
    }

    // Save instance metadata
    println!("  Saving instance metadata...");
    let metadata = serde_json::json!({
        "instance_name": state.name,
        "instance_id": state.id,
        "backup_timestamp": chrono::Utc::now().to_rfc3339(),
        "config": {
            "vcpus": state.config.vcpus,
            "memory_mb": state.config.memory_mb,
            "storage_bytes": state.config.storage_bytes,
            "rpc_port": state.config.rpc_port,
            "tee_mode": state.config.tee_mode,
            "vcpu_type": state.config.vcpu_type,
            "dev_mode": state.config.dev_mode,
        }
    });

    let metadata_file = output_dir.join("metadata.json");
    fs::write(&metadata_file, serde_json::to_string_pretty(&metadata)?)?;
    println!("  ✓ Metadata saved");

    // Unmount the disk image
    println!("  Unmounting disk image...");
    storage_manager.unmount_disk_image(&state.id, &mount_point, &nbd_device)?;

    println!("\n✓ Backup completed successfully");
    println!("  Location: {}", output_dir.display());
    println!("  Files:");
    println!("    - katana-db/     (Katana database)");
    println!("    - metadata.json  (Instance configuration)");

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
