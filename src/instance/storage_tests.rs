#[cfg(test)]
mod tests {
    use super::super::StorageManager;
    use tempfile::TempDir;

    fn create_test_storage() -> (StorageManager, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let storage = StorageManager::new(temp_dir.path().to_path_buf());
        (storage, temp_dir)
    }

    #[test]
    fn test_create_instance_storage() {
        let (mut storage, _temp) = create_test_storage();

        let instance_dir = storage
            .create_instance_storage("test-instance-1", 10_000_000_000)
            .unwrap();

        assert!(instance_dir.exists());
        assert!(instance_dir.join("data").exists());
    }

    #[test]
    fn test_get_instance_dir() {
        let (storage, temp) = create_test_storage();

        let dir = storage.get_instance_dir("test-instance-1");
        assert_eq!(dir, temp.path().join("test-instance-1"));
    }

    #[test]
    fn test_get_paths() {
        let (storage, temp) = create_test_storage();

        let paths = storage.get_paths("test-instance-1");

        assert_eq!(paths.instance_dir, temp.path().join("test-instance-1"));
        assert_eq!(paths.data_dir, temp.path().join("test-instance-1").join("data"));
        assert_eq!(paths.serial_log, temp.path().join("test-instance-1").join("serial.log"));
        assert_eq!(paths.qmp_socket, temp.path().join("test-instance-1").join("qmp.sock"));
        assert_eq!(paths.pid_file, temp.path().join("test-instance-1").join("qemu.pid"));
    }

    #[test]
    fn test_get_disk_usage_empty_dir() {
        let (mut storage, _temp) = create_test_storage();

        storage
            .create_instance_storage("test-instance-1", 10_000_000_000)
            .unwrap();

        let usage = storage.get_disk_usage("test-instance-1").unwrap();
        assert_eq!(usage, 0); // Empty directory
    }

    #[test]
    fn test_get_disk_usage_with_files() {
        let (mut storage, _temp) = create_test_storage();

        let instance_dir = storage
            .create_instance_storage("test-instance-1", 10_000_000_000)
            .unwrap();

        // Write some test files
        std::fs::write(instance_dir.join("test1.txt"), "hello").unwrap();
        std::fs::write(instance_dir.join("data").join("test2.txt"), "world").unwrap();

        let usage = storage.get_disk_usage("test-instance-1").unwrap();
        assert_eq!(usage, 10); // 5 + 5 bytes
    }

    #[test]
    fn test_get_disk_usage_nonexistent() {
        let (storage, _temp) = create_test_storage();

        let usage = storage.get_disk_usage("nonexistent").unwrap();
        assert_eq!(usage, 0);
    }

    #[test]
    fn test_check_quota_under_limit() {
        let (mut storage, _temp) = create_test_storage();

        let instance_dir = storage
            .create_instance_storage("test-instance-1", 1000)
            .unwrap();

        std::fs::write(instance_dir.join("test.txt"), "hello").unwrap();

        // Should not error (5 bytes < 1000 bytes quota)
        storage.check_quota("test-instance-1", 1000).unwrap();
    }

    #[test]
    fn test_check_quota_exceeded() {
        let (mut storage, _temp) = create_test_storage();

        let instance_dir = storage
            .create_instance_storage("test-instance-1", 100)
            .unwrap();

        std::fs::write(instance_dir.join("test.txt"), &vec![0u8; 200]).unwrap();

        // Should error (200 bytes > 100 bytes quota)
        let result = storage.check_quota("test-instance-1", 100);
        assert!(result.is_err());
    }

    #[test]
    fn test_delete_instance_storage() {
        let (mut storage, _temp) = create_test_storage();

        let instance_dir = storage
            .create_instance_storage("test-instance-1", 10_000_000_000)
            .unwrap();

        assert!(instance_dir.exists());

        // Delete storage
        storage.delete_instance_storage("test-instance-1").unwrap();

        assert!(!instance_dir.exists());
    }

    #[test]
    fn test_delete_nonexistent_storage() {
        let (storage, _temp) = create_test_storage();

        // Should not error
        storage.delete_instance_storage("nonexistent").unwrap();
    }

    #[test]
    fn test_multiple_instances_isolation() {
        let (mut storage, _temp) = create_test_storage();

        // Create multiple instances
        let dir1 = storage
            .create_instance_storage("instance1", 1_000_000)
            .unwrap();
        let dir2 = storage
            .create_instance_storage("instance2", 2_000_000)
            .unwrap();

        assert_ne!(dir1, dir2);
        assert!(dir1.exists());
        assert!(dir2.exists());

        // Write to instance1
        std::fs::write(dir1.join("test.txt"), "instance1").unwrap();

        // Verify instance2 doesn't have the file
        assert!(!dir2.join("test.txt").exists());
    }
}
