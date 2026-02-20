use crate::{config::Config, crypto::Crypto, net::framing::Framer};
use chrono::Utc;
use shared_protocol::{CommandAcknowledgment, CommunicationPacket, PacketPayload, Source};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tracing::{info, warn};

pub async fn spawn_receiver(
    _cfg: Config,
    crypto: Crypto,
    rx_sock: Arc<UdpSocket>,
    tx_sock: Arc<UdpSocket>,
    framer: Framer,
) {
    tokio::spawn(async move {
        let mut buf = vec![0u8; 64 * 1024];
        let framer = framer; // move into task

        loop {
            match rx_sock.recv_from(&mut buf).await {
                Ok((n, _from)) => {
                    match framer.deframe(&buf[..n]) {
                        Ok(frame) => match crypto.open(frame) {
                            Ok(pkt) => match pkt.payload {
                                PacketPayload::CommandData(cmd) => {
                                    info!(
                                        cmd_id = %cmd.command_id,
                                        ?cmd.command_type,
                                        ?cmd.target_system,
                                        "received command"
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
                                    // ignore non-command payloads for now
                                }
                            },
                            Err(e) => warn!("decrypt error: {e}"),
                        },
                        Err(e) => warn!("deframe error: {e}"),
                    }
                }
                Err(e) => warn!("recv error: {e}"),
            }
        }
    });
}

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
