use katana_hypervisor::{
    instance::{config::InstanceConfig, state::InstanceStatus, storage::StorageManager},
    port::allocator::PortAllocator,
    qemu::config::QemuConfig,
    state::db::StateDatabase,
};
use std::path::PathBuf;
use tempfile::TempDir;

fn create_test_environment() -> (StateDatabase, StorageManager, PortAllocator, TempDir, TempDir) {
    let state_temp = TempDir::new().unwrap();
    let storage_temp = TempDir::new().unwrap();

    let db_path = state_temp.path().join("test.db");
    let db = StateDatabase::new(&db_path).unwrap();

    let storage = StorageManager::new(storage_temp.path().to_path_buf());
    let port_allocator = PortAllocator::new(db.clone());

    (db, storage, port_allocator, state_temp, storage_temp)
}

fn create_test_config(rpc_port: u16) -> InstanceConfig {
    InstanceConfig {
        vcpus: 2,
        memory_mb: 2048,
        storage_bytes: 1_000_000_000,
        rpc_port,
        metrics_port: None,
        tee_mode: false,
        vcpu_type: "host".to_string(),
        expected_measurement: None,
        kernel_path: PathBuf::from("/tmp/vmlinuz"),
        initrd_path: PathBuf::from("/tmp/initrd.img"),
        ovmf_path: None,
        data_dir: PathBuf::from("/tmp/data"),
        chain_id: None,
        dev_mode: true,
        block_time: None,
        accounts: Some(10),
        disable_fee: false,
        extra_args: vec![],
    }
}

#[test]
fn test_full_instance_lifecycle() {
    let (db, storage, port_allocator, _state_temp, _storage_temp) = create_test_environment();

    // Allocate port (use high port number unlikely to be in use)
    let base_port = 55000u16;
    let port = port_allocator.allocate_port(base_port).unwrap();
    assert_eq!(port, base_port);

    // Create instance configuration
    let config = create_test_config(port);

    // Create storage
    let instance_id = "test-instance-1";
    let instance_dir = storage.create_instance_storage(instance_id, 1_000_000_000).unwrap();
    assert!(instance_dir.exists());

    // Get paths
    let paths = storage.get_paths(instance_id);
    assert_eq!(paths.instance_dir, instance_dir);
    assert!(paths.data_dir.exists());

    // Create instance state
    let mut state = katana_hypervisor::instance::state::InstanceState::new(
        instance_id.to_string(),
        "test-instance".to_string(),
        config,
    );

    // Save to database
    db.save_instance(&state).unwrap();
    db.allocate_port(instance_id, port, "rpc").unwrap();

    // Verify saved
    let loaded_state = db.get_instance("test-instance").unwrap();
    assert_eq!(loaded_state.name, "test-instance");
    assert_eq!(loaded_state.config.rpc_port, port);

    // Update status to running
    state.update_status(InstanceStatus::Running);
    state.vm_pid = Some(12345);
    db.save_instance(&state).unwrap();

    // Verify update
    let updated_state = db.get_instance("test-instance").unwrap();
    assert!(matches!(updated_state.status, InstanceStatus::Running));
    assert_eq!(updated_state.vm_pid, Some(12345));

    // Clean up
    storage.delete_instance_storage(instance_id).unwrap();
    db.delete_instance("test-instance").unwrap();

    // Verify deleted
    assert!(!instance_dir.exists());
    assert!(db.get_instance("test-instance").is_err());
}

#[test]
fn test_multiple_instances_isolation() {
    let (db, storage, port_allocator, _state_temp, _storage_temp) = create_test_environment();

    let base_port = 55000u16;

    // Create first instance
    let port1 = port_allocator.allocate_port(base_port).unwrap();
    let config1 = create_test_config(port1);
    let instance_id1 = "instance-1";
    let dir1 = storage.create_instance_storage(instance_id1, 1_000_000_000).unwrap();

    let state1 = katana_hypervisor::instance::state::InstanceState::new(
        instance_id1.to_string(),
        "instance-1".to_string(),
        config1,
    );
    db.save_instance(&state1).unwrap();
    db.allocate_port(instance_id1, port1, "rpc").unwrap();

    // Create second instance
    let port2 = port_allocator.allocate_port(base_port).unwrap();
    assert_ne!(port1, port2); // Ports should be different

    let config2 = create_test_config(port2);
    let instance_id2 = "instance-2";
    let dir2 = storage.create_instance_storage(instance_id2, 2_000_000_000).unwrap();

    let state2 = katana_hypervisor::instance::state::InstanceState::new(
        instance_id2.to_string(),
        "instance-2".to_string(),
        config2,
    );
    db.save_instance(&state2).unwrap();
    db.allocate_port(instance_id2, port2, "rpc").unwrap();

    // Verify isolation
    assert_ne!(dir1, dir2);

    let loaded1 = db.get_instance("instance-1").unwrap();
    let loaded2 = db.get_instance("instance-2").unwrap();

    assert_eq!(loaded1.config.rpc_port, port1);
    assert_eq!(loaded2.config.rpc_port, port2);
    assert_eq!(loaded1.config.memory_mb, 2048);
    assert_eq!(loaded2.config.memory_mb, 2048);

    // Write to first instance storage
    std::fs::write(dir1.join("test.txt"), "instance1").unwrap();

    // Verify second instance doesn't have the file
    assert!(!dir2.join("test.txt").exists());

    // List instances
    let instances = db.list_instances().unwrap();
    assert_eq!(instances.len(), 2);

    // Clean up
    storage.delete_instance_storage(instance_id1).unwrap();
    storage.delete_instance_storage(instance_id2).unwrap();
    db.delete_instance("instance-1").unwrap();
    db.delete_instance("instance-2").unwrap();
}

#[test]
fn test_port_reallocation_after_delete() {
    let (db, _storage, port_allocator, _state_temp, _storage_temp) = create_test_environment();

    let base_port = 55000u16;

    // Create first instance
    let state1 = katana_hypervisor::instance::state::InstanceState::new(
        "instance-1".to_string(),
        "instance-1".to_string(),
        create_test_config(base_port),
    );
    db.save_instance(&state1).unwrap();
    let port1 = port_allocator.allocate_port(base_port).unwrap();
    db.allocate_port(&state1.id, port1, "rpc").unwrap();
    assert_eq!(port1, base_port);

    // Create second instance
    let state2 = katana_hypervisor::instance::state::InstanceState::new(
        "instance-2".to_string(),
        "instance-2".to_string(),
        create_test_config(base_port),
    );
    db.save_instance(&state2).unwrap();
    let port2 = port_allocator.allocate_port(base_port).unwrap();
    db.allocate_port(&state2.id, port2, "rpc").unwrap();
    assert_eq!(port2, base_port + 1);

    // Create third instance
    let state3 = katana_hypervisor::instance::state::InstanceState::new(
        "instance-3".to_string(),
        "instance-3".to_string(),
        create_test_config(base_port),
    );
    db.save_instance(&state3).unwrap();
    let port3 = port_allocator.allocate_port(base_port).unwrap();
    db.allocate_port(&state3.id, port3, "rpc").unwrap();
    assert_eq!(port3, base_port + 2);

    // Delete middle instance
    db.delete_instance("instance-2").unwrap();

    // Allocate new port - should reuse the gap
    let port4 = port_allocator.allocate_port(base_port).unwrap();
    assert_eq!(port4, base_port + 1); // Gap filled

    // Clean up
    db.delete_instance("instance-1").unwrap();
    db.delete_instance("instance-3").unwrap();
}

#[test]
fn test_storage_quota_enforcement() {
    let (db, storage, port_allocator, _state_temp, _storage_temp) = create_test_environment();

    let port = port_allocator.allocate_port(5050).unwrap();
    let config = create_test_config(port);

    let instance_id = "quota-test";
    let quota = 1000u64; // 1KB quota

    // Create storage with small quota
    let instance_dir = storage.create_instance_storage(instance_id, quota).unwrap();

    let state = katana_hypervisor::instance::state::InstanceState::new(
        instance_id.to_string(),
        "quota-test".to_string(),
        config,
    );
    db.save_instance(&state).unwrap();

    // Write small file - should be under quota
    std::fs::write(instance_dir.join("small.txt"), "hello").unwrap();
    storage.check_quota(instance_id, quota).unwrap();

    // Write large file - should exceed quota
    std::fs::write(instance_dir.join("large.txt"), vec![0u8; 2000]).unwrap();
    let result = storage.check_quota(instance_id, quota);
    assert!(result.is_err());

    // Clean up
    storage.delete_instance_storage(instance_id).unwrap();
    db.delete_instance("quota-test").unwrap();
}

#[test]
fn test_qemu_config_generation_non_tee() {
    let (_db, storage, _port_allocator, _state_temp, _storage_temp) = create_test_environment();

    let instance_id = "config-test";
    storage.create_instance_storage(instance_id, 1_000_000_000).unwrap();
    let paths = storage.get_paths(instance_id);

    let config = QemuConfig {
        memory_mb: 4096,
        vcpus: 4,
        cpu_type: "host".to_string(),
        kernel_path: PathBuf::from("/tmp/vmlinuz"),
        initrd_path: PathBuf::from("/tmp/initrd.img"),
        bios_path: None,
        kernel_cmdline: "console=ttyS0 katana.args=--dev".to_string(),
        rpc_port: 5050,
        qmp_socket: paths.qmp_socket.clone(),
        serial_log: paths.serial_log.clone(),
        pid_file: paths.pid_file.clone(),
        sev_snp: None,
        enable_kvm: true,
    };

    let args = config.to_qemu_args();

    // Verify essential args
    assert!(args.contains(&"qemu-system-x86_64".to_string()));
    assert!(args.contains(&"-enable-kvm".to_string()));
    assert!(args.contains(&"-cpu".to_string()));
    assert!(args.contains(&"host".to_string()));
    assert!(args.contains(&"-smp".to_string()));
    assert!(args.contains(&"4".to_string()));
    assert!(args.contains(&"-m".to_string()));
    assert!(args.contains(&"4096M".to_string()));
    assert!(args.contains(&"-kernel".to_string()));
    assert!(args.contains(&"-initrd".to_string()));
    assert!(args.contains(&"-append".to_string()));
    assert!(args.contains(&"console=ttyS0 katana.args=--dev".to_string()));
    assert!(args.contains(&"-netdev".to_string()));
    assert!(args.contains(&"user,id=net0,hostfwd=tcp::5050-:5050".to_string()));

    // Clean up
    storage.delete_instance_storage(instance_id).unwrap();
}

#[test]
fn test_state_persistence_across_operations() {
    let (db, storage, _port_allocator, _state_temp, _storage_temp) = create_test_environment();

    let instance_id = "persist-test";
    let config = create_test_config(5050);

    storage.create_instance_storage(instance_id, 1_000_000_000).unwrap();

    let mut state = katana_hypervisor::instance::state::InstanceState::new(
        instance_id.to_string(),
        "persist-test".to_string(),
        config,
    );

    // Save initial state
    db.save_instance(&state).unwrap();

    // Simulate state transitions
    state.update_status(InstanceStatus::Starting);
    db.save_instance(&state).unwrap();

    let loaded = db.get_instance("persist-test").unwrap();
    assert!(matches!(loaded.status, InstanceStatus::Starting));

    state.update_status(InstanceStatus::Running);
    state.vm_pid = Some(99999);
    state.qmp_socket = Some(PathBuf::from("/tmp/qmp.sock"));
    state.serial_log = Some(PathBuf::from("/tmp/serial.log"));
    db.save_instance(&state).unwrap();

    let loaded = db.get_instance("persist-test").unwrap();
    assert!(matches!(loaded.status, InstanceStatus::Running));
    assert_eq!(loaded.vm_pid, Some(99999));
    assert_eq!(loaded.qmp_socket, Some(PathBuf::from("/tmp/qmp.sock")));

    state.update_status(InstanceStatus::Stopping);
    db.save_instance(&state).unwrap();

    let loaded = db.get_instance("persist-test").unwrap();
    assert!(matches!(loaded.status, InstanceStatus::Stopping));

    state.update_status(InstanceStatus::Stopped);
    state.vm_pid = None;
    db.save_instance(&state).unwrap();

    let loaded = db.get_instance("persist-test").unwrap();
    assert!(matches!(loaded.status, InstanceStatus::Stopped));
    assert_eq!(loaded.vm_pid, None);

    // Clean up
    storage.delete_instance_storage(instance_id).unwrap();
    db.delete_instance("persist-test").unwrap();
}

#[test]
fn test_error_state_handling() {
    let (db, storage, _port_allocator, _state_temp, _storage_temp) = create_test_environment();

    let instance_id = "error-test";
    let config = create_test_config(5050);

    storage.create_instance_storage(instance_id, 1_000_000_000).unwrap();

    let mut state = katana_hypervisor::instance::state::InstanceState::new(
        instance_id.to_string(),
        "error-test".to_string(),
        config,
    );

    db.save_instance(&state).unwrap();

    // Transition to failed state
    state.update_status(InstanceStatus::Failed {
        error: "QEMU process crashed".to_string(),
    });
    db.save_instance(&state).unwrap();

    let loaded = db.get_instance("error-test").unwrap();
    match loaded.status {
        InstanceStatus::Failed { error } => {
            assert_eq!(error, "QEMU process crashed");
        }
        _ => panic!("Expected Failed status"),
    }

    // Clean up
    storage.delete_instance_storage(instance_id).unwrap();
    db.delete_instance("error-test").unwrap();
}

#[test]
fn test_concurrent_instance_creation() {
    let (db, storage, port_allocator, _state_temp, _storage_temp) = create_test_environment();

    let base_port = 55000u16;

    // Create 5 instances rapidly
    let mut ports = Vec::new();
    let mut instance_ids = Vec::new();

    for i in 0..5 {
        let port = port_allocator.allocate_port(base_port).unwrap();
        ports.push(port);

        let instance_id = format!("instance-{}", i);
        let config = create_test_config(port);

        storage.create_instance_storage(&instance_id, 1_000_000_000).unwrap();

        let state = katana_hypervisor::instance::state::InstanceState::new(
            instance_id.clone(),
            format!("instance-{}", i),
            config,
        );

        db.save_instance(&state).unwrap();
        db.allocate_port(&instance_id, port, "rpc").unwrap();

        instance_ids.push(instance_id);
    }

    // Verify all ports are unique and sequential
    assert_eq!(ports.len(), 5);
    for i in 0..5 {
        assert_eq!(ports[i], base_port + i as u16);
    }

    // Verify all instances exist
    let instances = db.list_instances().unwrap();
    assert_eq!(instances.len(), 5);

    // Clean up
    for instance_id in instance_ids {
        storage.delete_instance_storage(&instance_id).unwrap();
        db.delete_instance(&format!("instance-{}", instance_id.chars().last().unwrap())).unwrap();
    }
}
