// src/network.rs

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket as TokioUdpSocket;
use tokio::time::timeout;
use tokio::sync::Mutex;
use tracing::{info, warn, error, debug};
use anyhow::{Result, Context};
use chrono::{DateTime, Utc};
use std::collections::{HashMap, VecDeque};

use shared_protocol::{
    CommunicationPacket,
    Command,
    CryptoContext,
    EncryptedFrame,
    SensorType,
    Source,
};

/// ---- match satellite defaults (same key_id/key_hex) ----
const KEY_ID: u8 = 1;
const KEY_HEX: &str =
    "0000000000000000000000000000000000000000000000000000000000000007";

/// Reception timing data for performance tracking
#[derive(Debug, Clone)]
pub struct ReceptionTiming {
    pub packet_id: String,
    pub packet_type: String, // stringified enum for logs / metadata
    pub reception_time: DateTime<Utc>,
    pub packet_timestamp: DateTime<Utc>,
    pub end_to_end_latency_ms: f64,
    pub reception_drift_ms: f64,
    pub jitter_ms: f64,
    pub is_delayed: bool,
    pub delay_severity: DriftSeverity,
    pub decode_time_ms: f64,
}

#[derive(Debug, Clone)]
pub enum DriftSeverity {
    Normal,    // < 25ms drift
    Minor,     // 25-50ms drift
    Moderate,  // 50-100ms drift
    Severe,    // > 100ms drift
}

/// Manages network communication between Ground Control and Satellite
pub struct NetworkManager {
    socket: TokioUdpSocket,
    satellite_address: SocketAddr,
    local_address: SocketAddr,
    receive_timeout: Duration,
    send_timeout: Duration,
    packet_sequence: AtomicU32,

    // Statistics (thread-safe)
    packets_sent: AtomicU64,
    packets_received: AtomicU64,
    bytes_sent: AtomicU64,
    bytes_received: AtomicU64,
    retransmission_requests: AtomicU64,
    last_packet_time: Arc<Mutex<Option<DateTime<Utc>>>>,

    // Expected packet intervals by stringified type
    expected_intervals: Arc<Mutex<HashMap<String, f64>>>,
    packet_sequence_tracker: Arc<Mutex<HashMap<String, PacketSequenceInfo>>>,
    drift_history: Arc<Mutex<VecDeque<DriftMeasurement>>>,
    expected_schedule: Arc<Mutex<HashMap<String, ExpectedSchedule>>>,

    // crypto (shared with satellite)
    crypto: CryptoContext,
}

#[derive(Debug, Clone)]
struct PacketSequenceInfo {
    packet_type: String,
    last_sequence: u64,
    expected_next_time: DateTime<Utc>,
    packets_received: u64,
    total_drift_ms: f64,
    max_drift_ms: f64,
}

#[derive(Debug, Clone)]
struct DriftMeasurement {
    timestamp: DateTime<Utc>,
    packet_type: String,
    expected_time: DateTime<Utc>,
    actual_time: DateTime<Utc>,
    drift_ms: f64,
    sequence_gap: bool,
}

#[derive(Debug, Clone)]
struct ExpectedSchedule {
    start_time: DateTime<Utc>,
    interval_ms: f64,
    next_expected: DateTime<Utc>,
    packets_expected: u64,
}

#[derive(Debug, Clone)]
pub struct PacketTypeDriftStats {
    pub packet_type: String,
    pub packets_received: u64,
    pub avg_drift_ms: f64,
    pub max_drift_ms: f64,
    pub violations: u32,
}

impl NetworkManager {
    /// Creates a new NetworkManager bound to default Ground Control port
    pub async fn new() -> Result<Self> {
        Self::new_with_config(
            "127.0.0.1:7891".parse()?, // Ground Control address
            "127.0.0.1:7890".parse()?, // Satellite address
            Duration::from_millis(100), // Receive timeout
            Duration::from_millis(50),  // Send timeout
        ).await
    }

    /// Creates NetworkManager with custom configuration
    pub async fn new_with_config(
        local_addr: SocketAddr,
        satellite_addr: SocketAddr,
        recv_timeout: Duration,
        send_timeout: Duration,
    ) -> Result<Self> {
        info!("Binding UDP socket to {}", local_addr);

        let socket = TokioUdpSocket::bind(local_addr).await
            .with_context(|| format!("Failed to bind to {}", local_addr))?;

        let key_bytes = hex::decode(KEY_HEX)
            .context("GC: failed to decode key hex")?;
        let key_array: [u8; 32] = key_bytes.try_into()
            .map_err(|_| anyhow::anyhow!("GC: key must be 32 bytes"))?;
        let crypto = CryptoContext::new(KEY_ID, key_array);

        info!("Network Manager initialized - Local: {}, Satellite: {}",
            local_addr, satellite_addr);

        let mut intervals = HashMap::new();
        intervals.insert("telemetry".to_string(), 100.0);  // 10 Hz
        intervals.insert("heartbeat".to_string(), 1000.0); // 1 Hz
        intervals.insert("emergency".to_string(), 50.0);   // 20 Hz
        intervals.insert("status".to_string(), 500.0);     // 2 Hz

        Ok(Self {
            socket,
            satellite_address: satellite_addr,
            local_address: local_addr,
            receive_timeout: recv_timeout,
            send_timeout,
            packet_sequence: AtomicU32::new(0),
            packets_sent: AtomicU64::new(0),
            packets_received: AtomicU64::new(0),
            bytes_sent: AtomicU64::new(0),
            bytes_received: AtomicU64::new(0),
            retransmission_requests: AtomicU64::new(0),
            last_packet_time: Arc::new(Mutex::new(None)),
            expected_intervals: Arc::new(Mutex::new(intervals)),
            packet_sequence_tracker: Arc::new(Mutex::new(HashMap::new())),
            drift_history: Arc::new(Mutex::new(VecDeque::with_capacity(1000))),
            expected_schedule: Arc::new(Mutex::new(HashMap::new())),
            crypto,
        })
    }

    /// Receives a packet from the satellite with detailed timing analysis (deframe + decrypt)
    pub async fn receive_packet_with_timing(&self) -> Result<(CommunicationPacket, ReceptionTiming)> {
        let receive_start = Instant::now();

        // Buffer for receiving data (sat sends: [len:4][EncryptedFrame JSON...])
        let mut buffer = vec![0u8; shared_protocol::MAX_PACKET_SIZE + 4];

        // Receive with timeout
        let (bytes_received, sender_addr) = timeout(
            self.receive_timeout,
            self.socket.recv_from(&mut buffer)
        ).await
        .context("Receive timeout")?
        .context("Failed to receive packet")?;

        let reception_time = Utc::now();
        let reception_latency = receive_start.elapsed().as_secs_f64() * 1000.0;

        debug!("Received {} bytes from {} in {:.3}ms",
            bytes_received, sender_addr, reception_latency);

        // Verify sender (ip match is enough for localhost demo)
        if sender_addr.ip() != self.satellite_address.ip() {
            return Err(anyhow::anyhow!(
                "Packet received from unexpected source: {} (expected from {})",
                sender_addr, self.satellite_address
            ));
        }

        // ---- Deframe: length prefix (big-endian u32), then JSON EncryptedFrame
        if bytes_received < 4 {
            return Err(anyhow::anyhow!("short UDP frame ({} bytes)", bytes_received));
        }
        let len = u32::from_be_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]) as usize;
        if bytes_received < 4 + len {
            return Err(anyhow::anyhow!(
                "incomplete framed payload: got {}, want {}",
                bytes_received, 4 + len
            ));
        }
        let frame_bytes = &buffer[4..4 + len];

        // Optional: inspect header fields for logs
        if let Ok(frame) = serde_json::from_slice::<EncryptedFrame>(frame_bytes) {
            debug!(
                event = "rx_frame",
                pkt_type = ?frame.header.packet_type,
                seq      = frame.header.sequence_number,
                src      = ?frame.header.source,
                dst      = ?frame.header.destination,
                key_id   = frame.header.key_id,
                ct_len   = frame.ciphertext.len(),
                "GC received frame"
            );
        }

        // ---- Decrypt (measure decode time too)
        let decode_start = Instant::now();
        let packet = self.crypto.open_from_bytes(frame_bytes)
            .map_err(|e| anyhow::anyhow!("decrypt/open failed: {}", e))?;
        let decode_time_ms = decode_start.elapsed().as_secs_f64() * 1000.0;

        // Calculate timing metrics WITH decode time
        let mut timing = self.calculate_reception_timing(&packet, reception_time).await;
        timing.decode_time_ms = decode_time_ms;

        // Decode time requirement (≤3ms)
        if decode_time_ms > 3.0 {
            error!(
                "DECODE VIOLATION: Packet {} decode took {:.3}ms (limit: 3ms)",
                packet.header.packet_id, decode_time_ms
            );
        }

        // Update statistics
        self.packets_received.fetch_add(1, Ordering::Relaxed);
        self.bytes_received.fetch_add(bytes_received as u64, Ordering::Relaxed);
        *self.last_packet_time.lock().await = Some(reception_time);

        Ok((packet, timing))
    }

    /// Original receive_packet method for backward compatibility
    pub async fn receive_packet(&self) -> Result<CommunicationPacket> {
        let (packet, _timing) = self.receive_packet_with_timing().await?;
        Ok(packet)
    }

    /// Calculate comprehensive reception timing metrics
    async fn calculate_reception_timing(&self, packet: &CommunicationPacket, reception_time: DateTime<Utc>) -> ReceptionTiming {
        let packet_type_s = format!("{:?}", packet.header.packet_type);
        let packet_id = packet.header.packet_id.clone();

        // trackers
        let mut sequence_tracker = self.packet_sequence_tracker.lock().await;
        let mut expected_schedule = self.expected_schedule.lock().await;

        let expected_interval = {
            let intervals = self.expected_intervals.lock().await;
            intervals.get(&packet_type_s).copied().unwrap_or(100.0)
        };

        // Initialize schedule if first packet of this type
        let schedule = expected_schedule.entry(packet_type_s.clone())
            .or_insert_with(|| ExpectedSchedule {
                start_time: reception_time,
                interval_ms: expected_interval,
                next_expected: reception_time,
                packets_expected: 0,
            });

        let interval_dur = chrono::Duration::milliseconds(expected_interval as i64);

        // First packet: drift = 0
        let (expected_time, drift_ms) = if schedule.packets_expected == 0 {
            schedule.packets_expected = 1;
            schedule.next_expected = reception_time + interval_dur;
            (reception_time, 0.0)
        } else {
            let expected_time = schedule.next_expected;
            let drift_ms = (reception_time - expected_time)
                .num_microseconds()
                .unwrap_or(0) as f64 / 1000.0;

            while schedule.next_expected <= reception_time {
                schedule.next_expected = schedule.next_expected + interval_dur;
                schedule.packets_expected += 1;
            }
            (expected_time, drift_ms)
        };

        let jitter_ms = drift_ms.abs();

        // Update sequence tracking (by packet-type string)
        let seq_info = sequence_tracker.entry(packet_type_s.clone())
            .or_insert_with(|| PacketSequenceInfo {
                packet_type: packet_type_s.clone(),
                last_sequence: 0,
                expected_next_time: reception_time,
                packets_received: 0,
                total_drift_ms: 0.0,
                max_drift_ms: 0.0,
            });

        seq_info.packets_received += 1;
        seq_info.total_drift_ms += drift_ms.abs();
        seq_info.max_drift_ms = seq_info.max_drift_ms.max(drift_ms.abs());
        seq_info.expected_next_time = reception_time + chrono::Duration::milliseconds(expected_interval as i64);

        // Record drift measurement for history
        let mut drift_history = self.drift_history.lock().await;
        drift_history.push_back(DriftMeasurement {
            timestamp: reception_time,
            packet_type: packet_type_s.clone(),
            expected_time,
            actual_time: reception_time,
            drift_ms,
            sequence_gap: false,
        });

        if drift_history.len() > 1000 {
            drift_history.pop_front();
        }

        // end-to-end latency (header timestamp → now)
        let packet_timestamp = packet.header.timestamp;
        let end_to_end_latency = (reception_time - packet_timestamp).num_milliseconds() as f64;

        // Determine severity
        let delay_severity = match jitter_ms {
            j if j < 25.0 => DriftSeverity::Normal,
            j if j < 50.0 => DriftSeverity::Minor,
            j if j < 100.0 => DriftSeverity::Moderate,
            _ => DriftSeverity::Severe,
        };

        if jitter_ms > 25.0 {
            warn!(
                "DRIFT DETECTED: {} - Expected: {}, Actual: {}, Drift: {:.1}ms, Severity: {:?}",
                packet_id,
                expected_time.format("%H:%M:%S%.3f"),
                reception_time.format("%H:%M:%S%.3f"),
                drift_ms,
                delay_severity
            );
        }

        ReceptionTiming {
            packet_id,
            packet_type: packet_type_s,
            reception_time,
            packet_timestamp,
            end_to_end_latency_ms: end_to_end_latency,
            reception_drift_ms: drift_ms,
            jitter_ms,
            is_delayed: jitter_ms > 25.0 || end_to_end_latency > 200.0,
            delay_severity,
            decode_time_ms: 0.0, // set by caller
        }
    }

    /// Update expected interval for a packet type (by string name)
    pub async fn set_expected_interval(&self, packet_type: &str, interval_ms: f64) {
        let mut intervals = self.expected_intervals.lock().await;
        intervals.insert(packet_type.to_string(), interval_ms);
        info!("Updated expected interval for '{}' to {:.1}ms", packet_type, interval_ms);
    }

    /// Get current reception drift statistics
    pub async fn get_drift_stats(&self) -> DriftStats {
        let drift_history = self.drift_history.lock().await;
        let sequence_tracker = self.packet_sequence_tracker.lock().await;

        if drift_history.is_empty() {
            return DriftStats {
                avg_drift_ms: 0.0,
                max_drift_ms: 0.0,
                drift_violations: 0,
                total_packets_analyzed: 0,
            };
        }

        let total_drift: f64 = drift_history.iter().map(|d| d.drift_ms.abs()).sum();
        DriftStats {
            avg_drift_ms: total_drift / drift_history.len() as f64,
            max_drift_ms: drift_history.iter().map(|d| d.drift_ms.abs()).fold(0.0, f64::max),
            drift_violations: drift_history.iter().filter(|d| d.drift_ms.abs() > 25.0).count() as u32,
            total_packets_analyzed: sequence_tracker.values().map(|s| s.packets_received).sum(),
        }
    }

    /// Get detailed drift report by packet type
    pub async fn get_drift_report_by_type(&self) -> HashMap<String, PacketTypeDriftStats> {
        let sequence_tracker = self.packet_sequence_tracker.lock().await;
        let drift_history = self.drift_history.lock().await;
        let mut report = HashMap::new();

        for (packet_type, seq_info) in sequence_tracker.iter() {
            let avg_drift = if seq_info.packets_received > 0 {
                seq_info.total_drift_ms / seq_info.packets_received as f64
            } else {
                0.0
            };

            report.insert(packet_type.clone(), PacketTypeDriftStats {
                packet_type: packet_type.clone(),
                packets_received: seq_info.packets_received,
                avg_drift_ms: avg_drift,
                max_drift_ms: seq_info.max_drift_ms,
                violations: if seq_info.max_drift_ms > 25.0 { 1 } else { 0 },
            });
        }

        for m in drift_history.iter() {
            if m.drift_ms.abs() > 25.0 {
                if let Some(s) = report.get_mut(&m.packet_type) {
                    s.violations += 1;
                } else {
                    report.insert(m.packet_type.clone(), PacketTypeDriftStats {
                        packet_type: m.packet_type.clone(),
                        packets_received: 1,
                        avg_drift_ms: m.drift_ms.abs(),
                        max_drift_ms: m.drift_ms.abs(),
                        violations: 1,
                    });
                }
            }
        }

        report
    }

    /// Sends a packet to the satellite (seal + send framed bytes)
    pub async fn send_packet(&self, packet: CommunicationPacket) -> Result<()> {
        let send_start = Instant::now();

        // Seal → framed bytes [len:4][frame_json…]
        let packet_bytes = self.crypto.seal_to_bytes(&packet)
            .map_err(|e| anyhow::anyhow!("seal_to_bytes failed: {}", e))?;

        debug!(
            "Sending packet {} ({} bytes framed) to satellite",
            packet.header.packet_id, packet_bytes.len()
        );

        let bytes_sent = timeout(
            self.send_timeout,
            self.socket.send_to(&packet_bytes, self.satellite_address)
        ).await
        .context("Send timeout")?
        .context("Failed to send packet")?;

        let send_latency = send_start.elapsed().as_secs_f64() * 1000.0;

        if bytes_sent != packet_bytes.len() {
            return Err(anyhow::anyhow!(
                "Incomplete send: {} of {} bytes", bytes_sent, packet_bytes.len()
            ));
        }

        self.packets_sent.fetch_add(1, Ordering::Relaxed);
        self.bytes_sent.fetch_add(bytes_sent as u64, Ordering::Relaxed);
        self.packet_sequence.fetch_add(1, Ordering::Relaxed);

        debug!("Packet sent successfully in {:.3}ms", send_latency);

        Ok(())
    }

    /// Send a packet with deadline enforcement for urgent commands (seal + send)
    pub async fn send_packet_with_deadline_check(
        &self,
        packet: CommunicationPacket,
        is_urgent: bool,
        deadline: Option<DateTime<Utc>>
    ) -> Result<SendResult> {
        let send_start_time = Utc::now();

        if let Some(dl) = deadline {
            if send_start_time > dl {
                return Ok(SendResult {
                    success: false,
                    send_time_ms: 0.0,
                    deadline_met: false,
                    deadline_violation_ms: (send_start_time - dl)
                        .num_microseconds().unwrap_or(0) as f64 / 1000.0,
                    packet_id: packet.header.packet_id.clone(),
                });
            }
        }

        // Seal → framed bytes
        let packet_bytes = self.crypto.seal_to_bytes(&packet)
            .map_err(|e| anyhow::anyhow!("seal_to_bytes failed: {}", e))?;

        debug!(
            "Sending {} packet {} ({} bytes framed) to satellite",
            if is_urgent { "URGENT" } else { "normal" },
            packet.header.packet_id,
            packet_bytes.len()
        );

        // Measure network send only
        let network_send_start = Instant::now();
        let bytes_sent = timeout(
            self.send_timeout,
            self.socket.send_to(&packet_bytes, self.satellite_address)
        ).await
        .context("Send timeout")?
        .context("Failed to send packet")?;

        let network_send_time_ms = network_send_start.elapsed().as_secs_f64() * 1000.0;
        let send_complete_time = Utc::now();

        if bytes_sent != packet_bytes.len() {
            return Err(anyhow::anyhow!(
                "Incomplete send: {} of {} bytes", bytes_sent, packet_bytes.len()
            ));
        }

        let deadline_met = if let Some(dl) = deadline {
            send_complete_time <= dl
        } else {
            true
        };

        let deadline_violation_ms = if let Some(dl) = deadline {
            if send_complete_time > dl {
                (send_complete_time - dl).num_microseconds().unwrap_or(0) as f64 / 1000.0
            } else { 0.0 }
        } else { 0.0 };

        if is_urgent {
            if network_send_time_ms > 2.0 {
                error!(
                    "URGENT COMMAND DEADLINE VIOLATION: {} took {:.3}ms to send (limit: 2ms)",
                    packet.header.packet_id, network_send_time_ms
                );
            } else {
                debug!(
                    "Urgent command {} sent in {:.3}ms (within 2ms limit)",
                    packet.header.packet_id, network_send_time_ms
                );
            }
            if !deadline_met {
                error!(
                    "URGENT COMMAND MISSED DEADLINE: {} violated deadline by {:.3}ms",
                    packet.header.packet_id, deadline_violation_ms
                );
            }
        }

        self.packets_sent.fetch_add(1, Ordering::Relaxed);
        self.bytes_sent.fetch_add(bytes_sent as u64, Ordering::Relaxed);
        self.packet_sequence.fetch_add(1, Ordering::Relaxed);

        Ok(SendResult {
            success: true,
            send_time_ms: network_send_time_ms,
            deadline_met,
            deadline_violation_ms,
            packet_id: packet.header.packet_id.clone(),
        })
    }

    /// Request retransmission (uses Command from shared_protocol)
    pub async fn request_retransmission(&self, packet_id: &str) -> Result<()> {
        info!("Requesting retransmission of packet: {}", packet_id);
        let (sensor_id, sensor_type, reason) = self.parse_packet_info(packet_id);
        let re_request = Command::re_request_command(sensor_id, sensor_type, &reason);
        let packet = CommunicationPacket::new_command(re_request, Source::GroundControl);
        self.send_packet(packet).await
            .context("Failed to send retransmission request")?;
        self.retransmission_requests.fetch_add(1, Ordering::Relaxed);
        info!("Re-request command sent for {:?} sensor {} - packet: {}",
              sensor_type, sensor_id, packet_id);
        Ok(())
    }

    pub async fn request_sensor_retransmission(
        &self,
        sensor_id: u32,
        sensor_type: &str,
        reason: &str
    ) -> Result<()> {
        info!("Requesting sensor retransmission - ID: {}, Type: {}, Reason: {}",
              sensor_id, sensor_type, reason);

        let sensor_enum = match sensor_type {
            "thermal" => SensorType::Thermal,
            "power" => SensorType::Power,
            "attitude" => SensorType::Attitude,
            other => anyhow::bail!("Invalid sensor type: {other}. Must be 'thermal', 'power', or 'attitude'"),
        };

        let re_request = Command::re_request_command(sensor_id, sensor_enum, reason);
        let packet = CommunicationPacket::new_command(re_request, Source::GroundControl);
        self.send_packet(packet).await
            .context("Failed to send sensor retransmission request")?;
        self.retransmission_requests.fetch_add(1, Ordering::Relaxed);
        info!("Sensor re-request sent successfully for {} sensor {}", sensor_type, sensor_id);
        Ok(())
    }

    fn parse_packet_info(&self, packet_id: &str) -> (u32, SensorType, String) {
        if packet_id.contains("thermal") {
            let sid = packet_id.split('_').filter_map(|p| p.parse::<u32>().ok()).next().unwrap_or(1);
            (sid, SensorType::Thermal, "thermal_data_missing".to_string())
        } else if packet_id.contains("power") {
            (4, SensorType::Power, "power_data_missing".to_string())
        } else if packet_id.contains("attitude") {
            (6, SensorType::Attitude, "attitude_data_missing".to_string())
        } else if packet_id.contains("seq") {
            (1, SensorType::Thermal, "sequence_gap_detected".to_string())
        } else if packet_id.contains("delayed") {
            (1, SensorType::Thermal, "packet_delayed".to_string())
        } else {
            (1, SensorType::Thermal, format!("packet_missing_{}", packet_id))
        }
    }

    pub async fn is_connection_healthy(&self) -> bool {
        match *self.last_packet_time.lock().await {
            Some(last_time) => {
                let d = Utc::now() - last_time;
                d.num_milliseconds() < 5000
            }
            None => false,
        }
    }
}

/// Network performance and connection statistics
#[derive(Debug, Clone)]
pub struct NetworkStats {
    pub packets_sent: u64,
    pub packets_received: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub retransmission_requests: u64,
    pub local_address: SocketAddr,
    pub satellite_address: SocketAddr,
    pub last_packet_time: Option<DateTime<Utc>>,
}

impl NetworkStats {
    pub fn packets_per_second(&self, duration: Duration) -> f64 {
        self.packets_received as f64 / duration.as_secs_f64()
    }
    pub fn avg_packet_size(&self) -> f64 {
        if self.packets_received == 0 { 0.0 } else {
            self.bytes_received as f64 / self.packets_received as f64
        }
    }
    pub fn connection_status(&self) -> String {
        match self.last_packet_time {
            Some(last_time) => {
                let s = (Utc::now() - last_time).num_seconds();
                if s < 2 { "Excellent" } else if s < 5 { "Good" }
                else if s < 10 { "Poor" } else { "Lost" }
            }.to_string(),
            None => "No Contact".to_string(),
        }
    }
    pub fn retransmission_rate(&self) -> f64 {
        if self.packets_received == 0 { 0.0 } else {
            self.retransmission_requests as f64 / self.packets_received as f64
        }
    }
}

/// Drift statistics for reporting
#[derive(Debug, Clone)]
pub struct DriftStats {
    pub avg_drift_ms: f64,
    pub max_drift_ms: f64,
    pub drift_violations: u32,
    pub total_packets_analyzed: u64,
}

/// Result of a deadline-aware send operation
#[derive(Debug, Clone)]
pub struct SendResult {
    pub success: bool,
    pub send_time_ms: f64,          // network send only
    pub deadline_met: bool,
    pub deadline_violation_ms: f64,
    pub packet_id: String,
}
