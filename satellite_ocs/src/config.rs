// Command line interface
// Defines runtime configuration such as network ports, encryption keys, and batching settings
use anyhow::Result;
use clap::Parser;

// Main runtime configuration used by the system
// Stores all parameters needed during program execution
#[derive(Debug, Clone)]
pub struct Config {
    pub gcs_addr: String,   // Address of the Ground Control Station (GCS)
    pub bind_addr: String,  // Local address where this node listens for incoming data
    pub key_id: u8,         // Identifier for the encryption key
    pub key_hex: String,    // Encryption key represented as a hexadecimal string
    pub batch_ms: u64,      // Telemetry batching interval in milliseconds
    pub max_batch: usize,   // Maximum number of messages allowed in one batch
}

#[derive(Parser, Debug, Clone)]
pub struct Cli {
    #[arg(long, default_value = "127.0.0.1:7891")] pub gcs_addr: String,
    #[arg(long, default_value = "0.0.0.0:7892")]   pub bind_addr: String,
    #[arg(long, default_value_t = 1)]              pub key_id: u8,
    #[arg(long, default_value = "0000000000000000000000000000000000000000000000000000000000000007")]
    pub key_hex: String,
    #[arg(long, default_value_t = 50)]             pub batch_ms: u64,
    #[arg(long, default_value_t = 64)]             pub max_batch: usize,
}

impl Cli {
    // Parses command line arguments and converts them into the runtime Config structure
    pub fn parse_and_build_config() -> Result<Config> {
        // Parse CLI arguments provided when the program starts
        let cli = Cli::parse();
        // Build the runtime configuration object
        Ok(Config {
            gcs_addr: cli.gcs_addr,
            bind_addr: cli.bind_addr,
            key_id: cli.key_id,
            key_hex: cli.key_hex,
            batch_ms: cli.batch_ms,
            max_batch: cli.max_batch,
        })
    }
}
