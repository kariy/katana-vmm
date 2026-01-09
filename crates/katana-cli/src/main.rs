use anyhow::Result;
use clap::{Parser, Subcommand};

mod commands;
mod config;
mod format;

use config::CliConfig;
use katana_client::Client;

#[derive(Parser)]
#[command(name = "katana-cli")]
#[command(about = "Katana Hypervisor CLI - Manage VM instances", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Output format (table or json)
    #[arg(long, global = true)]
    format: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a new instance
    Create {
        /// Instance name
        name: String,
        /// Number of vCPUs
        #[arg(long, default_value = "2")]
        vcpus: u32,
        /// Memory size (e.g., "4G", "2048M")
        #[arg(long, default_value = "2G")]
        memory: String,
        /// Storage size (e.g., "10G", "5120M")
        #[arg(long, default_value = "10G")]
        storage: String,
        /// RPC port (auto-allocated if not specified)
        #[arg(long)]
        port: Option<u16>,
        /// Enable development mode
        #[arg(long, default_value = "true")]
        dev: bool,
        /// Enable TEE mode
        #[arg(long)]
        tee: bool,
    },
    /// Start an instance
    Start {
        /// Instance name
        name: String,
    },
    /// Stop an instance
    Stop {
        /// Instance name
        name: String,
    },
    /// Delete an instance
    Delete {
        /// Instance name
        name: String,
    },
    /// List all instances
    List,
    /// Show instance details
    Show {
        /// Instance name
        name: String,
    },
    /// View instance logs
    Logs {
        /// Instance name
        name: String,
        /// Number of lines to show
        #[arg(long, short = 'n')]
        tail: Option<usize>,
        /// Stream logs in real-time (like tail -f)
        #[arg(long, short = 'f')]
        follow: bool,
    },
    /// Show instance statistics
    Stats {
        /// Instance name
        name: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Load configuration
    let config = CliConfig::load()?;

    // Create client
    let client = Client::new(&config.socket);

    // Determine output format
    let output_format = if let Some(fmt) = cli.format {
        match fmt.to_lowercase().as_str() {
            "json" => config::OutputFormat::Json,
            "table" => config::OutputFormat::Table,
            _ => {
                eprintln!("Invalid format '{}', using default", fmt);
                config.format
            }
        }
    } else {
        config.format
    };

    // Execute command
    match cli.command {
        Commands::Create {
            name,
            vcpus,
            memory,
            storage,
            port,
            dev,
            tee,
        } => {
            commands::create::execute(
                &client,
                name,
                vcpus,
                memory,
                storage,
                port,
                dev,
                tee,
                &output_format,
            )
            .await?
        }
        Commands::Start { name } => commands::start::execute(&client, name, &output_format).await?,
        Commands::Stop { name } => commands::stop::execute(&client, name, &output_format).await?,
        Commands::Delete { name } => commands::delete::execute(&client, name).await?,
        Commands::List => commands::list::execute(&client, &output_format).await?,
        Commands::Show { name } => commands::show::execute(&client, name, &output_format).await?,
        Commands::Logs { name, tail, follow } => {
            commands::logs::execute(&client, name, tail, follow).await?
        }
        Commands::Stats { name } => commands::stats::execute(&client, name, &output_format).await?,
    }

    Ok(())
}
