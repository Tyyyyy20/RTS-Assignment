use crate::{config::Config, crypto::Crypto, net::framing::Framer};
use chrono::Utc;
use shared_protocol::{CommandAcknowledgment, CommunicationPacket, PacketPayload, Source};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tracing::{info, warn};

/// Spawns an asynchronous receiver task that listens for incoming UDP packets
/// from the Ground Control Station (GCS).
///
/// The receiver performs the following pipeline:
/// 1. Receive UDP packet
/// 2. Deframe the packet into a protocol frame
/// 3. Decrypt the frame to obtain a CommunicationPacket
/// 4. Process command payloads
/// 5. Send an acknowledgment (ACK) back to the ground station
///
/// This function runs continuously as part of the satellite command handling system.

pub async fn spawn_receiver(
    _cfg: Config,
    crypto: Crypto,
    rx_sock: Arc<UdpSocket>,
    tx_sock: Arc<UdpSocket>,
    framer: Framer,
) {
    tokio::spawn(async move {
        // Buffer used to store incoming UDP packets (max 64KB)
        let mut buf = vec![0u8; 64 * 1024];
        let framer = framer; // move into task

        // Main receiver loop that continuously listens for packets from GCS
        loop {
            match rx_sock.recv_from(&mut buf).await {
                Ok((n, _from)) => {
                    // Remove transport framing and extract a valid protocol frame
                    match framer.deframe(&buf[..n]) {
                        // Decrypt communication packet
                        Ok(frame) => match crypto.open(frame) {
                            // Process command payload
                            Ok(pkt) => match pkt.payload {
                                PacketPayload::CommandData(cmd) => {
                                    info!(
                                        cmd_id = %cmd.command_id,
                                        ?cmd.command_type,
                                        ?cmd.target_system,
                                        "Satellite received command from ground station"
                                    );

                                    // ACK: received
                                    let ack_recv = CommandAcknowledgment {
                                        command_id: cmd.command_id.clone(),
                                        status: "received".into(),
                                        execution_timestamp: Some(Utc::now()),
                                        completion_timestamp: None,
                                        error_message: None,
                                        execution_time_ms: 0.0,
                                    };
                                    if let Err(e) = send_ack(tx_sock.as_ref(), &crypto, ack_recv).await {
                                        warn!(?e, "failed to send 'received' ack");
                                    }

                                    // TODO: schedule/execute → send 'executing' and 'completed' ACKs
                                }
                                _other => {
                                    // warn!("Satellite received non-command payload from ground station");
                                }
                            },
                            Err(e) => warn!("decrypt failed: {e}"),
                        },
                        Err(e) => warn!("deframe failed: {e}"),
                    }
                }
                Err(e) => warn!("receive failed: {e}"),
            }
        }
    });
}


/// Steps:
/// 1. Create a CommunicationPacket containing the ACK
/// 2. Encrypt the packet using the Crypto module
/// 3. Send the encrypted packet via UDP
async fn send_ack(
    sock: &UdpSocket,
    crypto: &Crypto,
    ack: CommandAcknowledgment,
) -> Result<(), std::io::Error> {
    let pkt = CommunicationPacket::new_ack(ack, Source::Satellite);
    if let Ok(bytes) = crypto.seal(&pkt) {
        sock.send(&bytes).await?;
    }
    Ok(())
}
