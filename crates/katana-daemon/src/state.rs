use anyhow::{Context, Result};
use katana_core::{
    instance::StorageManager,
    port::PortAllocator,
    state::StateDatabase,
};
use std::path::PathBuf;

/// Daemon state shared across request handlers
pub struct DaemonState {
    pub db: StateDatabase,
    pub storage: StorageManager,
    pub port_allocator: PortAllocator,
}

impl DaemonState {
    pub fn new() -> Result<Self> {
        // Determine state directory
        let state_dir = if let Ok(dir) = std::env::var("KATANA_STATE_DIR") {
            PathBuf::from(dir)
        } else {
            // Use XDG data home
            directories::ProjectDirs::from("", "", "katana")
                .context("Failed to determine project directories")?
                .data_dir()
                .join("hypervisor")
        };

        // Ensure state directory exists
        std::fs::create_dir_all(&state_dir)
            .context("Failed to create state directory")?;

        let db_path = state_dir.join("state.db");
        let instances_dir = state_dir.join("instances");

        tracing::info!("State directory: {}", state_dir.display());
        tracing::info!("Database path: {}", db_path.display());
        tracing::info!("Instances directory: {}", instances_dir.display());

        // Initialize components
        let db = StateDatabase::new(&db_path)
            .context("Failed to initialize state database")?;

        let storage = StorageManager::new(instances_dir);

        let port_allocator = PortAllocator::new(db.clone());

        Ok(Self {
            db,
            storage,
            port_allocator,
        })
    }
}
