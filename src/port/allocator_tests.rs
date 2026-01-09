#[cfg(test)]
mod tests {
    use super::super::PortAllocator;
    use crate::instance::{InstanceConfig, InstanceState};
    use crate::state::StateDatabase;
    use tempfile::TempDir;

    fn create_test_setup() -> (PortAllocator, StateDatabase, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = StateDatabase::new(&db_path).unwrap();
        let allocator = PortAllocator::new(db.clone());
        (allocator, db, temp_dir)
    }

    fn create_test_instance(name: &str) -> InstanceState {
        let config = InstanceConfig {
            vcpus: 4,
            memory_mb: 4096,
            storage_bytes: 10 * 1024 * 1024 * 1024,
            quota_project_id: None,
            rpc_port: 5050,
            metrics_port: None,
            tee_mode: false,
            vcpu_type: "host".to_string(),
            expected_measurement: None,
            kernel_path: "/tmp/vmlinuz".into(),
            initrd_path: "/tmp/initrd.img".into(),
            ovmf_path: None,
            data_dir: "/tmp/data".into(),
            chain_id: None,
            dev_mode: true,
            block_time: None,
            accounts: Some(10),
            disable_fee: true,
            extra_args: vec![],
        };

        // Use unique ID based on name for testing
        let id = format!("test-id-{}", name);
        InstanceState::new(id, name.to_string(), config)
    }

    #[test]
    fn test_allocate_first_port() {
        let (allocator, _db, _temp) = create_test_setup();

        let port = allocator.allocate_port(5050).unwrap();
        assert_eq!(port, 5050);
    }

    #[test]
    fn test_allocate_sequential_ports() {
        let (allocator, db, _temp) = create_test_setup();

        // Create instances first
        let instance1 = create_test_instance("instance1");
        let instance2 = create_test_instance("instance2");
        db.save_instance(&instance1).unwrap();
        db.save_instance(&instance2).unwrap();

        // Allocate first port
        let port1 = allocator.allocate_port(5050).unwrap();
        db.allocate_port(&instance1.id, port1, "rpc").unwrap();

        // Allocate second port (should be 5051)
        let port2 = allocator.allocate_port(5050).unwrap();
        assert_eq!(port2, 5051);

        // Allocate third port (should be 5052)
        db.allocate_port(&instance2.id, port2, "rpc").unwrap();
        let port3 = allocator.allocate_port(5050).unwrap();
        assert_eq!(port3, 5052);
    }

    #[test]
    fn test_port_reuse_after_release() {
        let (allocator, db, _temp) = create_test_setup();

        // Create instances first
        let instance1 = create_test_instance("instance1");
        let instance2 = create_test_instance("instance2");
        let instance3 = create_test_instance("instance3");
        db.save_instance(&instance1).unwrap();
        db.save_instance(&instance2).unwrap();
        db.save_instance(&instance3).unwrap();

        // Allocate ports
        let port1 = allocator.allocate_port(5050).unwrap();
        db.allocate_port(&instance1.id, port1, "rpc").unwrap();

        let port2 = allocator.allocate_port(5050).unwrap();
        db.allocate_port(&instance2.id, port2, "rpc").unwrap();

        let port3 = allocator.allocate_port(5050).unwrap();
        db.allocate_port(&instance3.id, port3, "rpc").unwrap();

        // Now we have 5050, 5051, 5052 allocated
        assert_eq!(port1, 5050);
        assert_eq!(port2, 5051);
        assert_eq!(port3, 5052);

        // Delete instance2 (releases port 5051)
        db.delete_instance("instance2").unwrap();

        // Next allocation should reuse 5051
        let port4 = allocator.allocate_port(5050).unwrap();
        assert_eq!(port4, 5051);
    }

    #[test]
    fn test_is_port_available() {
        let (allocator, db, _temp) = create_test_setup();

        // Create instance first
        let instance1 = create_test_instance("instance1");
        db.save_instance(&instance1).unwrap();

        // Port should be available initially
        assert!(allocator.is_port_available(5050).unwrap());

        // Allocate port
        db.allocate_port(&instance1.id, 5050, "rpc").unwrap();

        // Port should not be available now
        assert!(!allocator.is_port_available(5050).unwrap());

        // Different port should still be available
        assert!(allocator.is_port_available(5051).unwrap());
    }

    #[test]
    fn test_allocate_custom_base_port() {
        let (allocator, _db, _temp) = create_test_setup();

        // Allocate from custom base
        let port = allocator.allocate_port(8080).unwrap();
        assert_eq!(port, 8080);
    }

    #[test]
    fn test_multiple_instances_same_base() {
        let (allocator, db, _temp) = create_test_setup();

        // Use a high port number unlikely to be in use
        let base_port = 55000u16;

        let mut ports = vec![];
        for i in 0..10 {
            // Create instance first
            let instance = create_test_instance(&format!("instance{}", i));
            db.save_instance(&instance).unwrap();

            let port = allocator.allocate_port(base_port).unwrap();
            db.allocate_port(&instance.id, port, "rpc")
                .unwrap();
            ports.push(port);
        }

        // All ports should be unique and sequential
        for i in 0..10 {
            assert_eq!(ports[i], base_port + i as u16);
        }
    }

    #[test]
    fn test_port_allocation_with_gaps() {
        let (allocator, db, _temp) = create_test_setup();

        // Create instances first
        let instance1 = create_test_instance("instance1");
        let instance2 = create_test_instance("instance2");
        let instance3 = create_test_instance("instance3");
        db.save_instance(&instance1).unwrap();
        db.save_instance(&instance2).unwrap();
        db.save_instance(&instance3).unwrap();

        // Allocate 5050, 5051, 5052
        db.allocate_port(&instance1.id, 5050, "rpc").unwrap();
        db.allocate_port(&instance2.id, 5051, "rpc").unwrap();
        db.allocate_port(&instance3.id, 5052, "rpc").unwrap();

        // Delete middle instance (5051)
        db.delete_instance("instance2").unwrap();

        // Next allocation should fill the gap
        let port = allocator.allocate_port(5050).unwrap();
        assert_eq!(port, 5051);
    }
}
