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
            description: format!("Temperature sensor at {}", self.location),
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
            description: format!("Power management sensor at {}", self.location),
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
            description: format!("Attitude control sensor at {}", self.location),
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
            description: format!("Set thermal sensor {} to normal operation", sensor_id),
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
        let mut meta = HashMap::new();
        meta.insert("trigger_temp".into(), temperature.to_string());
        meta.insert("action".into(), "preventive_cooling".into());
        Self {
            command_id: Uuid::new_v4().to_string(),
            command_type: CommandType::ThermalControl,
            description: format!(
                "Thermal warning response for sensor {} at {}°C",
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
            metadata: meta,
        }
    }

    pub fn thermal_critical_response(sensor_id: u32, temperature: f64) -> Self {
        let mut meta = HashMap::new();
        meta.insert("trigger_temp".into(), temperature.to_string());
        meta.insert("action".into(), "aggressive_cooling".into());
        meta.insert("power_reduction".into(), "30".into());
        Self {
            command_id: Uuid::new_v4().to_string(),
            command_type: CommandType::ThermalControl,
            description: format!(
                "Critical thermal response for sensor {} at {}°C",
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
            metadata: meta,
        }
    }

    pub fn thermal_emergency_response(sensor_id: u32, temperature: f64) -> Self {
        let mut meta = HashMap::new();
        meta.insert("trigger_temp".into(), temperature.to_string());
        meta.insert("shutdown_non_essential".into(), "true".into());
        meta.insert("maintain_life_support".into(), "true".into());
        Self {
            command_id: Uuid::new_v4().to_string(),
            command_type: CommandType::Emergency,
            description: format!(
                "EMERGENCY: Thermal shutdown for sensor {} at {}°C",
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
            metadata: meta,
        }
    }

    // --------------------------- POWER -------------------------------------
    pub fn power_normal_operation(sensor_id: u32) -> Self {
        Self {
            command_id: Uuid::new_v4().to_string(),
            command_type: CommandType::PowerControl,
            description: format!("Set power sensor {} to normal operation", sensor_id),
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
        let mut meta = HashMap::new();
        meta.insert("battery_level".into(), battery_level.to_string());
        meta.insert("action".into(), "power_conservation".into());
        meta.insert("dim_displays".into(), "true".into());
        Self {
            command_id: Uuid::new_v4().to_string(),
            command_type: CommandType::PowerControl,
            description: format!(
                "Power conservation for sensor {} at {}% battery",
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
            metadata: meta,
        }
    }

    pub fn power_critical_response(sensor_id: u32, battery_level: f64) -> Self {
        let mut meta = HashMap::new();
        meta.insert("battery_level".into(), battery_level.to_string());
        meta.insert("shutdown_non_critical".into(), "true".into());
        meta.insert("hibernate_mode".into(), "partial".into());
        Self {
            command_id: Uuid::new_v4().to_string(),
            command_type: CommandType::PowerControl,
            description: format!(
                "Critical power saving for sensor {} at {}% battery",
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
            metadata: meta,
        }
    }

    // -------------------------- ATTITUDE -----------------------------------
    pub fn attitude_normal_operation(sensor_id: u32) -> Self {
        Self {
            command_id: Uuid::new_v4().to_string(),
            command_type: CommandType::AttitudeControl,
            description: format!("Set attitude sensor {} to normal operation", sensor_id),
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
        let mut meta = HashMap::new();
        meta.insert("error_degrees".into(), error_degrees.to_string());
        meta.insert("action".into(), "gentle_correction".into());
        Self {
            command_id: Uuid::new_v4().to_string(),
            command_type: CommandType::AttitudeControl,
            description: format!(
                "Attitude correction for sensor {} with {}° error",
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
            metadata: meta,
        }
    }

    pub fn attitude_critical_response(sensor_id: u32, error_degrees: f64) -> Self {
        let mut meta = HashMap::new();
        meta.insert("error_degrees".into(), error_degrees.to_string());
        meta.insert("action".into(), "aggressive_stabilization".into());
        meta.insert("all_thrusters".into(), "true".into());
        Self {
            command_id: Uuid::new_v4().to_string(),
            command_type: CommandType::AttitudeControl,
            description: format!(
                "Critical attitude stabilization for sensor {} with {}° error",
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
            metadata: meta,
        }
    }

    // ----------------------- CROSS SYSTEM / OTHER --------------------------
    pub fn re_request_command(sensor_id: u32, sensor_type: SensorType, reason: &str) -> Self {
        let mut meta = HashMap::new();
        meta.insert("sensor_type".into(), format!("{sensor_type:?}").to_lowercase());
        meta.insert("reason".into(), reason.to_string());
        Self {
            command_id: Uuid::new_v4().to_string(),
            command_type: CommandType::DataRequest,
            description: format!(
                "Re-request data from {:?} sensor {} due to {}",
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
            metadata: meta,
        }
    }

    pub fn enter_safe_mode(triggered_sensors: Vec<u32>) -> Self {
        let mut meta = HashMap::new();
        meta.insert(
            "triggered_sensors".into(),
            triggered_sensors
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .join(","),
        );
        meta.insert("minimal_operations".into(), "true".into());
        meta.insert("ground_contact_priority".into(), "true".into());
        Self {
            command_id: Uuid::new_v4().to_string(),
            command_type: CommandType::Emergency,
            description: "Enter safe mode due to multiple sensor warnings".to_string(),
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
            metadata: meta,
        }
    }

    pub fn initiate_recovery_mode() -> Self {
        let mut meta = HashMap::new();
        meta.insert("recovery_phase".into(), "1".into());
        meta.insert("gradual_restore".into(), "true".into());
        Self {
            command_id: Uuid::new_v4().to_string(),
            command_type: CommandType::Recovery,
            description: "Begin systematic recovery from emergency state".to_string(),
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
            metadata: meta,
        }
    }

    pub fn sensor_self_test(sensor_id: u32, sensor_type: SensorType) -> Self {
        let mut meta = HashMap::new();
        meta.insert("sensor_type".into(), format!("{sensor_type:?}").to_lowercase());
        meta.insert("test_type".into(), "comprehensive".into());
        Self {
            command_id: Uuid::new_v4().to_string(),
            command_type: CommandType::Diagnostic,
            description: format!("Self-test for {:?} sensor {}", sensor_type, sensor_id),
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
            metadata: meta,
        }
    }

    pub fn recalibrate_sensor(sensor_id: u32, sensor_type: SensorType) -> Self {
        let mut meta = HashMap::new();
        meta.insert("sensor_type".into(), format!("{sensor_type:?}").to_lowercase());
        meta.insert("calibration_type".into(), "full".into());
        Self {
            command_id: Uuid::new_v4().to_string(),
            command_type: CommandType::Maintenance,
            description: format!("Recalibrate {:?} sensor {}", sensor_type, sensor_id),
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
            metadata: meta,
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
        let payload = PacketPayload::TelemetryData(readings);
        Self::create_packet(payload, source, PacketType::Telemetry)
    }

    pub fn new_command(command: Command, source: Source) -> Self {
        let payload = PacketPayload::CommandData(command);
        Self::create_packet(payload, source, PacketType::Command)
    }

    pub fn new_emergency(emergency: EmergencyData, source: Source) -> Self {
        let payload = PacketPayload::EmergencyAlert(emergency);
        Self::create_packet(payload, source, PacketType::Emergency)
    }

    pub fn new_ack(ack: CommandAcknowledgment, source: Source) -> Self {
        let payload = PacketPayload::AcknowledgmentData(ack);
        Self::create_packet(payload, source, PacketType::Ack)
    }

    pub fn new_heartbeat(health: SystemHealth, source: Source) -> Self {
        let payload = PacketPayload::HeartbeatData(health);
        Self::create_packet(payload, source, PacketType::Heartbeat)
    }

    fn create_packet(payload: PacketPayload, source: Source, packet_type: PacketType) -> Self {
        let payload_bytes = serde_json::to_vec(&payload).unwrap_or_default();
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
            payload,
            checksum: 0,
        };
        // (Checksum unused under AEAD; retained for compatibility)
        packet.checksum = packet.calculate_checksum();
        packet
    }

    pub fn calculate_checksum(&self) -> u32 {
        let mut hasher = Hasher::new();
        hasher.update(self.header.packet_id.as_bytes());
        hasher.update(&(self.header.protocol_version.to_be_bytes()));
        hasher.update(&(self.header.sequence_number.to_be_bytes()));
        hasher.update(&(self.header.payload_size_bytes.to_be_bytes()));
        if let Ok(hs) = serde_json::to_string(&self.header.source) {
            hasher.update(hs.as_bytes());
        }
        if let Ok(hd) = serde_json::to_string(&self.header.destination) {
            hasher.update(hd.as_bytes());
        }
        if let Ok(pt) = serde_json::to_string(&self.header.packet_type) {
            hasher.update(pt.as_bytes());
        }
        hasher.update(self.header.timestamp.to_rfc3339().as_bytes());
        if let Ok(payload_bytes) = serde_json::to_vec(&self.payload) {
            hasher.update(&payload_bytes);
        }
        hasher.finalize()
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

    fn gen_nonce() -> [u8; 12] {
        let mut nonce = [0u8; 12];
        OsRng.fill_bytes(&mut nonce);
        nonce
    }

    /// Seal a logical packet to **length-prefixed encrypted bytes** ready to send.
    pub fn seal_to_bytes(&self, packet: &CommunicationPacket) -> Result<Vec<u8>, String> {
        // Serialize the logical packet (payload+header)
        let serialized = serde_json::to_vec(packet)
            .map_err(|e| format!("serialize packet: {e}"))?;

        if serialized.len() > MAX_PACKET_SIZE {
            return Err(format!("Packet too large before encryption: {}", serialized.len()));
        }

        let nonce_arr = Self::gen_nonce();
        let nonce = Nonce::from_slice(&nonce_arr);

        let clear = ClearHeader {
            protocol_version: PROTOCOL_VERSION,
            packet_type: packet.header.packet_type,
            sequence_number: packet.header.sequence_number,
            source: packet.header.source,
            destination: packet.header.destination,
            key_id: self.key_id,
            nonce: nonce_arr,
        };

        let aad = serde_json::to_vec(&clear).map_err(|e| format!("serialize AAD: {e}"))?;

        let cipher = self.cipher();
        let ciphertext = cipher
            .encrypt(nonce, Payload { msg: &serialized, aad: &aad })
            .map_err(|_| "encryption failed".to_string())?;

        let frame = EncryptedFrame {
            header: clear,
            ciphertext,
        };

        // Length-prefixed JSON framing for the encrypted frame
        let frame_bytes =
            serde_json::to_vec(&frame).map_err(|e| format!("serialize frame: {e}"))?;

        if frame_bytes.len() > MAX_PACKET_SIZE {
            return Err(format!("Encrypted frame too large: {}", frame_bytes.len()));
        }

        let mut out = Vec::with_capacity(frame_bytes.len() + 4);
        out.extend_from_slice(&(frame_bytes.len() as u32).to_be_bytes());
        out.extend_from_slice(&frame_bytes);
        Ok(out)
    }

    /// Open **one complete frame** from a contiguous buffer (length-prefixed),
    /// returning the logical `CommunicationPacket`.
    pub fn open_from_bytes(&self, buf: &[u8]) -> Result<CommunicationPacket, String> {
        if buf.len() < 4 {
            return Err("insufficient data: need 4-byte length prefix".into());
        }
        let len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
        if buf.len() < 4 + len {
            return Err(format!(
                "insufficient data: expected {} bytes, got {}",
                4 + len,
                buf.len()
            ));
        }
        let json = &buf[4..4 + len];
        let frame: EncryptedFrame =
            serde_json::from_slice(json).map_err(|e| format!("frame deserialization: {e}"))?;

        if frame.header.key_id != self.key_id {
            return Err(format!(
                "key id mismatch: frame={}, ctx={}",
                frame.header.key_id, self.key_id
            ));
        }

        let aad = serde_json::to_vec(&frame.header)
            .map_err(|e| format!("AAD serialization: {e}"))?;
        let nonce = Nonce::from_slice(&frame.header.nonce);

        let cipher = self.cipher();
        let plaintext = cipher
            .decrypt(nonce, Payload { msg: &frame.ciphertext, aad: &aad })
            .map_err(|_| "authentication/decryption failed".to_string())?;

        let packet: CommunicationPacket = serde_json::from_slice(&plaintext)
            .map_err(|e| format!("packet deserialization: {e}"))?;

        // Optional: sanity checks (version, type, seq) vs clear header
        if packet.header.protocol_version != frame.header.protocol_version
            || packet.header.packet_type != frame.header.packet_type
            || packet.header.sequence_number != frame.header.sequence_number
            || packet.header.source != frame.header.source
            || packet.header.destination != frame.header.destination
        {
            return Err("header mismatch between clear header and decrypted packet".into());
        }

        Ok(packet)
    }
}

// ================================ Tests =====================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telemetry_roundtrip_encrypted() {
        // Build a telemetry packet
        let thermal = ThermalSensor::new(1, "CPU");
        let pkt = CommunicationPacket::new_telemetry(
            vec![thermal.create_reading(72.5, 10)],
            Source::Satellite,
        );

        // Crypto
        let key = [7u8; 32];
        let crypto = CryptoContext::new(1, key);

        // Seal → bytes → open
        let bytes = crypto.seal_to_bytes(&pkt).expect("seal");
        let back = crypto.open_from_bytes(&bytes).expect("open");

        assert_eq!(back.header.packet_type, PacketType::Telemetry);
        assert_eq!(back.header.source, Source::Satellite);
        assert_eq!(back.header.destination, Source::GroundControl);
        match back.payload {
            PacketPayload::TelemetryData(v) => assert_eq!(v.len(), 1),
            _ => panic!("wrong payload"),
        }
    }

    #[test]
    fn command_roundtrip_encrypted() {
        let cmd = Command::thermal_normal_operation(1);
        let pkt = CommunicationPacket::new_command(cmd, Source::GroundControl);

        let crypto = CryptoContext::new(42, [9u8; 32]);
        let bytes = crypto.seal_to_bytes(&pkt).unwrap();
        let back = crypto.open_from_bytes(&bytes).unwrap();

        assert!(matches!(back.header.packet_type, PacketType::Command));
        assert_eq!(back.header.source, Source::GroundControl);
        assert_eq!(back.header.destination, Source::Satellite);
    }
}
