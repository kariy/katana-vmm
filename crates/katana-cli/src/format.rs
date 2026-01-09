use comfy_table::{modifiers::UTF8_ROUND_CORNERS, presets::UTF8_FULL, Table};
use katana_models::InstanceResponse;
use serde_json::Value;

pub fn print_json(value: &Value) {
    println!("{}", serde_json::to_string_pretty(value).unwrap());
}

pub fn print_instance_list(instances: &[InstanceResponse]) {
    if instances.is_empty() {
        println!("No instances found.");
        return;
    }

    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS)
        .set_header(vec!["NAME", "STATUS", "VCPUS", "MEMORY", "RPC PORT"]);

    for instance in instances {
        table.add_row(vec![
            instance.name.clone(),
            instance.status.clone(),
            instance.config.vcpus.to_string(),
            format!("{} MB", instance.config.memory_mb),
            instance.config.rpc_port.to_string(),
        ]);
    }

    println!("{table}");
}

pub fn print_instance_details(instance: &InstanceResponse) {
    let storage_gb = instance.config.storage_bytes / 1_000_000_000;

    println!("Instance: {}", instance.name);
    println!("  ID:         {}", instance.id);
    println!("  Status:     {}", instance.status);
    println!("  vCPUs:      {}", instance.config.vcpus);
    println!("  Memory:     {} MB", instance.config.memory_mb);
    println!("  Storage:    {} GB", storage_gb);
    println!("  RPC Port:   {}", instance.config.rpc_port);
    println!(
        "  TEE Mode:   {}",
        if instance.config.tee_mode {
            "enabled"
        } else {
            "disabled"
        }
    );
    println!("  Created:    {}", instance.created_at);

    if let Some(endpoints) = &instance.endpoints {
        println!("\nEndpoints:");
        println!("  RPC: {}", endpoints.rpc);
        if let Some(metrics) = &endpoints.metrics {
            println!("  Metrics: {}", metrics);
        }
    }
}
