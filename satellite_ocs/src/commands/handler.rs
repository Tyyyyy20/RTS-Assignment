use crate::{config::Config, crypto::Crypto, logging, net::framing::Framer};
use chrono::Utc;
use shared_protocol::{CommandAcknowledgment, CommunicationPacket, PacketPayload, Source};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::time::{Duration, Instant};
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
                                    let recv_instant = Instant::now();
                                    let exec_timestamp = Utc::now();

                                    let cmd_type_str = format!("{:?}", cmd.command_type);
                                    let target_str = format!("{:?}", cmd.target_system);

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
                                        execution_timestamp: Some(exec_timestamp),
                                        completion_timestamp: None,
                                        error_message: None,
                                        execution_time_ms: 0.0,
                                    };
                                    if let Err(e) = send_ack(tx_sock.as_ref(), &crypto, ack_recv).await {
                                        warn!(?e, "failed to send 'received' ack");
                                    }

                                    // Check if target system is currently under fault
                                    let target_lower = target_str.to_lowercase();
                                    let is_faulted = crate::faults::is_system_faulted(&target_lower);

                                    if is_faulted {
                                        // Reject: safety interlock — target system under active fault
                                        let exec_ms = recv_instant.elapsed().as_secs_f64() * 1000.0;
                                        let reason = format!(
                                            "rejected: target '{}' under active fault (safety interlock)",
                                            target_str
                                        );
                                        warn!(
                                            cmd_id = %cmd.command_id,
                                            %reason,
                                            "command rejected by safety interlock"
                                        );

                                        let ack_reject = CommandAcknowledgment {
                                            command_id: cmd.command_id.clone(),
                                            status: "rejected".into(),
                                            execution_timestamp: Some(exec_timestamp),
                                            completion_timestamp: Some(Utc::now()),
                                            error_message: Some(reason),
                                            execution_time_ms: exec_ms,
                                        };
                                        let _ = send_ack(tx_sock.as_ref(), &crypto, ack_reject).await;

                                        logging::csv::log_command(
                                            &cmd.command_id,
                                            &cmd_type_str,
                                            &target_str,
                                            exec_ms,
                                            "rejected",
                                        ).await;
                                        continue;
                                    }

                                    // Simulate command execution (~5ms of work)
                                    tokio::time::sleep(Duration::from_millis(5)).await;

                                    let exec_ms = recv_instant.elapsed().as_secs_f64() * 1000.0;

                                    // ACK: completed
                                    let ack_done = CommandAcknowledgment {
                                        command_id: cmd.command_id.clone(),
                                        status: "completed".into(),
                                        execution_timestamp: Some(exec_timestamp),
                                        completion_timestamp: Some(Utc::now()),
                                        error_message: None,
                                        execution_time_ms: exec_ms,
                                    };
                                    if let Err(e) = send_ack(tx_sock.as_ref(), &crypto, ack_done).await {
                                        warn!(?e, "failed to send 'completed' ack");
                                    }

                                    info!(
                                        cmd_id = %cmd.command_id,
                                        execution_time_ms = format_args!("{:.3}", exec_ms),
                                        "command completed"
                                    );

                                    // Log command-to-response latency
                                    logging::csv::log_command(
                                        &cmd.command_id,
                                        &cmd_type_str,
                                        &target_str,
                                        exec_ms,
                                        "completed",
                                    ).await;
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
