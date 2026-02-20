//Command line interface 
// runtime configuration (ports, keys, rates)
use anyhow::Result;
use clap::Parser;

#[derive(Debug, Clone)]
pub struct Config {
    pub gcs_addr: String,
    pub bind_addr: String,
    pub key_id: u8,
    pub key_hex: String,
    pub batch_ms: u64,
    pub max_batch: usize,
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
    pub fn parse_and_build_config() -> Result<Config> {
        let c = <Cli as Parser>::parse();
        Ok(Config {
            gcs_addr: c.gcs_addr,
            bind_addr: c.bind_addr,
            key_id: c.key_id,
            key_hex: c.key_hex,
            batch_ms: c.batch_ms,
            max_batch: c.max_batch,
        })
    }
}
