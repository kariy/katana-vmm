use anyhow::Result;

use crate::{config::OutputFormat, format};
use katana_client::Client;

pub async fn execute(client: &Client, name: String, output_format: &OutputFormat) -> Result<()> {
    // Fetch stats from daemon
    let response = client.get_stats(&name).await?;

    match output_format {
        OutputFormat::Json => {
            // JSON output
            let json_value = serde_json::to_value(&response)?;
            format::print_json(&json_value);
        }
        OutputFormat::Table => {
            // Pretty table output
            println!("===========================================");
            println!(" Instance Statistics: {}", response.instance_name);
            println!("===========================================");
            println!();
            println!("Status:");
            println!("  State:       {}", response.status.state);
            println!("  Running:     {}", response.status.running);
            println!(
                "  PID:         {}",
                response
                    .status
                    .pid
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "N/A".to_string())
            );
            println!("  Uptime:      {}", response.status.uptime);
            println!();
            println!("Configuration:");
            println!("  vCPUs:       {}", response.config.vcpus);
            println!("  Memory:      {} MB", response.config.memory_mb);
            println!("  RPC Port:    {}", response.config.rpc_port);
            if let Some(tee) = &response.config.tee_mode {
                println!("  TEE Mode:    {}", tee);
            }
            println!();
            println!("Resources:");
            println!("  CPU Count:   {}", response.resources.cpu_count);
            for cpu in &response.resources.cpus {
                println!("    CPU {}:     Thread ID {}", cpu.cpu_index, cpu.thread_id);
            }
            println!("  Memory:      {} MB", response.resources.memory_mb);
            println!();
            println!("Network:");
            println!("  RPC:         {}", response.network.rpc_url);
            println!("  Health:      {}", response.network.health_url);
            println!();
        }
    }

    Ok(())
}
