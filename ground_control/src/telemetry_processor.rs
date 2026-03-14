// src/telemetry.rs

use std::collections::HashMap;
use std::time::Instant;
use chrono::{DateTime, Utc, Duration};
use tracing::{info, warn, error, debug};
use anyhow::Result;

use shared_protocol::{
    CommunicationPacket, PacketPayload, SensorReading,
    SensorType, PacketType, Severity as NetSeverity,
};
use crate::fault_management::{FaultEvent, FaultType, Severity};

/// Processes and analyzes telemetry packets from the satellite
#[derive(Debug)]
pub struct TelemetryProcessor {
    last_sequence_numbers: HashMap<String, u32>, // by packet-type string
    missing_packets: Vec<MissingPacketInfo>,
    delayed_packets: Vec<DelayedPacketInfo>,
    packet_history: Vec<PacketRecord>,
    expected_sensors: HashMap<u32, ExpectedSensorConfig>,
    last_packet_times: HashMap<String, DateTime<Utc>>,
    
    // Statistics
    total_packets_processed: u64,
    total_sensors_processed: u64,
    total_processing_time_ms: f64,
    consecutive_failures: u32,
    delayed_packet_count: u64,  
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
struct PacketRecord {
    packet_id: String,
    packet_type: String,       // stringified enum
    sequence_number: u32,
    reception_time: DateTime<Utc>,
    processing_time_ms: f64,
    sensor_count: usize,
    has_errors: bool,
    reception_delay_ms: Option<f64>, 
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
struct ExpectedSensorConfig {
    sensor_id: u32,
    sensor_type: String, // for display only
    expected_interval_ms: u64,
    last_reading_time: Option<DateTime<Utc>>,
    critical_thresholds: SensorThresholds,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
struct SensorThresholds {
    thermal_critical: f64,
    thermal_emergency: f64,
    power_low: f64,
    power_critical: f64,
    attitude_warning: f64,
    attitude_critical: f64,
}

#[derive(Debug, Clone)]
struct DelayedPacketInfo {
    packet_id: String,
    packet_type: String,
    expected_time: DateTime<Utc>,
    actual_delay_ms: f64,
    detected_at: DateTime<Utc>,
    reported: bool,
}

impl Default for SensorThresholds {
    fn default() -> Self {
        Self {
            thermal_critical: 80.0,
            thermal_emergency: 85.0,
            power_low: 30.0,
            power_critical: 20.0,
            attitude_warning: 5.0,
            attitude_critical: 10.0,
        }
    }
}

/// Result of processing a telemetry packet
#[derive(Debug)]
pub struct TelemetryProcessingResult {
    pub packet_id: String,
    pub processing_time_ms: f64,
    pub sensor_count: usize,
    pub detected_faults: Vec<FaultEvent>,
    pub missing_packets_detected: Vec<String>,
    pub delayed_packets_detected: Vec<DelayedPacketResult>, 
    pub command_acknowledgments: Vec<CommandAckObservation>,
    pub sensor_analysis: Vec<SensorAnalysis>,
}

#[derive(Debug, Clone)]
pub struct CommandAckObservation {
    pub command_id: String,
    pub status: String,
    pub observed_at: DateTime<Utc>,
    pub execution_timestamp: Option<DateTime<Utc>>,
    pub completion_timestamp: Option<DateTime<Utc>>,
}

/// Detailed information about detected delayed packets
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct DelayedPacketResult {
    pub packet_id: String,
    pub packet_type: String,
    pub delay_amount_ms: f64,
    pub expected_time: DateTime<Utc>,
    pub actual_time: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct MissingPacketUplinkCandidate {
    pub packet_id: String,
    pub wait_ms: f64,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct SensorAnalysis {
    pub sensor_id: u32,
    pub sensor_type: String,
    pub status: String,
    pub primary_value: f64,
    pub anomalies_detected: Vec<String>,
    pub trend_analysis: Option<TrendInfo>,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct TrendInfo {
    pub direction: String,
    pub rate_of_change: f64,
    pub confidence: f64,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
struct MissingPacketInfo {
    packet_id: String,
    expected_sequence: u32,
    missed_at: DateTime<Utc>,
    re_request_sent: bool,
}

impl TelemetryProcessor {
    pub fn new() -> Self {
        info!("Starting Telemetry Processing Engine");
        
        let mut expected_sensors: HashMap<u32, ExpectedSensorConfig> = HashMap::new();
        
        for i in 1..=3 {
            expected_sensors.insert(i, ExpectedSensorConfig {
                sensor_id: i,
                sensor_type: "thermal".to_string(),
                expected_interval_ms: 50, // 20Hz
                last_reading_time: None,
                critical_thresholds: SensorThresholds::default(),
            });
        }
        for i in 4..=5 {
            expected_sensors.insert(i, ExpectedSensorConfig {
                sensor_id: i,
                sensor_type: "power".to_string(),
                expected_interval_ms: 100, // 10Hz
                last_reading_time: None,
                critical_thresholds: SensorThresholds::default(),
            });
        }
        expected_sensors.insert(6, ExpectedSensorConfig {
            sensor_id: 6,
            sensor_type: "attitude".to_string(),
            expected_interval_ms: 200, // 5Hz
            last_reading_time: None,
            critical_thresholds: SensorThresholds::default(),
        });
        
        Self {
            last_sequence_numbers: HashMap::new(),
            missing_packets: Vec::new(),
            delayed_packets: Vec::new(),                   
            packet_history: Vec::new(),
            expected_sensors,
            last_packet_times: HashMap::new(),              
            total_packets_processed: 0,
            total_sensors_processed: 0,
            total_processing_time_ms: 0.0,
            consecutive_failures: 0,
            delayed_packet_count: 0,   
        }
    }
    
    /// Main packet processing function - must complete within 3ms
    pub async fn process_telemetry_packet(
        &mut self, 
        packet: CommunicationPacket, 
        reception_time: DateTime<Utc>
    ) -> Result<TelemetryProcessingResult> {
        let processing_started_at = Instant::now();
        
        debug!("Processing Telemetry Packet: {} (Type: {:?})", 
            packet.header.packet_id, packet.header.packet_type);
        
        // Check for packet delays before other processing
        let _delay_probe_result = self.detect_packet_delay(&packet, reception_time);

        // Check packet sequence for gaps
        self.track_packet_sequence(&packet)?;
        
        let mut packet_result = TelemetryProcessingResult {
            packet_id: packet.header.packet_id.clone(),
            processing_time_ms: 0.0,
            sensor_count: 0,
            detected_faults: Vec::new(),
            missing_packets_detected: Vec::new(),
            delayed_packets_detected: Vec::new(),
            command_acknowledgments: Vec::new(),
            sensor_analysis: Vec::new(),
        };
        
        // Process based on packet type
        match &packet.payload {
            PacketPayload::TelemetryData(sensor_readings) => {
                packet_result.sensor_count = sensor_readings.len();
                
                for reading in sensor_readings {
                    match self.analyze_sensor_reading(reading, reception_time).await {
                        Ok(analysis) => {
                            packet_result.sensor_analysis.push(analysis.clone());
                            
                            // Check for sensor-specific faults
                            if let Some(fault) = self.detect_sensor_fault_event(reading, &analysis) {
                                packet_result.detected_faults.push(fault);
                            }
                        }
                        Err(e) => {
                            error!("Sensor Processing Failed For {}: {}", reading.sensor_id, e);
                            packet_result.detected_faults.push(FaultEvent {
                                timestamp: Utc::now(),
                                fault_type: FaultType::TelemetryError,
                                severity: Severity::Medium,
                                description: format!("Sensor {} Processing Pipeline Failed: {}", reading.sensor_id, e),
                                affected_systems: vec![format!("sensor_{}", reading.sensor_id)],
                            });
                        }
                    }
                }
                
                self.total_sensors_processed += packet_result.sensor_count as u64;
            }
            
            PacketPayload::EmergencyAlert(emergency) => {
                warn!("Emergency Alert Received: {}", emergency.description);
                packet_result.detected_faults.push(FaultEvent {
                    timestamp: emergency.timestamp,
                    fault_type: FaultType::from_emergency_label(&emergency.alert_type),
                    severity: Severity::from_network_emergency_severity(&emergency.severity),
                    description: emergency.description.clone(),
                    affected_systems: emergency.affected_systems.clone(),
                });
            }
            
            PacketPayload::HeartbeatData(health) => {
                debug!("System Health Update Received: {}", health.overall_status);
                
                if health.cpu_usage_percent > 90.0 {
                    packet_result.detected_faults.push(FaultEvent {
                        timestamp: Utc::now(),
                        fault_type: FaultType::SystemOverload,
                        severity: Severity::High,
                        description: format!("Elevated CPU Utilization: {:.1}%", health.cpu_usage_percent),
                        affected_systems: vec!["cpu".to_string()],
                    });
                }
                
                if health.memory_usage_percent > 85.0 {
                    packet_result.detected_faults.push(FaultEvent {
                        timestamp: Utc::now(),
                        fault_type: FaultType::SystemOverload,
                        severity: Severity::Medium,
                        description: format!("Elevated Memory Utilization: {:.1}%", health.memory_usage_percent),
                        affected_systems: vec!["memory".to_string()],
                    });
                }
            }
            
            PacketPayload::CommandData(_) => {
                debug!("Command Data Packet Received (Uncommon For Ground Control Path)");
            }
            
            PacketPayload::AcknowledgmentData(ack) => {
                debug!("Command Acknowledgment Received: {} - {}", ack.command_id, ack.status);
                packet_result.command_acknowledgments.push(CommandAckObservation {
                    command_id: ack.command_id.clone(),
                    status: ack.status.clone(),
                    observed_at: reception_time,
                    execution_timestamp: ack.execution_timestamp,
                    completion_timestamp: ack.completion_timestamp,
                });
            }
        }
        
        // Check for missing packets in the sequence
        packet_result.missing_packets_detected = self.collect_missing_packet_ids();

        // Get detailed delayed packet information
        packet_result.delayed_packets_detected = self.collect_unreported_delayed_packet_details();
        
        // Calculate processing time
        let processing_time = processing_started_at.elapsed().as_secs_f64() * 1000.0;
        packet_result.processing_time_ms = processing_time;
        
        // Update statistics
        self.total_packets_processed += 1;
        self.total_processing_time_ms += processing_time;
        
        // Record packet in history
        self.packet_history.push(PacketRecord {
            packet_id: packet.header.packet_id.clone(),
            packet_type: format!("{:?}", packet.header.packet_type),
            sequence_number: packet.header.sequence_number,
            reception_time,
            processing_time_ms: processing_time,
            sensor_count: packet_result.sensor_count,
            has_errors: !packet_result.detected_faults.is_empty(),
            reception_delay_ms: None,
        });
        
        if self.packet_history.len() > 1000 {
            self.packet_history.remove(0);
        }
        
        if processing_time > 3.0 {
            warn!("Telemetry Processing Exceeded 3ms Target: {:.3}ms", processing_time);
            self.consecutive_failures += 1;
        } else {
            self.consecutive_failures = 0;
        }
        
        info!("Packet {} Processed In {:.3}ms - {} Sensors, {} Faults, {} Delayed", 
            packet_result.packet_id, processing_time, packet_result.sensor_count, packet_result.detected_faults.len(), packet_result.delayed_packets_detected.len());

        Ok(packet_result)
    }
    
    /// Check for packet delay
    fn expected_interval_ms(packet_type: PacketType) -> f64 {
        match packet_type {
            PacketType::Telemetry => 100.0,
            PacketType::Heartbeat => 1000.0,
            PacketType::Emergency => 50.0,
            _ => 100.0,
        }
    }

    fn detect_packet_delay(&mut self, packet: &CommunicationPacket, reception_time: DateTime<Utc>) -> Option<DelayedPacketInfo> {
        let packet_type_s = format!("{:?}", packet.header.packet_type);

        let expected_interval_ms = Self::expected_interval_ms(packet.header.packet_type);
        
        if let Some(&last_time) = self.last_packet_times.get(&packet_type_s) {
            let actual_interval_ms = (reception_time - last_time).num_milliseconds() as f64;
            let expected_time = last_time + Duration::milliseconds(expected_interval_ms as i64);
            let delay_ms = (reception_time - expected_time).num_milliseconds() as f64;
            let is_delayed = delay_ms > 25.0 || actual_interval_ms > expected_interval_ms * 1.5;
            
            if is_delayed {
                let delayed_info = DelayedPacketInfo {
                    packet_id: packet.header.packet_id.clone(),
                    packet_type: packet_type_s.clone(),
                    expected_time,
                    actual_delay_ms: delay_ms.max(0.0),
                    detected_at: reception_time,
                    reported: false,
                };
                
                self.delayed_packets.push(delayed_info.clone());
                self.delayed_packet_count += 1;
                
                match delay_ms {
                    d if d > 500.0 => error!("SEVERE Packet Delay: {} ({}) Delayed By {:.1}ms", 
                        packet.header.packet_id, packet_type_s, delay_ms),
                    d if d > 200.0 => warn!("HIGH Packet Delay: {} ({}) Delayed By {:.1}ms", 
                        packet.header.packet_id, packet_type_s, delay_ms),
                    d if d > 50.0 => info!("Moderate Packet Delay: {} ({}) Delayed By {:.1}ms", 
                        packet.header.packet_id, packet_type_s, delay_ms),
                    _ => debug!("Minor Packet Delay: {} ({}) Delayed By {:.1}ms", 
                        packet.header.packet_id, packet_type_s, delay_ms),
                }
                
                self.last_packet_times.insert(packet_type_s.clone(), reception_time);
                return Some(delayed_info);
            }
        }
        
        self.last_packet_times.insert(packet_type_s, reception_time);
        None
    }
    
    /// Process individual sensor reading
    async fn analyze_sensor_reading(
        &mut self, 
        reading: &SensorReading, 
        reception_time: DateTime<Utc>
    ) -> Result<SensorAnalysis> {
        let mut anomalies = Vec::new();
        let mut status = format!("{:?}", reading.status);
        
        // Update expected sensor timing
        if let Some(config) = self.expected_sensors.get_mut(&reading.sensor_id) {
            if let Some(last_time) = config.last_reading_time {
                let actual_interval = (reception_time - last_time).num_milliseconds() as u64;
                let expected_interval = config.expected_interval_ms;
                
                if actual_interval > expected_interval * 2 {
                    anomalies.push(format!("Late reading: {}ms (expected ~{}ms)", 
                        actual_interval, expected_interval));
                }
            }
            config.last_reading_time = Some(reception_time);
        }
        
        match reading.sensor_type {
            SensorType::Thermal => {
                let temperature = reading.value1;
                let critical_threshold = reading.value2;
                let emergency_threshold = reading.value3;
                
                if temperature >= emergency_threshold {
                    status = "emergency".to_string();
                    anomalies.push(format!("Temperature emergency: {:.1}°C", temperature));
                } else if temperature >= critical_threshold {
                    status = "critical".to_string();
                    anomalies.push(format!("Temperature critical: {:.1}°C", temperature));
                } else if temperature > critical_threshold * 0.9 {
                    anomalies.push(format!("Temperature approaching critical: {:.1}°C", temperature));
                }
                
                if let Some(trend) = self.estimate_thermal_trend(reading.sensor_id, temperature) {
                    if trend.rate_of_change > 5.0 {
                        anomalies.push(format!("Rapid temperature rise: {:.1}°C/min", trend.rate_of_change));
                    }
                }
            }
            SensorType::Power => {
                let battery_percentage = reading.value1;
                let voltage = reading.value2;
                let _current = reading.value3;
                let power_watts = reading.value4;
                
                if battery_percentage <= 20.0 {
                    status = "critical".to_string();
                    anomalies.push(format!("Battery critical: {:.1}%", battery_percentage));
                } else if battery_percentage <= 30.0 {
                    status = "warning".to_string();
                    anomalies.push(format!("Battery low: {:.1}%", battery_percentage));
                }
                
                if power_watts > 100.0 {
                    anomalies.push(format!("High power consumption: {:.1}W", power_watts));
                }
                
                if voltage < 11.0 || voltage > 13.0 {
                    anomalies.push(format!("Voltage out of range: {:.2}V", voltage));
                }
            }
            SensorType::Attitude => {
                let roll = reading.value1;
                let pitch = reading.value2;
                let yaw = reading.value3;
                let error = reading.value4;
                
                if error >= 10.0 {
                    status = "critical".to_string();
                    anomalies.push(format!("Attitude error critical: {:.1}°", error));
                } else if error >= 5.0 {
                    status = "warning".to_string();
                    anomalies.push(format!("Attitude error high: {:.1}°", error));
                }
                
                if roll.abs() > 45.0 || pitch.abs() > 45.0 {
                    anomalies.push(format!("Extreme attitude: R{:.1}° P{:.1}° Y{:.1}°", 
                        roll, pitch, yaw));
                }
            }
        }
        
        // Quality: treat >=3 as poor (0..=4 scale)
        if (reading.quality as u8) >= 3 {
            anomalies.push(format!("Poor data quality: level {}", reading.quality as u8));
            if (reading.quality as u8) >= 4 {
                status = "error".to_string();
            }
        }
        
        // Priority bump
        if (reading.priority as u8) == 0 {
            status = "emergency".to_string();
        } else if (reading.priority as u8) == 1 && status == "normal" {
            status = "critical".to_string();
        }
        
        let trend_analysis = self.estimate_sensor_trend(reading);
        
        Ok(SensorAnalysis {
            sensor_id: reading.sensor_id,
            sensor_type: format!("{:?}", reading.sensor_type),
            status,
            primary_value: reading.value1,
            anomalies_detected: anomalies,
            trend_analysis,
        })
    }
    
    /// Check packet sequence for missing packets.
    ///
    /// A sequence gap larger than MAX_SEQUENCE_GAP almost certainly means the
    /// satellite uses a *global* sequence counter while GCS tracks per packet-type
    /// sequences.  In that case we resync silently instead of flooding the
    /// re-request list with tens of thousands of phantom missing packets.
    fn track_packet_sequence(&mut self, packet: &CommunicationPacket) -> Result<()> {
        /// Maximum plausible gap before we treat the jump as a resync event.
        const MAX_SEQUENCE_GAP: u32 = 10;

        let packet_type_s = format!("{:?}", packet.header.packet_type);
        let current_sequence = packet.header.sequence_number;
        
        if let Some(&last_sequence) = self.last_sequence_numbers.get(&packet_type_s) {
            let expected_sequence = last_sequence + 1;
            
            if current_sequence > expected_sequence {
                let gap = current_sequence - expected_sequence;
                if gap > MAX_SEQUENCE_GAP {
                    // Large gap → satellite likely uses a global counter.
                    // Log one warning and resync; do NOT create thousands of
                    // missing-packet entries that would cause a re-request flood.
                    warn!(
                        "Large Sequence Jump For {} (Expected {} Got {}, Gap {}): \
                         Resyncing Sequence Tracker - No Re-Requests Generated",
                        packet_type_s, expected_sequence, current_sequence, gap
                    );
                } else {
                    for missing_seq in expected_sequence..current_sequence {
                        self.missing_packets.push(MissingPacketInfo {
                            packet_id: format!("{}_seq_{}", packet_type_s, missing_seq),
                            expected_sequence: missing_seq,
                            missed_at: Utc::now(),
                            re_request_sent: false,
                        });
                    }
                    warn!("Missing Packet Span For {}: Seq {}..{}",
                        packet_type_s, expected_sequence, current_sequence - 1);
                }
            } else if current_sequence < expected_sequence {
                debug!("Duplicate Or Out-Of-Order Packet Received: {} (Expected: {})", 
                    current_sequence, expected_sequence);
            }
        }
        
        self.last_sequence_numbers.insert(packet_type_s, current_sequence);
        Ok(())
    }

    /// Get detailed delayed packet information
    pub fn collect_unreported_delayed_packet_details(&mut self) -> Vec<DelayedPacketResult> {
        let mut results = Vec::new();
        
        for delayed in &mut self.delayed_packets {
            if !delayed.reported && delayed.actual_delay_ms > 200.0 {
                results.push(DelayedPacketResult {
                    packet_id: delayed.packet_id.clone(),
                    packet_type: delayed.packet_type.clone(),
                    delay_amount_ms: delayed.actual_delay_ms,
                    expected_time: delayed.expected_time,
                    actual_time: delayed.detected_at,
                });
                delayed.reported = true;
            }
        }
        
        results
    }

    /// Get list of missing packet IDs
    pub fn collect_missing_packet_ids(&mut self) -> Vec<String> {
        let now = Utc::now();
        let mut missing_packet_ids = Vec::new();
        
        for missing in &mut self.missing_packets {
            if !missing.re_request_sent && (now - missing.missed_at).num_milliseconds() > 1000 {
                missing_packet_ids.push(missing.packet_id.clone());
            }
        }
        
        missing_packet_ids
    }

    /// Get missing packets ready for re-request with current wait age in ms.
    pub fn collect_missing_packet_uplink_candidates(&mut self) -> Vec<MissingPacketUplinkCandidate> {
        let now = Utc::now();
        let mut candidates = Vec::new();

        for missing in &mut self.missing_packets {
            if !missing.re_request_sent && (now - missing.missed_at).num_milliseconds() > 1000 {
                let wait_ms = (now - missing.missed_at).num_microseconds().unwrap_or(0) as f64 / 1000.0;
                candidates.push(MissingPacketUplinkCandidate {
                    packet_id: missing.packet_id.clone(),
                    wait_ms,
                });
            }
        }

        candidates
    }

    /// Mark packet as re-requested
    pub fn mark_packet_as_rerequested(&mut self, packet_id: &str) {
        for missing in &mut self.missing_packets {
            if missing.packet_id == packet_id {
                missing.re_request_sent = true;
                break;
            }
        }
    }
    
    /// Clean up old records
    pub fn prune_stale_missing_packets(&mut self) {
        let now = Utc::now();
        let old_count = self.missing_packets.len();
        
        self.missing_packets.retain(|missing| {
            (now - missing.missed_at).num_seconds() < 30
        });
        
        let cleaned_count = old_count - self.missing_packets.len();
        if cleaned_count > 0 {
            debug!("Pruned {} Aged Missing-Packet Records", cleaned_count);
        }
    }
    
    pub fn prune_stale_delayed_packets(&mut self) {
        let now = Utc::now();
        let old_count = self.delayed_packets.len();
        
        self.delayed_packets.retain(|delayed| {
            (now - delayed.detected_at).num_seconds() < 30
        });
        
        let cleaned_count = old_count - self.delayed_packets.len();
        if cleaned_count > 0 {
            debug!("Pruned {} Aged Delayed-Packet Records", cleaned_count);
        }
    }
    
    /// Check for sensor faults mapping using SensorType
    fn detect_sensor_fault_event(&self, reading: &SensorReading, analysis: &SensorAnalysis) -> Option<FaultEvent> {
        match analysis.status.as_str() {
            "emergency" => Some(FaultEvent {
                timestamp: reading.timestamp,
                fault_type: FaultType::from_sensor_label(&format!("{:?}", reading.sensor_type)),
                severity: Severity::Critical,
                description: format!("Emergency Condition Detected In {:?} Sensor {}: {:.2}", 
                    reading.sensor_type, reading.sensor_id, reading.value1),
                affected_systems: vec![format!("{:?}", reading.sensor_type), reading.location.clone()],
            }),
            
            "critical" => Some(FaultEvent {
                timestamp: reading.timestamp,
                fault_type: FaultType::from_sensor_label(&format!("{:?}", reading.sensor_type)),
                severity: Severity::High,
                description: format!("Critical Condition Detected In {:?} Sensor {}: {:.2}", 
                    reading.sensor_type, reading.sensor_id, reading.value1),
                affected_systems: vec![format!("{:?}", reading.sensor_type)],
            }),
            
            "error" => Some(FaultEvent {
                timestamp: reading.timestamp,
                fault_type: FaultType::TelemetryError,
                severity: Severity::Medium,
                description: format!("Sensor Error Detected In {:?} Sensor {}", 
                    reading.sensor_type, reading.sensor_id),
                affected_systems: vec![format!("sensor_{}", reading.sensor_id)],
            }),
            
            _ => {
                if analysis.anomalies_detected.len() >= 2 {
                    Some(FaultEvent {
                        timestamp: reading.timestamp,
                        fault_type: FaultType::from_sensor_label(&format!("{:?}", reading.sensor_type)),
                        severity: Severity::Medium,
                        description: format!("Multiple Anomalies Detected In {:?} Sensor {}: {}", 
                            reading.sensor_type, reading.sensor_id, 
                            analysis.anomalies_detected.join(", ")),
                        affected_systems: vec![format!("{:?}", reading.sensor_type)],
                    })
                } else {
                    None
                }
            }
        }
    }
    
    /// Calculate thermal trend
    fn estimate_thermal_trend(&self, _sensor_id: u32, _current_temp: f64) -> Option<TrendInfo> {
        Some(TrendInfo {
            direction: "stable".to_string(),
            rate_of_change: 0.0,
            confidence: 0.5,
        })
    }
    
    /// Calculate sensor trend
    fn estimate_sensor_trend(&self, reading: &SensorReading) -> Option<TrendInfo> {
        if reading.value1.abs() > 0.0 {
            Some(TrendInfo {
                direction: "stable".to_string(),
                rate_of_change: 0.0,
                confidence: 0.5,
            })
        } else {
            None
        }
    }
    
}

// Extension traits for fault type mapping
impl FaultType {
    pub fn from_sensor_label(sensor_type: &str) -> Self {
        match sensor_type {
            "thermal" => FaultType::ThermalAnomaly,
            "power" => FaultType::PowerAnomaly,
            "attitude" => FaultType::AttitudeAnomaly,
            _ => FaultType::TelemetryError,
        }
    }
    pub fn from_emergency_label(emergency_type: &str) -> Self {
        match emergency_type {
            "thermal" => FaultType::ThermalAnomaly,
            "power" => FaultType::PowerAnomaly,
            "communication" => FaultType::NetworkError,
            "attitude" => FaultType::AttitudeAnomaly,
            _ => FaultType::SystemOverload,
        }
    }
}

impl Severity {
    pub fn from_network_emergency_severity(severity: &NetSeverity) -> Self {
        match severity {
            NetSeverity::Critical => Severity::Critical,
            NetSeverity::High => Severity::High,
            NetSeverity::Medium => Severity::Medium,
            NetSeverity::Low => Severity::Low,
        }
    }
}
