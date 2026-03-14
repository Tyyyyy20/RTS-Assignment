// This module handles the command and control (C2) functionality of the OCS.
// It is responsible for receiving commands from the ground control station
// and executing them on the satellite.
pub mod handler;
pub use handler::spawn_receiver;
