//thiserror-based error types (optional) 
use thiserror::Error;

#[derive(Error, Debug)]
pub enum OcsError {
    #[error("IO: {0}")] Io(#[from] std::io::Error),
    #[error("Protocol: {0}")] Protocol(String),
    #[error("Other: {0}")] Other(String),
}
