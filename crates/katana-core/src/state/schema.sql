CREATE TABLE IF NOT EXISTS instances (
    id TEXT PRIMARY KEY,
    name TEXT UNIQUE NOT NULL,
    status TEXT NOT NULL,
    config_json TEXT NOT NULL,
    vm_pid INTEGER,
    qmp_socket TEXT,
    serial_log TEXT,
    tee_mode BOOLEAN NOT NULL,
    expected_measurement TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS ports (
    port INTEGER PRIMARY KEY,
    instance_id TEXT NOT NULL,
    port_type TEXT NOT NULL,
    FOREIGN KEY (instance_id) REFERENCES instances(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS boot_components (
    instance_id TEXT NOT NULL,
    component_type TEXT NOT NULL,
    file_path TEXT NOT NULL,
    sha256_hash TEXT NOT NULL,
    PRIMARY KEY (instance_id, component_type),
    FOREIGN KEY (instance_id) REFERENCES instances(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_instances_status ON instances(status);
CREATE INDEX IF NOT EXISTS idx_ports_instance ON ports(instance_id);
