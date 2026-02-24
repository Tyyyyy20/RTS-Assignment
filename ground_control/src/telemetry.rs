// src/telemetry.rs

use std::collections::HashMap;
use std::time::Instant;
use chrono::{DateTime, Utc, Duration};
use tracing::{info, warn, error, debug};
use anyhow::{Result, Context};

use shared_protocol::{
    CommunicationPacket, PacketPayload, SensorReading,
    SensorType, Status, Quality, PacketType, Severity as NetSeverity,
};
use crate::fault::{FaultEvent, FaultType, Severity};

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

#[derive(Debug, Clone)]
struct ExpectedSensorConfig {
    sensor_id: u32,
    sensor_type: String, // for display only
    expected_interval_ms: u64,
    last_reading_time: Option<DateTime<Utc>>,
    critical_thresholds: SensorThresholds,
}

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
    pub sensor_analysis: Vec<SensorAnalysis>,
}

/// Detailed information about detected delayed packets
#[derive(Debug, Clone)]
pub struct DelayedPacketResult {
    pub packet_id: String,
    pub packet_type: String,
    pub delay_amount_ms: f64,
    pub expected_time: DateTime<Utc>,
    pub actual_time: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct SensorAnalysis {
    pub sensor_id: u32,
    pub sensor_type: String,
    pub status: String,
    pub primary_value: f64,
    pub anomalies_detected: Vec<String>,
    pub trend_analysis: Option<TrendInfo>,
}

#[derive(Debug, Clone)]
pub struct TrendInfo {
    pub direction: String,
    pub rate_of_change: f64,
    pub confidence: f64,
}

#[derive(Debug, Clone)]
struct MissingPacketInfo {
    packet_id: String,
    expected_sequence: u32,
    missed_at: DateTime<Utc>,
    re_request_sent: bool,
}

impl TelemetryProcessor {
    pub fn new() -> Self {
        info!("Initializing Telemetry Processor");
        
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
    pub async fn process_packet(
        &mut self, 
        packet: CommunicationPacket, 
        reception_time: DateTime<Utc>
    ) -> Result<TelemetryProcessingResult> {
        let processing_start = Instant::now();
        
        debug!("Processing packet: {} (Type: {:?})", 
            packet.header.packet_id, packet.header.packet_type);
        
        // Check for packet delays before other processing
        let _delay_info = self.check_packet_delay(&packet, reception_time);

        // Check packet sequence for gaps
        self.check_packet_sequence(&packet)?;
        
        let mut result = TelemetryProcessingResult {
            packet_id: packet.header.packet_id.clone(),
            processing_time_ms: 0.0,
            sensor_count: 0,
            detected_faults: Vec::new(),
            missing_packets_detected: Vec::new(),
            delayed_packets_detected: Vec::new(),
            sensor_analysis: Vec::new(),
        };
        
        // Process based on packet type
        match &packet.payload {
            PacketPayload::TelemetryData(sensor_readings) => {
                result.sensor_count = sensor_readings.len();
                
                for reading in sensor_readings {
                    match self.process_sensor_reading(reading, reception_time).await {
                        Ok(analysis) => {
                            result.sensor_analysis.push(analysis.clone());
                            
                            // Check for sensor-specific faults
                            if let Some(fault) = self.check_sensor_faults(reading, &analysis) {
                                result.detected_faults.push(fault);
                            }
                        }
                        Err(e) => {
                            error!("Failed to process sensor {}: {}", reading.sensor_id, e);
                            result.detected_faults.push(FaultEvent {
                                timestamp: Utc::now(),
                                fault_type: FaultType::TelemetryError,
                                severity: Severity::Medium,
                                description: format!("Sensor {} processing failed: {}", reading.sensor_id, e),
                                affected_systems: vec![format!("sensor_{}", reading.sensor_id)],
                            });
                        }
                    }
                }
                
                self.total_sensors_processed += result.sensor_count as u64;
            }
            
            PacketPayload::EmergencyAlert(emergency) => {
                warn!("Emergency alert received: {}", emergency.description);
                result.detected_faults.push(FaultEvent {
                    timestamp: emergency.timestamp,
                    fault_type: FaultType::from_emergency_type(&emergency.alert_type),
                    severity: Severity::from_emergency_severity(&emergency.severity),
                    description: emergency.description.clone(),
                    affected_systems: emergency.affected_systems.clone(),
                });
            }
            
            PacketPayload::HeartbeatData(health) => {
                debug!("System health update: {}", health.overall_status);
                
                if health.cpu_usage_percent > 90.0 {
                    result.detected_faults.push(FaultEvent {
                        timestamp: Utc::now(),
                        fault_type: FaultType::SystemOverload,
                        severity: Severity::High,
                        description: format!("High CPU usage: {:.1}%", health.cpu_usage_percent),
                        affected_systems: vec!["cpu".to_string()],
                    });
                }
                
                if health.memory_usage_percent > 85.0 {
                    result.detected_faults.push(FaultEvent {
                        timestamp: Utc::now(),
                        fault_type: FaultType::SystemOverload,
                        severity: Severity::Medium,
                        description: format!("High memory usage: {:.1}%", health.memory_usage_percent),
                        affected_systems: vec!["memory".to_string()],
                    });
                }
            }
            
            PacketPayload::CommandData(_) => {
                debug!("Received command data packet (not typical for ground control)");
            }
            
            PacketPayload::AcknowledgmentData(ack) => {
                debug!("Command acknowledgment: {} - {}", ack.command_id, ack.status);
            }
        }
        
        // Check for missing packets in the sequence
        result.missing_packets_detected = self.get_missing_packets();

        // Get detailed delayed packet information
        result.delayed_packets_detected = self.take_unreported_delayed_packet_details();
        
        // Calculate processing time
        let processing_time = processing_start.elapsed().as_secs_f64() * 1000.0;
        result.processing_time_ms = processing_time;
        
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
            sensor_count: result.sensor_count,
            has_errors: !result.detected_faults.is_empty(),
            reception_delay_ms: None,
        });
        
        if self.packet_history.len() > 1000 {
            self.packet_history.remove(0);
        }
        
        if processing_time > 3.0 {
            warn!("Telemetry processing exceeded 3ms target: {:.3}ms", processing_time);
            self.consecutive_failures += 1;
        } else {
            self.consecutive_failures = 0;
        }
        
        info!("Packet {} processed in {:.3}ms - {} sensors, {} faults, {} delayed", 
            result.packet_id, processing_time, result.sensor_count, result.detected_faults.len(), result.delayed_packets_detected.len());

        Ok(result)
    }
    
    /// Check for packet delay
    fn check_packet_delay(&mut self, packet: &CommunicationPacket, reception_time: DateTime<Utc>) -> Option<DelayedPacketInfo> {
        let packet_type_s = format!("{:?}", packet.header.packet_type);
        let packet_timestamp = packet.header.timestamp;
        
        let expected_interval_ms = match packet.header.packet_type {
            PacketType::Telemetry => 100.0,
            PacketType::Heartbeat => 1000.0,
            PacketType::Emergency => 50.0,
            _ => 100.0,
        };
        
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
                    d if d > 500.0 => error!("SEVERE packet delay: {} ({}) delayed by {:.1}ms", 
                        packet.header.packet_id, packet_type_s, delay_ms),
                    d if d > 200.0 => warn!("HIGH packet delay: {} ({}) delayed by {:.1}ms", 
                        packet.header.packet_id, packet_type_s, delay_ms),
                    d if d > 50.0 => info!("Moderate packet delay: {} ({}) delayed by {:.1}ms", 
                        packet.header.packet_id, packet_type_s, delay_ms),
                    _ => debug!("Minor packet delay: {} ({}) delayed by {:.1}ms", 
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
    async fn process_sensor_reading(
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
                
                if let Some(trend) = self.calculate_thermal_trend(reading.sensor_id, temperature) {
                    if trend.rate_of_change > 5.0 {
                        anomalies.push(format!("Rapid temperature rise: {:.1}°C/min", trend.rate_of_change));
                    }
                }
            }
            SensorType::Power => {
                let battery_percentage = reading.value1;
                let voltage = reading.value2;
                let current = reading.value3;
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
            _ => {
                anomalies.push(format!("Unknown sensor type: {:?}", reading.sensor_type));
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
        
        let trend_analysis = self.calculate_sensor_trend(reading);
        
        Ok(SensorAnalysis {
            sensor_id: reading.sensor_id,
            sensor_type: format!("{:?}", reading.sensor_type),
            status,
            primary_value: reading.value1,
            anomalies_detected: anomalies,
            trend_analysis,
        })
    }
    
    /// Check packet sequence for missing packets
    fn check_packet_sequence(&mut self, packet: &CommunicationPacket) -> Result<()> {
        let packet_type_s = format!("{:?}", packet.header.packet_type);
        let current_sequence = packet.header.sequence_number;
        
        if let Some(&last_sequence) = self.last_sequence_numbers.get(&packet_type_s) {
            let expected_sequence = last_sequence + 1;
            
            if current_sequence > expected_sequence {
                for missing_seq in expected_sequence..current_sequence {
                    self.missing_packets.push(MissingPacketInfo {
                        packet_id: format!("{}_seq_{}", packet_type_s, missing_seq),
                        expected_sequence: missing_seq,
                        missed_at: Utc::now(),
                        re_request_sent: false,
                    });
                }
                
                warn!("Missing packets detected for {}: sequences {} to {}", 
                    packet_type_s, expected_sequence, current_sequence - 1);
            } else if current_sequence < expected_sequence {
                debug!("Received duplicate or out-of-order packet: {} (expected: {})", 
                    current_sequence, expected_sequence);
            }
        }
        
        self.last_sequence_numbers.insert(packet_type_s, current_sequence);
        Ok(())
    }

    /// Get detailed delayed packet information
    pub fn take_unreported_delayed_packet_details(&mut self) -> Vec<DelayedPacketResult> {
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
    pub fn get_missing_packets(&mut self) -> Vec<String> {
        let now = Utc::now();
        let mut missing_packet_ids = Vec::new();
        
        for missing in &mut self.missing_packets {
            if !missing.re_request_sent && (now - missing.missed_at).num_milliseconds() > 1000 {
                missing_packet_ids.push(missing.packet_id.clone());
            }
        }
        
        missing_packet_ids
    }

    /// Mark packet as re-requested
    pub fn mark_packet_re_requested(&mut self, packet_id: &str) {
        for missing in &mut self.missing_packets {
            if missing.packet_id == packet_id {
                missing.re_request_sent = true;
                break;
            }
        }
    }
    
    /// Clean up old records
    pub fn cleanup_old_missing_packets(&mut self) {
        let now = Utc::now();
        let old_count = self.missing_packets.len();
        
        self.missing_packets.retain(|missing| {
            (now - missing.missed_at).num_seconds() < 30
        });
        
        let cleaned_count = old_count - self.missing_packets.len();
        if cleaned_count > 0 {
            debug!("Cleaned up {} old missing packet records", cleaned_count);
        }
    }
    
    pub fn cleanup_old_delayed_packets(&mut self) {
        let now = Utc::now();
        let old_count = self.delayed_packets.len();
        
        self.delayed_packets.retain(|delayed| {
            (now - delayed.detected_at).num_seconds() < 30
        });
        
        let cleaned_count = old_count - self.delayed_packets.len();
        if cleaned_count > 0 {
            debug!("Cleaned up {} old delayed packet records", cleaned_count);
        }
    }
    
    /// Check for sensor faults mapping using SensorType
    fn check_sensor_faults(&self, reading: &SensorReading, analysis: &SensorAnalysis) -> Option<FaultEvent> {
        match analysis.status.as_str() {
            "emergency" => Some(FaultEvent {
                timestamp: reading.timestamp,
                fault_type: FaultType::from_sensor_type(&format!("{:?}", reading.sensor_type)),
                severity: Severity::Critical,
                description: format!("Emergency condition in {:?} sensor {}: {:.2}", 
                    reading.sensor_type, reading.sensor_id, reading.value1),
                affected_systems: vec![format!("{:?}", reading.sensor_type), reading.location.clone()],
            }),
            
            "critical" => Some(FaultEvent {
                timestamp: reading.timestamp,
                fault_type: FaultType::from_sensor_type(&format!("{:?}", reading.sensor_type)),
                severity: Severity::High,
                description: format!("Critical condition in {:?} sensor {}: {:.2}", 
                    reading.sensor_type, reading.sensor_id, reading.value1),
                affected_systems: vec![format!("{:?}", reading.sensor_type)],
            }),
            
            "error" => Some(FaultEvent {
                timestamp: reading.timestamp,
                fault_type: FaultType::TelemetryError,
                severity: Severity::Medium,
                description: format!("Sensor error in {:?} sensor {}", 
                    reading.sensor_type, reading.sensor_id),
                affected_systems: vec![format!("sensor_{}", reading.sensor_id)],
            }),
            
            _ => {
                if analysis.anomalies_detected.len() >= 2 {
                    Some(FaultEvent {
                        timestamp: reading.timestamp,
                        fault_type: FaultType::from_sensor_type(&format!("{:?}", reading.sensor_type)),
                        severity: Severity::Medium,
                        description: format!("Multiple anomalies in {:?} sensor {}: {}", 
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
    fn calculate_thermal_trend(&self, _sensor_id: u32, _current_temp: f64) -> Option<TrendInfo> {
        Some(TrendInfo {
            direction: "stable".to_string(),
            rate_of_change: 0.0,
            confidence: 0.5,
        })
    }
    
    /// Calculate sensor trend
    fn calculate_sensor_trend(&self, reading: &SensorReading) -> Option<TrendInfo> {
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
    
    /// Get statistics
    pub fn get_stats(&self) -> TelemetryStats {
        let avg_processing_time = if self.total_packets_processed > 0 {
            self.total_processing_time_ms / self.total_packets_processed as f64
        } else {
            0.0
        };
        
        let recent_errors = self.packet_history.iter()
            .rev()
            .take(100)
            .filter(|record| record.has_errors)
            .count();
        
        TelemetryStats {
            total_packets_processed: self.total_packets_processed,
            total_sensors_processed: self.total_sensors_processed,
            avg_processing_time_ms: avg_processing_time,
            consecutive_failures: self.consecutive_failures,
            missing_packets_count: self.missing_packets.len() as u32,
            delayed_packets_count: self.delayed_packets.len() as u32,
            recent_error_rate: recent_errors as f64 / (self.packet_history.len().min(100) as f64).max(1.0),
            active_sensors: self.expected_sensors.len() as u32,
        }
    }
}

/// Telemetry processing statistics
#[derive(Debug, Clone)]
pub struct TelemetryStats {
    pub total_packets_processed: u64,
    pub total_sensors_processed: u64,
    pub avg_processing_time_ms: f64,
    pub consecutive_failures: u32,
    pub missing_packets_count: u32,
    pub delayed_packets_count: u32,
    pub recent_error_rate: f64,
    pub active_sensors: u32,
}

// Extension traits for fault type mapping
impl FaultType {
    pub fn from_sensor_type(sensor_type: &str) -> Self {
        match sensor_type {
            "thermal" => FaultType::ThermalAnomaly,
            "power" => FaultType::PowerAnomaly,
            "attitude" => FaultType::AttitudeAnomaly,
            _ => FaultType::TelemetryError,
        }
    }
    pub fn from_emergency_type(emergency_type: &str) -> Self {
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
    pub fn from_emergency_severity(severity: &NetSeverity) -> Self {
        match severity {
            NetSeverity::Critical => Severity::Critical,
            NetSeverity::High => Severity::High,
            NetSeverity::Medium => Severity::Medium,
            NetSeverity::Low => Severity::Low,
            _ => Severity::Medium,
        }
    }
}
