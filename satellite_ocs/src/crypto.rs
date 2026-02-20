// src/crypto.rs (recap)
use std::sync::Arc;
use anyhow::{bail, Result};
use shared_protocol::{CommunicationPacket, CryptoContext};
use crate::config::Config;

pub struct Crypto {
    ctx: Arc<CryptoContext>,
    key_id: u8,
}

impl Crypto {
    pub fn from_config(cfg: &Config) -> Result<Self> {
        let bytes = hex::decode(&cfg.key_hex)
            .map_err(|e| anyhow::anyhow!("invalid key_hex: {e}"))?;
        if bytes.len() != 32 { bail!("key_hex must be 64 hex chars"); }
        let mut key = [0u8; 32]; key.copy_from_slice(&bytes);
        Ok(Self { ctx: Arc::new(CryptoContext::new(cfg.key_id, key)), key_id: cfg.key_id })
    }
    #[inline] pub fn seal(&self, pkt: &CommunicationPacket) -> Result<Vec<u8>, String> {
        self.ctx.seal_to_bytes(pkt)
    }
    #[inline] pub fn open(&self, frame: &[u8]) -> Result<CommunicationPacket, String> {
        self.ctx.open_from_bytes(frame)
    }
}
impl Clone for Crypto {
    fn clone(&self) -> Self { Self { ctx: self.ctx.clone(), key_id: self.key_id } }
}
