#[cfg(test)]
mod tests {
    use super::super::StateDatabase;
    use crate::instance::{InstanceConfig, InstanceState, InstanceStatus};
    use tempfile::TempDir;

    fn create_test_db() -> (StateDatabase, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let db = StateDatabase::new(&db_path).unwrap();
        (db, temp_dir)
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
    fn test_create_database() {
        let (_db, _temp) = create_test_db();
        // Database creation should succeed
    }

    #[test]
    fn test_save_and_get_instance() {
        let (db, _temp) = create_test_db();
        let instance = create_test_instance("test1");

        // Save instance
        db.save_instance(&instance).unwrap();

        // Retrieve instance
        let retrieved = db.get_instance("test1").unwrap();

        assert_eq!(retrieved.name, "test1");
        assert_eq!(retrieved.id, instance.id);
        assert_eq!(retrieved.config.vcpus, 4);
        assert_eq!(retrieved.config.memory_mb, 4096);
    }

    #[test]
    fn test_get_nonexistent_instance() {
        let (db, _temp) = create_test_db();

        let result = db.get_instance("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_update_instance_status() {
        let (db, _temp) = create_test_db();
        let mut instance = create_test_instance("test1");

        // Save with Created status
        db.save_instance(&instance).unwrap();

        // Sleep for 1 second to ensure timestamp difference
        std::thread::sleep(std::time::Duration::from_secs(1));

        // Update to Running
        instance.update_status(InstanceStatus::Running);
        instance.vm_pid = Some(12345);
        db.save_instance(&instance).unwrap();

        // Retrieve and verify
        let retrieved = db.get_instance("test1").unwrap();
        assert!(matches!(retrieved.status, InstanceStatus::Running));
        assert_eq!(retrieved.vm_pid, Some(12345));
        assert!(retrieved.updated_at > retrieved.created_at);
    }

    #[test]
    fn test_list_instances() {
        let (db, _temp) = create_test_db();

        // Initially empty
        let instances = db.list_instances().unwrap();
        assert_eq!(instances.len(), 0);

        // Add instances
        let instance1 = create_test_instance("test1");
        let instance2 = create_test_instance("test2");
        let instance3 = create_test_instance("test3");

        db.save_instance(&instance1).unwrap();
        db.save_instance(&instance2).unwrap();
        db.save_instance(&instance3).unwrap();

        // List all
        let instances = db.list_instances().unwrap();
        assert_eq!(instances.len(), 3);

        let names: Vec<String> = instances.iter().map(|i| i.name.clone()).collect();
        assert!(names.contains(&"test1".to_string()));
        assert!(names.contains(&"test2".to_string()));
        assert!(names.contains(&"test3".to_string()));
    }

    #[test]
    fn test_delete_instance() {
        let (db, _temp) = create_test_db();
        let instance = create_test_instance("test1");

        db.save_instance(&instance).unwrap();

        // Delete
        db.delete_instance("test1").unwrap();

        // Verify deleted
        let result = db.get_instance("test1");
        assert!(result.is_err());
    }

    #[test]
    fn test_delete_nonexistent_instance() {
        let (db, _temp) = create_test_db();

        let result = db.delete_instance("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_instance_exists() {
        let (db, _temp) = create_test_db();
        let instance = create_test_instance("test1");

        // Should not exist initially
        assert!(!db.instance_exists("test1").unwrap());

        // Save
        db.save_instance(&instance).unwrap();

        // Should exist now
        assert!(db.instance_exists("test1").unwrap());

        // Delete
        db.delete_instance("test1").unwrap();

        // Should not exist again
        assert!(!db.instance_exists("test1").unwrap());
    }

    #[test]
    fn test_port_allocation() {
        let (db, _temp) = create_test_db();

        // Create instances first
        let instance1 = create_test_instance("instance1");
        let instance2 = create_test_instance("instance2");
        db.save_instance(&instance1).unwrap();
        db.save_instance(&instance2).unwrap();

        // Initially no ports allocated
        let ports = db.get_allocated_ports().unwrap();
        assert_eq!(ports.len(), 0);

        // Allocate ports
        db.allocate_port(&instance1.id, 5050, "rpc").unwrap();
        db.allocate_port(&instance1.id, 9090, "metrics").unwrap();
        db.allocate_port(&instance2.id, 5051, "rpc").unwrap();

        // Get allocated ports
        let ports = db.get_allocated_ports().unwrap();
        assert_eq!(ports.len(), 3);
        assert!(ports.contains(&5050));
        assert!(ports.contains(&9090));
        assert!(ports.contains(&5051));
    }

    #[test]
    fn test_port_cascade_delete() {
        let (db, _temp) = create_test_db();
        let instance = create_test_instance("test1");

        // Save instance and allocate ports
        db.save_instance(&instance).unwrap();
        db.allocate_port(&instance.id, 5050, "rpc").unwrap();
        db.allocate_port(&instance.id, 9090, "metrics").unwrap();

        // Verify ports allocated
        let ports = db.get_allocated_ports().unwrap();
        assert_eq!(ports.len(), 2);

        // Delete instance (should cascade delete ports)
        db.delete_instance("test1").unwrap();

        // Verify ports deleted
        let ports = db.get_allocated_ports().unwrap();
        assert_eq!(ports.len(), 0);
    }

    #[test]
    fn test_duplicate_instance_name() {
        let (db, _temp) = create_test_db();
        let instance1 = create_test_instance("test1");
        let mut instance2 = create_test_instance("test1");
        instance2.id = "different-id".to_string();

        // Save first instance
        db.save_instance(&instance1).unwrap();

        // Trying to save second with same name should fail
        let result = db.save_instance(&instance2);
        assert!(result.is_err());
    }

    #[test]
    fn test_get_instance_by_id() {
        let (db, _temp) = create_test_db();
        let instance = create_test_instance("test1");

        db.save_instance(&instance).unwrap();

        // Get by ID
        let retrieved = db.get_instance_by_id(&instance.id).unwrap();
        assert_eq!(retrieved.name, "test1");
        assert_eq!(retrieved.id, instance.id);

        // Get by non-existent ID
        let result = db.get_instance_by_id("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_instance_status_serialization() {
        let (db, _temp) = create_test_db();
        let mut instance = create_test_instance("test1");

        // Test different status values
        let statuses = vec![
            InstanceStatus::Created,
            InstanceStatus::Starting,
            InstanceStatus::Running,
            InstanceStatus::Stopping,
            InstanceStatus::Stopped,
            InstanceStatus::Failed {
                error: "test error".to_string(),
            },
        ];

        for status in statuses {
            instance.update_status(status.clone());
            db.save_instance(&instance).unwrap();

            let retrieved = db.get_instance("test1").unwrap();
            match (&status, &retrieved.status) {
                (InstanceStatus::Failed { error: e1 }, InstanceStatus::Failed { error: e2 }) => {
                    assert_eq!(e1, e2);
                }
                _ => assert!(std::mem::discriminant(&status) == std::mem::discriminant(&retrieved.status)),
            }
        }
    }
}
