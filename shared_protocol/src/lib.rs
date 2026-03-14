// lib.rs — Shared protocol with AEAD encryption (ChaCha20-Poly1305)

use chrono::{DateTime, Utc};
use crc32fast::Hasher; // retained for compatibility; not used on-wire once AEAD is on
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use uuid::Uuid;

// =============================== Common =====================================

pub type Timestamp = DateTime<Utc>;

pub const PROTOCOL_VERSION: u16 = 1;
pub const MAX_PACKET_SIZE: usize = 1024 * 1024; // 1MB
pub const DEFAULT_SATELLITE_PORT: u16 = 7890;
pub const DEFAULT_GROUND_CONTROL_PORT: u16 = 7891;

// =============================== Enums ======================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Satellite,
    GroundControl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PacketType {
    Telemetry,
    Command,
    Ack,
    Emergency,
    Heartbeat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SensorType {
    Thermal,
    Power,
    Attitude,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Emergency = 0,
    Critical = 1,
    Important = 2,
    Normal = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    Excellent = 0,
    Good = 1,
    Fair = 2,
    Poor = 3,
    Invalid = 4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Normal,
    Warning,
    Critical,
    Emergency,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandType {
    ThermalControl,
    PowerControl,
    AttitudeControl,
    Emergency,
    Recovery,
    Diagnostic,
    Maintenance,
    DataRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetSystem {
    AllSystems,
    ThermalManagement,
    PowerManagement,
    AttitudeControl,
}

// ======================== Unified Sensor Structures =========================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SensorReading {
    // Identity
    pub sensor_id: u32,
    pub sensor_type: SensorType,
    pub description: String,
    pub location: String,

    // Timing
    pub timestamp: Timestamp,
    pub sequence_number: u64,

    // Data (mapped per sensor type)
    pub value1: f64,
    pub value2: f64,
    pub value3: f64,
    pub value4: f64,

    // Health & importance
    pub priority: Priority,
    pub quality: Quality,
    pub status: Status,

    // Filled by OCS scheduler from monotonic clock
    pub processing_latency_ms: f64,
    pub jitter_ms: f64,
    pub drift_ms: f64,

    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThermalSensor {
    pub sensor_id: u32,
    pub location: String,
    pub critical_threshold: f64,
    pub emergency_threshold: f64,
    pub sampling_interval_ms: u64,
}

impl ThermalSensor {
    pub fn new(sensor_id: u32, location: &str) -> Self {
        Self {
            sensor_id,
            location: location.to_string(),
            critical_threshold: 80.0,
            emergency_threshold: 85.0,
            sampling_interval_ms: 50, // 20Hz
        }
    }

    /// value1: temp °C, value2: crit°, value3: emerg°, value4: 0
    pub fn create_reading(&self, temperature_celsius: f64, sequence_number: u64) -> SensorReading {
        let status = if temperature_celsius >= self.emergency_threshold {
            Status::Emergency
        } else if temperature_celsius >= self.critical_threshold {
            Status::Critical
        } else if temperature_celsius >= 60.0 {
            Status::Warning
        } else {
            Status::Normal
        };

        let priority = if temperature_celsius >= self.emergency_threshold {
            Priority::Emergency
        } else if temperature_celsius >= self.critical_threshold {
            Priority::Critical
        } else {
            Priority::Important
        };

        SensorReading {
            sensor_id: self.sensor_id,
            sensor_type: SensorType::Thermal,
            description: format!("Thermal sensor reading from {}", self.location),
            location: self.location.clone(),
            timestamp: Utc::now(),
            sequence_number,
            value1: temperature_celsius,
            value2: self.critical_threshold,
            value3: self.emergency_threshold,
            value4: 0.0,
            priority,
            quality: if (-50.0..150.0).contains(&temperature_celsius) {
                Quality::Good
            } else {
                Quality::Invalid
            },
            status,
            processing_latency_ms: 0.0,
            jitter_ms: 0.0,
            drift_ms: 0.0,
            metadata: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PowerSensor {
    pub sensor_id: u32,
    pub location: String,
    pub low_battery_threshold: f64,
    pub critical_battery_threshold: f64,
    pub sampling_interval_ms: u64,
}

impl PowerSensor {
    pub fn new(sensor_id: u32, location: &str) -> Self {
        Self {
            sensor_id,
            location: location.to_string(),
            low_battery_threshold: 30.0,
            critical_battery_threshold: 20.0,
            sampling_interval_ms: 100, // 10Hz
        }
    }

    /// value1: battery %, value2: V, value3: A, value4: W
    pub fn create_reading(
        &self,
        battery_percentage: f64,
        voltage: f64,
        current: f64,
        power_watts: f64,
        sequence_number: u64,
    ) -> SensorReading {
        let status = if battery_percentage <= self.critical_battery_threshold {
            Status::Critical
        } else if battery_percentage <= self.low_battery_threshold {
            Status::Warning
        } else {
            Status::Normal
        };

        let priority = if battery_percentage <= self.critical_battery_threshold {
            Priority::Critical
        } else if battery_percentage <= self.low_battery_threshold {
            Priority::Important
        } else {
            Priority::Normal
        };

        SensorReading {
            sensor_id: self.sensor_id,
            sensor_type: SensorType::Power,
            description: format!("Power subsystem sensor reading from {}", self.location),
            location: self.location.clone(),
            timestamp: Utc::now(),
            sequence_number,
            value1: battery_percentage,
            value2: voltage,
            value3: current,
            value4: power_watts,
            priority,
            quality: if (0.0..=100.0).contains(&battery_percentage) {
                Quality::Good
            } else {
                Quality::Invalid
            },
            status,
            processing_latency_ms: 0.0,
            jitter_ms: 0.0,
            drift_ms: 0.0,
            metadata: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttitudeSensor {
    pub sensor_id: u32,
    pub location: String,
    pub max_acceptable_error: f64,
    pub critical_error_threshold: f64,
    pub sampling_interval_ms: u64,
}

impl AttitudeSensor {
    pub fn new(sensor_id: u32, location: &str) -> Self {
        Self {
            sensor_id,
            location: location.to_string(),
            max_acceptable_error: 5.0,
            critical_error_threshold: 10.0,
            sampling_interval_ms: 200, // 5Hz
        }
    }

    /// value1: roll°, value2: pitch°, value3: yaw°, value4: √(r²+p²+y²)
    pub fn create_reading(
        &self,
        roll_degrees: f64,
        pitch_degrees: f64,
        yaw_degrees: f64,
        sequence_number: u64,
    ) -> SensorReading {
        let attitude_error =
            (roll_degrees.powi(2) + pitch_degrees.powi(2) + yaw_degrees.powi(2)).sqrt();

        let status = if attitude_error >= self.critical_error_threshold {
            Status::Critical
        } else if attitude_error >= self.max_acceptable_error {
            Status::Warning
        } else {
            Status::Normal
        };

        let priority = if attitude_error >= self.critical_error_threshold {
            Priority::Critical
        } else if attitude_error >= self.max_acceptable_error {
            Priority::Important
        } else {
            Priority::Normal
        };

        SensorReading {
            sensor_id: self.sensor_id,
            sensor_type: SensorType::Attitude,
            description: format!("Attitude sensor reading from {}", self.location),
            location: self.location.clone(),
            timestamp: Utc::now(),
            sequence_number,
            value1: roll_degrees,
            value2: pitch_degrees,
            value3: yaw_degrees,
            value4: attitude_error,
            priority,
            quality: Quality::Good,
            status,
            processing_latency_ms: 0.0,
            jitter_ms: 0.0,
            drift_ms: 0.0,
            metadata: HashMap::new(),
        }
    }
}

// ================================ Commands ==================================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Command {
    // Identification
    pub command_id: String,
    pub command_type: CommandType,
    pub description: String,
    pub target_system: TargetSystem,

    // Timing
    pub timestamp: Timestamp,
    pub deadline: Option<Timestamp>,
    pub retry_count: u8,

    // Params
    pub param1: f64,
    pub param2: f64,
    pub param3: f64,
    pub param4: f64,
    pub text_param: String,

    // Control
    pub priority: Priority,
    pub source: Source,
    pub destination: Source,

    pub metadata: HashMap<String, String>,
}

impl Command {
    // -------------------------- THERMAL ------------------------------------
    pub fn thermal_normal_operation(sensor_id: u32) -> Self {
        Self {
            command_id: Uuid::new_v4().to_string(),
            command_type: CommandType::ThermalControl,
            description: format!("Set thermal sensor {} to nominal mode", sensor_id),
            target_system: TargetSystem::ThermalManagement,
            timestamp: Utc::now(),
            deadline: Some(Utc::now() + chrono::Duration::seconds(10)),
            retry_count: 0,
            param1: sensor_id as f64,
            param2: 50.0, // 20Hz
            param3: 0.0,  // fan %
            param4: Priority::Normal as u8 as f64,
            text_param: "THERMAL_NORMAL".to_string(),
            priority: Priority::Normal,
            source: Source::GroundControl,
            destination: Source::Satellite,
            metadata: HashMap::new(),
        }
    }

    pub fn thermal_warning_response(sensor_id: u32, temperature: f64) -> Self {
        let mut metadata = HashMap::new();
        metadata.insert("trigger_temp".into(), temperature.to_string());
        metadata.insert("action".into(), "preventive_cooling".into());
        Self {
            command_id: Uuid::new_v4().to_string(),
            command_type: CommandType::ThermalControl,
            description: format!(
                "Thermal warning mitigation for sensor {} at {}°C",
                sensor_id, temperature
            ),
            target_system: TargetSystem::ThermalManagement,
            timestamp: Utc::now(),
            deadline: Some(Utc::now() + chrono::Duration::seconds(5)),
            retry_count: 0,
            param1: sensor_id as f64,
            param2: 25.0,  // 40Hz
            param3: 50.0,  // fan %
            param4: Priority::Important as u8 as f64,
            text_param: "THERMAL_WARNING".to_string(),
            priority: Priority::Important,
            source: Source::GroundControl,
            destination: Source::Satellite,
            metadata,
        }
    }

    pub fn thermal_critical_response(sensor_id: u32, temperature: f64) -> Self {
        let mut metadata = HashMap::new();
        metadata.insert("trigger_temp".into(), temperature.to_string());
        metadata.insert("action".into(), "aggressive_cooling".into());
        metadata.insert("power_reduction".into(), "30".into());
        Self {
            command_id: Uuid::new_v4().to_string(),
            command_type: CommandType::ThermalControl,
            description: format!(
                "Critical thermal mitigation for sensor {} at {}°C",
                sensor_id, temperature
            ),
            target_system: TargetSystem::ThermalManagement,
            timestamp: Utc::now(),
            deadline: Some(Utc::now() + chrono::Duration::milliseconds(500)),
            retry_count: 0,
            param1: sensor_id as f64,
            param2: 10.0,  // 100Hz
            param3: 100.0, // fan %
            param4: Priority::Critical as u8 as f64,
            text_param: "THERMAL_CRITICAL".to_string(),
            priority: Priority::Critical,
            source: Source::GroundControl,
            destination: Source::Satellite,
            metadata,
        }
    }

    pub fn thermal_emergency_response(sensor_id: u32, temperature: f64) -> Self {
        let mut metadata = HashMap::new();
        metadata.insert("trigger_temp".into(), temperature.to_string());
        metadata.insert("shutdown_non_essential".into(), "true".into());
        metadata.insert("maintain_life_support".into(), "true".into());
        Self {
            command_id: Uuid::new_v4().to_string(),
            command_type: CommandType::Emergency,
            description: format!(
                "EMERGENCY: thermal shutdown command for sensor {} at {}°C",
                sensor_id, temperature
            ),
            target_system: TargetSystem::AllSystems,
            timestamp: Utc::now(),
            deadline: Some(Utc::now() + chrono::Duration::milliseconds(100)),
            retry_count: 0,
            param1: sensor_id as f64,
            param2: 5.0,   // 200Hz sampling (monitoring)
            param3: 100.0, // fan %
            param4: Priority::Emergency as u8 as f64,
            text_param: "THERMAL_EMERGENCY".to_string(),
            priority: Priority::Emergency,
            source: Source::GroundControl,
            destination: Source::Satellite,
            metadata,
        }
    }

    // --------------------------- POWER -------------------------------------
    pub fn power_normal_operation(sensor_id: u32) -> Self {
        Self {
            command_id: Uuid::new_v4().to_string(),
            command_type: CommandType::PowerControl,
            description: format!("Set power sensor {} to nominal mode", sensor_id),
            target_system: TargetSystem::PowerManagement,
            timestamp: Utc::now(),
            deadline: Some(Utc::now() + chrono::Duration::seconds(10)),
            retry_count: 0,
            param1: sensor_id as f64,
            param2: 100.0, // 10Hz
            param3: 100.0, // power budget %
            param4: Priority::Normal as u8 as f64,
            text_param: "POWER_NORMAL".to_string(),
            priority: Priority::Normal,
            source: Source::GroundControl,
            destination: Source::Satellite,
            metadata: HashMap::new(),
        }
    }

    pub fn power_warning_response(sensor_id: u32, battery_level: f64) -> Self {
        let mut metadata = HashMap::new();
        metadata.insert("battery_level".into(), battery_level.to_string());
        metadata.insert("action".into(), "power_conservation".into());
        metadata.insert("dim_displays".into(), "true".into());
        Self {
            command_id: Uuid::new_v4().to_string(),
            command_type: CommandType::PowerControl,
            description: format!(
                "Power conservation command for sensor {} at {}% battery",
                sensor_id, battery_level
            ),
            target_system: TargetSystem::PowerManagement,
            timestamp: Utc::now(),
            deadline: Some(Utc::now() + chrono::Duration::seconds(2)),
            retry_count: 0,
            param1: sensor_id as f64,
            param2: 50.0,  // 20Hz
            param3: 80.0,  // reduce budget
            param4: Priority::Important as u8 as f64,
            text_param: "POWER_CONSERVATION".to_string(),
            priority: Priority::Important,
            source: Source::GroundControl,
            destination: Source::Satellite,
            metadata,
        }
    }

    pub fn power_critical_response(sensor_id: u32, battery_level: f64) -> Self {
        let mut metadata = HashMap::new();
        metadata.insert("battery_level".into(), battery_level.to_string());
        metadata.insert("shutdown_non_critical".into(), "true".into());
        metadata.insert("hibernate_mode".into(), "partial".into());
        Self {
            command_id: Uuid::new_v4().to_string(),
            command_type: CommandType::PowerControl,
            description: format!(
                "Critical power preservation command for sensor {} at {}% battery",
                sensor_id, battery_level
            ),
            target_system: TargetSystem::PowerManagement,
            timestamp: Utc::now(),
            deadline: Some(Utc::now() + chrono::Duration::milliseconds(500)),
            retry_count: 0,
            param1: sensor_id as f64,
            param2: 25.0, // 40Hz
            param3: 50.0, // severe reduction
            param4: Priority::Critical as u8 as f64,
            text_param: "POWER_CRITICAL".to_string(),
            priority: Priority::Critical,
            source: Source::GroundControl,
            destination: Source::Satellite,
            metadata,
        }
    }

    // -------------------------- ATTITUDE -----------------------------------
    pub fn attitude_normal_operation(sensor_id: u32) -> Self {
        Self {
            command_id: Uuid::new_v4().to_string(),
            command_type: CommandType::AttitudeControl,
            description: format!("Set attitude sensor {} to nominal mode", sensor_id),
            target_system: TargetSystem::AttitudeControl,
            timestamp: Utc::now(),
            deadline: Some(Utc::now() + chrono::Duration::seconds(10)),
            retry_count: 0,
            param1: sensor_id as f64,
            param2: 200.0, // 5Hz
            param3: 1.0,   // correction enabled
            param4: Priority::Normal as u8 as f64,
            text_param: "ATTITUDE_NORMAL".to_string(),
            priority: Priority::Normal,
            source: Source::GroundControl,
            destination: Source::Satellite,
            metadata: HashMap::new(),
        }
    }

    pub fn attitude_warning_response(sensor_id: u32, error_degrees: f64) -> Self {
        let mut metadata = HashMap::new();
        metadata.insert("error_degrees".into(), error_degrees.to_string());
        metadata.insert("action".into(), "gentle_correction".into());
        Self {
            command_id: Uuid::new_v4().to_string(),
            command_type: CommandType::AttitudeControl,
            description: format!(
                "Attitude correction command for sensor {} with {}° error",
                sensor_id, error_degrees
            ),
            target_system: TargetSystem::AttitudeControl,
            timestamp: Utc::now(),
            deadline: Some(Utc::now() + chrono::Duration::seconds(1)),
            retry_count: 0,
            param1: sensor_id as f64,
            param2: 100.0, // 10Hz
            param3: 50.0,  // thruster %
            param4: Priority::Important as u8 as f64,
            text_param: "ATTITUDE_CORRECTION".to_string(),
            priority: Priority::Important,
            source: Source::GroundControl,
            destination: Source::Satellite,
            metadata,
        }
    }

    pub fn attitude_critical_response(sensor_id: u32, error_degrees: f64) -> Self {
        let mut metadata = HashMap::new();
        metadata.insert("error_degrees".into(), error_degrees.to_string());
        metadata.insert("action".into(), "aggressive_stabilization".into());
        metadata.insert("all_thrusters".into(), "true".into());
        Self {
            command_id: Uuid::new_v4().to_string(),
            command_type: CommandType::AttitudeControl,
            description: format!(
                "Critical attitude stabilization command for sensor {} with {}° error",
                sensor_id, error_degrees
            ),
            target_system: TargetSystem::AttitudeControl,
            timestamp: Utc::now(),
            deadline: Some(Utc::now() + chrono::Duration::milliseconds(200)),
            retry_count: 0,
            param1: sensor_id as f64,
            param2: 50.0,  // 20Hz
            param3: 100.0, // thruster %
            param4: Priority::Critical as u8 as f64,
            text_param: "ATTITUDE_STABILIZE".to_string(),
            priority: Priority::Critical,
            source: Source::GroundControl,
            destination: Source::Satellite,
            metadata,
        }
    }

    // ----------------------- CROSS SYSTEM / OTHER --------------------------
    pub fn re_request_command(sensor_id: u32, sensor_type: SensorType, reason: &str) -> Self {
        let mut metadata = HashMap::new();
        metadata.insert("sensor_type".into(), format!("{sensor_type:?}").to_lowercase());
        metadata.insert("reason".into(), reason.to_string());
        Self {
            command_id: Uuid::new_v4().to_string(),
            command_type: CommandType::DataRequest,
            description: format!(
                "Request retransmission from {:?} sensor {} because {}",
                sensor_type, sensor_id, reason
            ),
            target_system: match sensor_type {
                SensorType::Thermal => TargetSystem::ThermalManagement,
                SensorType::Power => TargetSystem::PowerManagement,
                SensorType::Attitude => TargetSystem::AttitudeControl,
            },
            timestamp: Utc::now(),
            deadline: Some(Utc::now() + chrono::Duration::seconds(5)),
            retry_count: 0,
            param1: sensor_id as f64,
            param2: 1.0, // re-request flag
            param3: 0.0,
            param4: Priority::Important as u8 as f64,
            text_param: "RE_REQUEST_DATA".to_string(),
            priority: Priority::Important,
            source: Source::GroundControl,
            destination: Source::Satellite,
            metadata,
        }
    }

    pub fn enter_safe_mode(triggered_sensors: Vec<u32>) -> Self {
        let mut metadata = HashMap::new();
        metadata.insert(
            "triggered_sensors".into(),
            triggered_sensors
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .join(","),
        );
        metadata.insert("minimal_operations".into(), "true".into());
        metadata.insert("ground_contact_priority".into(), "true".into());
        Self {
            command_id: Uuid::new_v4().to_string(),
            command_type: CommandType::Emergency,
            description: "Enter safe mode due to multiple sensor alerts".to_string(),
            target_system: TargetSystem::AllSystems,
            timestamp: Utc::now(),
            deadline: Some(Utc::now() + chrono::Duration::milliseconds(500)),
            retry_count: 0,
            param1: triggered_sensors.len() as f64,
            param2: 1.0,  // enable safe mode
            param3: 25.0, // e.g., set sensors to 40Hz
            param4: Priority::Emergency as u8 as f64,
            text_param: "SAFE_MODE".to_string(),
            priority: Priority::Emergency,
            source: Source::GroundControl,
            destination: Source::Satellite,
            metadata,
        }
    }

    pub fn initiate_recovery_mode() -> Self {
        let mut metadata = HashMap::new();
        metadata.insert("recovery_phase".into(), "1".into());
        metadata.insert("gradual_restore".into(), "true".into());
        Self {
            command_id: Uuid::new_v4().to_string(),
            command_type: CommandType::Recovery,
            description: "Begin structured recovery from emergency state".to_string(),
            target_system: TargetSystem::AllSystems,
            timestamp: Utc::now(),
            deadline: Some(Utc::now() + chrono::Duration::seconds(30)),
            retry_count: 0,
            param1: 1.0,  // phase
            param2: 60.0, // timeout
            param3: 1.0,  // gradual restore
            param4: Priority::Critical as u8 as f64,
            text_param: "RECOVERY_MODE".to_string(),
            priority: Priority::Critical,
            source: Source::GroundControl,
            destination: Source::Satellite,
            metadata,
        }
    }

    pub fn sensor_self_test(sensor_id: u32, sensor_type: SensorType) -> Self {
        let mut metadata = HashMap::new();
        metadata.insert("sensor_type".into(), format!("{sensor_type:?}").to_lowercase());
        metadata.insert("test_type".into(), "comprehensive".into());
        Self {
            command_id: Uuid::new_v4().to_string(),
            command_type: CommandType::Diagnostic,
            description: format!("Run self-test for {:?} sensor {}", sensor_type, sensor_id),
            target_system: match sensor_type {
                SensorType::Thermal => TargetSystem::ThermalManagement,
                SensorType::Power => TargetSystem::PowerManagement,
                SensorType::Attitude => TargetSystem::AttitudeControl,
            },
            timestamp: Utc::now(),
            deadline: Some(Utc::now() + chrono::Duration::seconds(30)),
            retry_count: 0,
            param1: sensor_id as f64,
            param2: 1.0,  // enable
            param3: 10.0, // seconds
            param4: Priority::Important as u8 as f64,
            text_param: "SELF_TEST".to_string(),
            priority: Priority::Important,
            source: Source::GroundControl,
            destination: Source::Satellite,
            metadata,
        }
    }

    pub fn recalibrate_sensor(sensor_id: u32, sensor_type: SensorType) -> Self {
        let mut metadata = HashMap::new();
        metadata.insert("sensor_type".into(), format!("{sensor_type:?}").to_lowercase());
        metadata.insert("calibration_type".into(), "full".into());
        Self {
            command_id: Uuid::new_v4().to_string(),
            command_type: CommandType::Maintenance,
            description: format!("Run recalibration for {:?} sensor {}", sensor_type, sensor_id),
            target_system: match sensor_type {
                SensorType::Thermal => TargetSystem::ThermalManagement,
                SensorType::Power => TargetSystem::PowerManagement,
                SensorType::Attitude => TargetSystem::AttitudeControl,
            },
            timestamp: Utc::now(),
            deadline: Some(Utc::now() + chrono::Duration::minutes(5)),
            retry_count: 0,
            param1: sensor_id as f64,
            param2: 1.0,  // enable
            param3: 60.0, // timeout
            param4: Priority::Important as u8 as f64,
            text_param: "RECALIBRATE".to_string(),
            priority: Priority::Important,
            source: Source::GroundControl,
            destination: Source::Satellite,
            metadata,
        }
    }
}

// ============================= Packets (logical) ============================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommunicationPacket {
    pub header: PacketHeader,
    pub payload: PacketPayload,
    pub checksum: u32, // kept for backward compatibility; not used by AEAD
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PacketHeader {
    pub packet_id: String,
    pub source: Source,
    pub destination: Source,
    pub packet_type: PacketType,
    pub sequence_number: u32,
    pub timestamp: Timestamp,
    pub payload_size_bytes: u32,
    pub protocol_version: u16,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum PacketPayload {
    TelemetryData(Vec<SensorReading>),
    CommandData(Command),
    AcknowledgmentData(CommandAcknowledgment),
    EmergencyAlert(EmergencyData),
    HeartbeatData(SystemHealth),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandAcknowledgment {
    pub command_id: String,
    pub status: String, // "received", "executing", "completed", "failed"
    pub execution_timestamp: Option<Timestamp>,
    pub completion_timestamp: Option<Timestamp>,
    pub error_message: Option<String>,
    pub execution_time_ms: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmergencyData {
    pub alert_id: String,
    pub severity: Severity,
    pub alert_type: String,
    pub description: String,
    pub affected_systems: Vec<String>,
    pub recommended_actions: Vec<String>,
    pub auto_recovery_attempted: bool,
    pub timestamp: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SystemHealth {
    pub overall_status: String, // free-form for dashboards
    pub cpu_usage_percent: f64,
    pub memory_usage_percent: f64,
    pub disk_usage_percent: f64,
    pub uptime_seconds: u64,
    pub active_tasks: u32,
    pub failed_tasks: u32,
    pub timestamp: Timestamp,
}

// ---------- convenience creators (same as before) ----------

static GLOBAL_SEQ: AtomicU32 = AtomicU32::new(1);

impl CommunicationPacket {
    pub fn new_telemetry(readings: Vec<SensorReading>, source: Source) -> Self {
        let packet_payload = PacketPayload::TelemetryData(readings);
        Self::build_packet(packet_payload, source, PacketType::Telemetry)
    }

    pub fn new_command(command: Command, source: Source) -> Self {
        let packet_payload = PacketPayload::CommandData(command);
        Self::build_packet(packet_payload, source, PacketType::Command)
    }

    pub fn new_emergency(emergency: EmergencyData, source: Source) -> Self {
        let packet_payload = PacketPayload::EmergencyAlert(emergency);
        Self::build_packet(packet_payload, source, PacketType::Emergency)
    }

    pub fn new_ack(ack: CommandAcknowledgment, source: Source) -> Self {
        let packet_payload = PacketPayload::AcknowledgmentData(ack);
        Self::build_packet(packet_payload, source, PacketType::Ack)
    }

    pub fn new_heartbeat(health: SystemHealth, source: Source) -> Self {
        let packet_payload = PacketPayload::HeartbeatData(health);
        Self::build_packet(packet_payload, source, PacketType::Heartbeat)
    }

    fn build_packet(packet_payload: PacketPayload, source: Source, packet_type: PacketType) -> Self {
        let payload_bytes = serde_json::to_vec(&packet_payload).unwrap_or_default();
        let destination = match source {
            Source::Satellite => Source::GroundControl,
            Source::GroundControl => Source::Satellite,
        };

        let header = PacketHeader {
            packet_id: Uuid::new_v4().to_string(),
            source,
            destination,
            packet_type,
            sequence_number: GLOBAL_SEQ.fetch_add(1, Ordering::Relaxed),
            timestamp: Utc::now(),
            payload_size_bytes: payload_bytes.len() as u32,
            protocol_version: PROTOCOL_VERSION,
        };

        let mut packet = Self {
            header,
            payload: packet_payload,
            checksum: 0,
        };
        // (Checksum unused under AEAD; retained for compatibility)
        packet.checksum = packet.compute_checksum();
        packet
    }

    pub fn compute_checksum(&self) -> u32 {
        let mut checksum_hasher = Hasher::new();
        checksum_hasher.update(self.header.packet_id.as_bytes());
        checksum_hasher.update(&(self.header.protocol_version.to_be_bytes()));
        checksum_hasher.update(&(self.header.sequence_number.to_be_bytes()));
        checksum_hasher.update(&(self.header.payload_size_bytes.to_be_bytes()));
        if let Ok(source_json) = serde_json::to_string(&self.header.source) {
            checksum_hasher.update(source_json.as_bytes());
        }
        if let Ok(destination_json) = serde_json::to_string(&self.header.destination) {
            checksum_hasher.update(destination_json.as_bytes());
        }
        if let Ok(packet_type_json) = serde_json::to_string(&self.header.packet_type) {
            checksum_hasher.update(packet_type_json.as_bytes());
        }
        checksum_hasher.update(self.header.timestamp.to_rfc3339().as_bytes());
        if let Ok(payload_bytes) = serde_json::to_vec(&self.payload) {
            checksum_hasher.update(&payload_bytes);
        }
        checksum_hasher.finalize()
    }

    pub fn calculate_checksum(&self) -> u32 {
        // Backward-compatible alias for callers using the older API name.
        self.compute_checksum()
    }
}

// ============================ AEAD Crypto Envelope ===========================

use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::aead::rand_core::{OsRng, RngCore};

/// Clear header that stays outside encryption (needed for routing).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClearHeader {
    pub protocol_version: u16,
    pub packet_type: PacketType,
    pub sequence_number: u32,
    pub source: Source,
    pub destination: Source,
    pub key_id: u8,         // support key rotation
    pub nonce: [u8; 12],    // AEAD nonce (unique per key)
    // Optional: flags for compression, etc.
}

/// On-wire encrypted frame: [length (u32 BE)] [json(EncryptedFrame)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EncryptedFrame {
    pub header: ClearHeader, // used as AAD
    pub ciphertext: Vec<u8>, // includes Poly1305 tag appended
}

pub struct CryptoContext {
    key_id: u8,
    key: Key, // type alias, no generics
}

impl CryptoContext {
    pub fn new(key_id: u8, key_bytes_32: [u8; 32]) -> Self {
        Self {
            key_id,
            key: Key::from_slice(&key_bytes_32).to_owned(),
        }
    }

    fn cipher(&self) -> ChaCha20Poly1305 {
        ChaCha20Poly1305::new(&self.key)
    }

    fn generate_nonce_bytes() -> [u8; 12] {
        let mut nonce_bytes = [0u8; 12];
        OsRng.fill_bytes(&mut nonce_bytes);
        nonce_bytes
    }

    pub fn encode_encrypted_frame(&self, packet: &CommunicationPacket) -> Result<Vec<u8>, String> {
        // Serialize the logical packet (payload+header)
        let packet_bytes = serde_json::to_vec(packet)
            .map_err(|e| format!("Packet serialization failed: {e}"))?;

        if packet_bytes.len() > MAX_PACKET_SIZE {
            return Err(format!("Packet exceeds pre-encryption size limit: {}", packet_bytes.len()));
        }

        let nonce_bytes = Self::generate_nonce_bytes();
        let nonce = Nonce::from_slice(&nonce_bytes);

        let clear_header = ClearHeader {
            protocol_version: PROTOCOL_VERSION,
            packet_type: packet.header.packet_type,
            sequence_number: packet.header.sequence_number,
            source: packet.header.source,
            destination: packet.header.destination,
            key_id: self.key_id,
            nonce: nonce_bytes,
        };

        let associated_data = serde_json::to_vec(&clear_header)
            .map_err(|e| format!("AAD serialization failed: {e}"))?;

        let cipher = self.cipher();
        let ciphertext = cipher
            .encrypt(nonce, Payload { msg: &packet_bytes, aad: &associated_data })
            .map_err(|_| "Frame encryption failed".to_string())?;

        let encrypted_frame = EncryptedFrame {
            header: clear_header,
            ciphertext,
        };

        // Length-prefixed JSON framing for the encrypted frame
        let frame_bytes =
            serde_json::to_vec(&encrypted_frame).map_err(|e| format!("Encrypted frame serialization failed: {e}"))?;

        if frame_bytes.len() > MAX_PACKET_SIZE {
            return Err(format!("Encrypted frame exceeds size limit: {}", frame_bytes.len()));
        }

        let mut framed_bytes = Vec::with_capacity(frame_bytes.len() + 4);
        framed_bytes.extend_from_slice(&(frame_bytes.len() as u32).to_be_bytes());
        framed_bytes.extend_from_slice(&frame_bytes);
        Ok(framed_bytes)
    }

    /// Seal a logical packet to **length-prefixed encrypted bytes** ready to send.
    pub fn seal_to_bytes(&self, packet: &CommunicationPacket) -> Result<Vec<u8>, String> {
        // Backward-compatible alias for callers using the older API name.
        self.encode_encrypted_frame(packet)
    }

    pub fn decode_encrypted_frame(&self, framed_input: &[u8]) -> Result<CommunicationPacket, String> {
        if framed_input.len() < 4 {
            return Err("Insufficient data: 4-byte length prefix is missing".into());
        }
        let frame_len = u32::from_be_bytes([
            framed_input[0],
            framed_input[1],
            framed_input[2],
            framed_input[3],
        ]) as usize;
        if framed_input.len() < 4 + frame_len {
            return Err(format!(
                "Insufficient data: expected {} bytes, got {}",
                4 + frame_len,
                framed_input.len()
            ));
        }
        let frame_json = &framed_input[4..4 + frame_len];
        let encrypted_frame: EncryptedFrame =
            serde_json::from_slice(frame_json).map_err(|e| format!("Encrypted frame deserialization failed: {e}"))?;

        if encrypted_frame.header.key_id != self.key_id {
            return Err(format!(
                "Key ID mismatch: frame={}, context={}",
                encrypted_frame.header.key_id, self.key_id
            ));
        }

        let associated_data = serde_json::to_vec(&encrypted_frame.header)
            .map_err(|e| format!("AAD serialization failed: {e}"))?;
        let nonce = Nonce::from_slice(&encrypted_frame.header.nonce);

        let cipher = self.cipher();
        let plaintext = cipher
            .decrypt(
                nonce,
                Payload {
                    msg: &encrypted_frame.ciphertext,
                    aad: &associated_data,
                },
            )
            .map_err(|_| "Frame authentication/decryption failed".to_string())?;

        let packet: CommunicationPacket = serde_json::from_slice(&plaintext)
            .map_err(|e| format!("Packet deserialization failed: {e}"))?;

        // Optional: sanity checks (version, type, seq) vs clear header
        if packet.header.protocol_version != encrypted_frame.header.protocol_version
            || packet.header.packet_type != encrypted_frame.header.packet_type
            || packet.header.sequence_number != encrypted_frame.header.sequence_number
            || packet.header.source != encrypted_frame.header.source
            || packet.header.destination != encrypted_frame.header.destination
        {
            return Err("Header mismatch between clear header and decrypted payload".into());
        }

        Ok(packet)
    }

    /// Open **one complete frame** from a contiguous buffer (length-prefixed),
    /// returning the logical `CommunicationPacket`.
    pub fn open_from_bytes(&self, buf: &[u8]) -> Result<CommunicationPacket, String> {
        // Backward-compatible alias for callers using the older API name.
        self.decode_encrypted_frame(buf)
    }
}

// ================================ Tests =====================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telemetry_roundtrip_encrypted() {
        // Build a telemetry packet
        let thermal_sensor = ThermalSensor::new(1, "CPU");
        let telemetry_packet = CommunicationPacket::new_telemetry(
            vec![thermal_sensor.create_reading(72.5, 10)],
            Source::Satellite,
        );

        // Crypto
        let test_key = [7u8; 32];
        let crypto = CryptoContext::new(1, test_key);

        // Seal → bytes → open
        let frame_bytes = crypto.seal_to_bytes(&telemetry_packet).expect("seal");
        let decoded_packet = crypto.open_from_bytes(&frame_bytes).expect("open");

        assert_eq!(decoded_packet.header.packet_type, PacketType::Telemetry);
        assert_eq!(decoded_packet.header.source, Source::Satellite);
        assert_eq!(decoded_packet.header.destination, Source::GroundControl);
        match decoded_packet.payload {
            PacketPayload::TelemetryData(readings) => assert_eq!(readings.len(), 1),
            _ => panic!("unexpected payload kind"),
        }
    }

    #[test]
    fn command_roundtrip_encrypted() {
        let command = Command::thermal_normal_operation(1);
        let command_packet = CommunicationPacket::new_command(command, Source::GroundControl);

        let crypto = CryptoContext::new(42, [9u8; 32]);
        let frame_bytes = crypto.seal_to_bytes(&command_packet).unwrap();
        let decoded_packet = crypto.open_from_bytes(&frame_bytes).unwrap();

        assert!(matches!(decoded_packet.header.packet_type, PacketType::Command));
        assert_eq!(decoded_packet.header.source, Source::GroundControl);
        assert_eq!(decoded_packet.header.destination, Source::Satellite);
    }
}
