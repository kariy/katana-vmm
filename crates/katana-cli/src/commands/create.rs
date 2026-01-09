use anyhow::Result;

use crate::{config::OutputFormat, format};
use katana_client::Client;
use katana_models::CreateInstanceRequest;

pub async fn execute(
    client: &Client,
    name: String,
    vcpus: u32,
    memory: String,
    storage: String,
    port: Option<u16>,
    dev: bool,
    tee: bool,
    output_format: &OutputFormat,
) -> Result<()> {
    let request = CreateInstanceRequest {
        name,
        vcpus,
        memory,
        storage,
        port,
        dev,
        tee,
        vcpu_type: "host".to_string(),
        chain_id: None,
        block_time: None,
        accounts: None,
        disable_fee: false,
        extra_args: vec![],
    };

    let response = client.create_instance(request).await?;

    match output_format {
        OutputFormat::Json => {
            let json_value = serde_json::to_value(&response)?;
            format::print_json(&json_value);
        }
        OutputFormat::Table => {
            format::print_instance_details(&response);
            println!("\n✓ Instance created successfully!");
        }
    }

    Ok(())
}
