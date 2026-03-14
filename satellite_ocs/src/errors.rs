//thiserror-based error types (optional) 
use thiserror::Error;
// Define a custom error type for the OCS system
#[derive(Error, Debug)]
pub enum OcsError {
    // Handles standard input/output errors such as
    // file reading, file writing, or network operations.
    #[error("Input/Output error: {0}")] Io(#[from] std::io::Error),
    // Handles errors related to the communication protocol
    // such as invalid messages, incorrect packet format, etc.
    #[error("Protocol error: {0}")] Protocol(String),
    // Handles any other errors that may occur
    #[error("Other error: {0}")] Other(String),
}
