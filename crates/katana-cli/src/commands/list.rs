use anyhow::Result;

use crate::{config::OutputFormat, format};
use katana_client::Client;

pub async fn execute(client: &Client, output_format: &OutputFormat) -> Result<()> {
    let response = client.list_instances().await?;

    match output_format {
        OutputFormat::Json => {
            let json_value = serde_json::to_value(&response)?;
            format::print_json(&json_value);
        }
        OutputFormat::Table => {
            format::print_instance_list(&response.instances);
        }
    }

    Ok(())
}
