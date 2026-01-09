use crate::{instance::InstanceStatus, instance::StorageManager, state::StateDatabase};
use anyhow::Result;
use std::fs;
use std::path::PathBuf;

pub fn execute(
    name: &str,
    output_path: PathBuf,
    db: &StateDatabase,
    storage_manager: &StorageManager,
) -> Result<()> {
    tracing::info!("Exporting instance disk image: {}", name);

    // Load instance from database
    let state = db.get_instance(name)?;

    // Check if instance is running
    if state.status == InstanceStatus::Running {
        anyhow::bail!(
            "Instance '{}' is running. Please stop it first:\n  katana-hypervisor stop {}",
            name,
            name
        );
    }

    // Get disk image path
    let paths = storage_manager.get_paths(&state.id);

    if !paths.disk_image.exists() {
        anyhow::bail!("Disk image not found: {}", paths.disk_image.display());
    }

    println!("Exporting disk image for instance '{}'...", name);
    println!("  Source: {}", paths.disk_image.display());
    println!("  Destination: {}", output_path.display());

    // Get source file size
    let metadata = fs::metadata(&paths.disk_image)?;
    let file_size_mb = metadata.len() as f64 / 1024.0 / 1024.0;

    println!("  Size: {:.2} MB", file_size_mb);
    println!("  Copying...");

    // Create parent directory if needed
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }

    // Copy the disk image file
    fs::copy(&paths.disk_image, &output_path)?;

    // Verify copy
    let copied_metadata = fs::metadata(&output_path)?;
    if copied_metadata.len() != metadata.len() {
        anyhow::bail!("Export verification failed: file sizes don't match");
    }

    println!("\n✓ Disk image exported successfully");
    println!("  Location: {}", output_path.display());
    println!("  Format: qcow2");
    println!("\nYou can now:");
    println!("  - Use this disk image with another hypervisor");
    println!("  - Restore it to a new instance with: katana-hypervisor restore");
    println!("  - Mount it directly with: qemu-nbd");

    Ok(())
}
