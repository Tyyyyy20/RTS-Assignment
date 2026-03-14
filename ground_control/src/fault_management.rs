//fault.rs
use std::collections::HashMap;
use chrono::{DateTime, Utc};
use tracing::{info, warn, error};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use rand::Rng;
use shared_protocol::{EmergencyData, CommunicationPacket, Severity as NetSeverity};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FaultType {
    ThermalAnomaly,
    PowerAnomaly,
    AttitudeAnomaly,
    NetworkError,
    TelemetryError,
    SystemOverload,
    CommunicationLoss,
    SensorFailure,
    CommandRejection,
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum Severity {
    Critical = 0,
    High = 1,
    Medium = 2,
    Low = 3,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FaultEvent {
    pub timestamp: DateTime<Utc>,
    pub fault_type: FaultType,
    pub severity: Severity,
    pub description: String,
    pub affected_systems: Vec<String>,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct FaultResponse {
    pub fault_id: String,
    pub response_timestamp: DateTime<Utc>,
    pub response_time_ms: f64,
    pub recommended_action: Option<String>,
    pub safety_interlocks_triggered: Vec<String>,
    pub commands_blocked: Vec<String>,
    pub auto_recovery_attempted: bool,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
struct ActiveFault {
    fault_event: FaultEvent,
    fault_id: String,
    detected_at: DateTime<Utc>,
    last_occurrence: DateTime<Utc>,
    occurrence_count: u32,
    response_time_ms: Option<f64>,
    is_resolved: bool,
    resolution_time: Option<DateTime<Utc>>,
    blocked_commands: Vec<String>,
    auto_recovery_attempted: bool,
    recovery_mode: Option<RecoveryMode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RecoveryMode {
    SoftReset,
    SafeMode,
    Cooldown,
    PowerSave,
    AttitudeHold,
    LinkFallback,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
struct SafetyInterlock {
    interlock_id: String,
    fault_types: Vec<FaultType>,
    blocked_command_types: Vec<String>,
    blocked_systems: Vec<String>,
    activated_at: DateTime<Utc>,
    activation_reason: String,
    fault_id: Option<String>,
    fault_detected_at: Option<DateTime<Utc>>,
    activation_latency_ms: f64,
    released_at: Option<DateTime<Utc>>,
}

#[derive(Debug)]
pub struct FaultManager {
    active_faults: HashMap<String, ActiveFault>,
    safety_interlocks: HashMap<String, SafetyInterlock>,
    fault_history: Vec<ActiveFault>,
    consecutive_network_failures: u32,
    last_successful_communication: Option<DateTime<Utc>>,

    loss_of_contact_threshold: u32,
    critical_response_time_ms: f64,

    total_faults_detected: u64,
    total_critical_alerts: u64,
    total_response_time_critical_alerts: u64,
    total_interlocks_activated: u64,
    avg_fault_response_time_ms: f64,

    command_block_events: Vec<CommandBlockEvent>,
    interlock_latency_threshold_ms: f64,
    max_acceptable_block_latency_ms: f64,

    mttr_samples_ms: Vec<f64>,
    mtbf_samples_ms: Vec<f64>,
    last_any_fault_detected_at: Option<DateTime<Utc>>,
    auto_recovery_attempts: u64,
    auto_recovery_successes: u64,
    manual_recoveries: u64,

    interlock_total_active_ms: f64,
    interlock_releases: u64,

    loc_active_since: Option<DateTime<Utc>>,
    loc_events: u32,
    loc_total_duration_ms: f64,
}

pub struct FaultSimulator {
    simulation_counter: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandBlockEvent {
    pub command_id: String,
    pub command_type: String,
    pub target_system: String,
    pub fault_detection_time: DateTime<Utc>,
    pub interlock_activation_time: DateTime<Utc>,
    pub command_block_time: DateTime<Utc>,
    pub blocking_interlock_id: String,
    pub fault_id: String,

    pub fault_to_interlock_latency_ms: f64,
    pub interlock_to_block_latency_ms: f64,
    pub total_fault_to_block_latency_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RecoveryOutcome {
    AutoSuccess(Option<RecoveryMode>),
    AutoFailedThenManual,
    Manual,
}

impl FaultManager {
    pub fn new() -> Self {
        info!("Starting Fault Manager Subsystem");

        // Ensure rejected-ops audit CSV exists from startup, even before first block event.
        Self::initialize_rejected_operations_csv();

        Self {
            active_faults: HashMap::new(),
            safety_interlocks: HashMap::new(),
            fault_history: Vec::new(),
            consecutive_network_failures: 0,
            last_successful_communication: Some(Utc::now()),

            loss_of_contact_threshold: 3,
            critical_response_time_ms: 100.0,

            total_faults_detected: 0,
            total_critical_alerts: 0,
            total_response_time_critical_alerts: 0,
            total_interlocks_activated: 0,
            avg_fault_response_time_ms: 0.0,

            command_block_events: Vec::new(),
            interlock_latency_threshold_ms: 10.0,
            max_acceptable_block_latency_ms: 5.0,

            mttr_samples_ms: Vec::with_capacity(200),
            mtbf_samples_ms: Vec::with_capacity(200),
            last_any_fault_detected_at: None,
            auto_recovery_attempts: 0,
            auto_recovery_successes: 0,
            manual_recoveries: 0,
            interlock_total_active_ms: 0.0,
            interlock_releases: 0,
            loc_active_since: None,
            loc_events: 0,
            loc_total_duration_ms: 0.0,
        }
    }

    pub async fn handle_fault(&mut self, fault_event: FaultEvent) -> Result<FaultResponse> {
        let handler_started_at = std::time::Instant::now();
        let response_timestamp = Utc::now();

        info!("Fault Intake Received: {:?} | {}", fault_event.fault_type, fault_event.description);

        // Deterministic test hook for requirement evidence: this marker forces
        // fault handling over 100ms so critical ground-alert logic is exercised.
        if fault_event.description.contains("SIM_CRITICAL_GROUND_ALERT_100MS") {
            tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        }

        let fault_id = self.generate_fault_id(&fault_event);

        let fault_already_active = self.active_faults.contains_key(&fault_id);

        let mut response = FaultResponse {
            fault_id: fault_id.clone(),
            response_timestamp,
            response_time_ms: 0.0,
            recommended_action: None,
            safety_interlocks_triggered: Vec::new(),
            commands_blocked: Vec::new(),
            auto_recovery_attempted: false,
        };

        if fault_already_active {
            if let Some(active_fault) = self.active_faults.get_mut(&fault_id) {
                active_fault.last_occurrence = fault_event.timestamp;
                active_fault.occurrence_count += 1;

                warn!("Recurring Fault Observed: {} (Count #{})", fault_id, active_fault.occurrence_count);

                if active_fault.occurrence_count >= 3 {
                    response.recommended_action = Some("Escalate To Critical - Repeated Fault Pattern".to_string());
                }
            }
        } else {
            let detected_timestamp = fault_event.timestamp;

            if let Some(prev) = self.last_any_fault_detected_at {
                let gap_ms = (detected_timestamp - prev)
                    .num_microseconds()
                    .unwrap_or(0) as f64 / 1000.0;
                Self::push_sample_bounded(&mut self.mtbf_samples_ms, gap_ms, 200);
            }
            self.last_any_fault_detected_at = Some(detected_timestamp);

            let active_fault = ActiveFault {
                fault_event: fault_event.clone(),
                fault_id: fault_id.clone(),
                detected_at: detected_timestamp,
                last_occurrence: detected_timestamp,
                occurrence_count: 1,
                response_time_ms: None,
                is_resolved: false,
                resolution_time: None,
                blocked_commands: Vec::new(),
                auto_recovery_attempted: false,
                recovery_mode: None,
            };

            self.active_faults.insert(fault_id.clone(), active_fault);
            self.total_faults_detected += 1;
        }

        match fault_event.fault_type {
            FaultType::NetworkError | FaultType::CommunicationLoss => {
                self.consecutive_network_failures += 1;
                response = self.handle_network_fault(fault_event.clone(), response, &fault_id).await?;
            }
            FaultType::ThermalAnomaly => {
                response = self.handle_thermal_fault(fault_event.clone(), response, &fault_id).await?;
            }
            FaultType::PowerAnomaly => {
                response = self.handle_power_fault(fault_event.clone(), response, &fault_id).await?;
            }
            FaultType::AttitudeAnomaly => {
                response = self.handle_attitude_fault(fault_event.clone(), response, &fault_id).await?;
            }
            FaultType::SystemOverload => {
                response = self.handle_system_overload(fault_event.clone(), response, &fault_id).await?;
            }
            _ => {
                response = self.handle_generic_fault(fault_event.clone(), response, &fault_id).await?;
            }
        }

        let response_time = handler_started_at.elapsed().as_secs_f64() * 1000.0;
        response.response_time_ms = response_time;

        if let Some(active_fault) = self.active_faults.get_mut(&fault_id) {
            active_fault.response_time_ms = Some(response_time);
            active_fault.blocked_commands = response.commands_blocked.clone();
        }

        if response_time > self.critical_response_time_ms {
            error!("Fault Response Window Exceeded {}ms: {:.3}ms",
                self.critical_response_time_ms, response_time);
            self.total_response_time_critical_alerts += 1;
            self.trigger_critical_ground_alert(&fault_event, response_time).await?;
        }

        self.update_response_time_stats(response_time);

        if fault_event.severity <= Severity::High {
            self.total_critical_alerts += 1;
        }

        info!("Fault {} Processed In {:.3}ms", fault_id, response_time);
        Ok(response)
    }

    /// Returns whether the GCS is currently in an active loss-of-contact episode.
    pub fn has_active_loss_of_contact(&self) -> bool {
        self.loc_active_since.is_some()
    }

    pub async fn handle_loss_of_contact(&mut self) -> Result<FaultResponse> {
        // Guard: only declare LOC once per episode. Subsequent timeouts while
        // already in LOC mode are silent — the interlock is already active.
        if self.loc_active_since.is_some() {
            return Ok(FaultResponse {
                fault_id: "loss_of_contact_ongoing".to_string(),
                response_timestamp: Utc::now(),
                response_time_ms: 0.0,
                recommended_action: None,
                safety_interlocks_triggered: vec![],
                commands_blocked: vec![],
                auto_recovery_attempted: false,
            });
        }

        error!("LOSS OF CONTACT DETECTED - Engaging Emergency Procedures ({} Consecutive Failures)",
            self.consecutive_network_failures);

        let fault_event = FaultEvent {
            timestamp: Utc::now(),
            fault_type: FaultType::CommunicationLoss,
            severity: Severity::Critical,
            description: format!("Loss Of Contact Detected - {} Consecutive Network Failures",
                self.consecutive_network_failures),
            affected_systems: vec!["communication".to_string(), "all_systems".to_string()],
        };

        let fault_id = self.generate_fault_id(&fault_event);

        self.activate_safety_interlock(
            "emergency_loss_of_contact".to_string(),
            vec![FaultType::CommunicationLoss, FaultType::NetworkError],
            vec!["non_essential".to_string(), "experimental".to_string(), "high_power".to_string()],
            vec!["all_non_critical".to_string()],
            "Emergency: Complete Loss Of Satellite Contact".to_string(),
            Some(fault_id.clone()),
            Some(fault_event.timestamp),
        );

        self.loc_active_since = Some(Utc::now());
        self.loc_events += 1;
        self.auto_recovery_attempts += 1;

        Ok(FaultResponse {
            fault_id: "loss_of_contact_emergency".to_string(),
            response_timestamp: Utc::now(),
            response_time_ms: 0.0,
            recommended_action: Some("EMERGENCY: Loss Of Contact - Execute Backup Procedures".to_string()),
            safety_interlocks_triggered: vec!["emergency_loss_of_contact".to_string()],
            commands_blocked: vec![
                "all_non_essential".to_string(),
                "experimental_mode".to_string(),
                "high_power_operations".to_string(),
            ],
            auto_recovery_attempted: true,
        })
    }

    pub fn has_loss_of_contact_condition(&self) -> bool {
        self.consecutive_network_failures >= self.loss_of_contact_threshold
    }

    async fn handle_network_fault(
        &mut self,
        fault_event: FaultEvent,
        mut response: FaultResponse,
        fault_id: &str
    ) -> Result<FaultResponse> {
        if self.consecutive_network_failures >= self.loss_of_contact_threshold {
            // Only activate the interlock once – if LOC is already declared
            // (loc_active_since is Some) the emergency_loss_of_contact interlock
            // was already installed by handle_loss_of_contact; don't duplicate.
            if self.loc_active_since.is_none() {
                error!("LOSS OF CONTACT DETECTED - {} Consecutive Failures",
                    self.consecutive_network_failures);

                let interlock_id = "emergency_comm_loss".to_string();
                self.activate_safety_interlock(
                    interlock_id.clone(),
                    vec![FaultType::NetworkError, FaultType::CommunicationLoss],
                    vec!["non_essential".to_string(), "experimental".to_string()],
                    vec!["instruments".to_string(), "payload".to_string()],
                    "Communication Loss Condition Detected".to_string(),
                    Some(fault_id.to_string()),
                    Some(fault_event.timestamp),
                );
                response.safety_interlocks_triggered.push(interlock_id);
                response.commands_blocked.extend(vec![
                    "payload_activation".to_string(),
                    "experimental_mode".to_string(),
                    "non_critical_systems".to_string(),
                ]);
                response.auto_recovery_attempted = true;
                if let Some(af) = self.active_faults.get_mut(fault_id) {
                    af.auto_recovery_attempted = true;
                }
                self.auto_recovery_attempts += 1;
            }
            response.recommended_action = Some("LOSS OF CONTACT - Emergency Procedures Active".to_string());
        } else {
            response.recommended_action = Some(format!(
                "Network Instability Detected ({}/{} Failures) - Continue Communication Monitoring",
                self.consecutive_network_failures, self.loss_of_contact_threshold
            ));
        }
        Ok(response)
    }

    async fn handle_thermal_fault(
        &mut self,
        fault_event: FaultEvent,
        mut response: FaultResponse,
        fault_id: &str
    ) -> Result<FaultResponse> {

        match fault_event.severity {
            Severity::Critical => {
                response.recommended_action = Some("THERMAL EMERGENCY - Engage Cooling Systems".to_string());
                let interlock_id = "thermal_emergency".to_string();
                self.activate_safety_interlock(
                    interlock_id.clone(),
                    vec![FaultType::ThermalAnomaly],
                    vec!["power_intensive".to_string(), "heating".to_string()],
                    vec!["thermal_management".to_string()],
                    format!("Critical Thermal Condition Detected: {}", fault_event.description),
                    Some(fault_id.to_string()),
                    Some(fault_event.timestamp),
                );

                response.safety_interlocks_triggered.push(interlock_id);
                response.commands_blocked.extend(vec![
                    "payload_high_power".to_string(),
                    "transmitter_high_power".to_string(),
                    "heater_activation".to_string(),
                ]);

                response.auto_recovery_attempted = true;
                if let Some(af) = self.active_faults.get_mut(fault_id) {
                    af.auto_recovery_attempted = true;
                }
                self.auto_recovery_attempts += 1;
            }

            Severity::High => {
                response.recommended_action = Some("Thermal Warning - Increase Monitoring And Cooling".to_string());
                response.commands_blocked.extend(vec![
                    "heater_activation".to_string(),
                    "cpu_intensive_tasks".to_string(),
                ]);
            }

            _ => {
                response.recommended_action = Some("Thermal Anomaly Detected - Track Temperature Trends".to_string());
            }
        }
        Ok(response)
    }

    async fn handle_power_fault(
        &mut self,
        fault_event: FaultEvent,
        mut response: FaultResponse,
        fault_id: &str
    ) -> Result<FaultResponse> {
        match fault_event.severity {
            Severity::Critical => {
                response.recommended_action = Some("POWER CRITICAL - Enter Power Conservation Mode".to_string());
                let interlock_id = "power_conservation".to_string();
                self.activate_safety_interlock(
                    interlock_id.clone(),
                    vec![FaultType::PowerAnomaly],
                    vec!["high_power".to_string(), "non_essential".to_string()],
                    vec!["power_management".to_string()],
                    format!("Critical Power Condition Detected: {}", fault_event.description),
                    Some(fault_id.to_string()),
                    Some(fault_event.timestamp),
                );

                response.safety_interlocks_triggered.push(interlock_id);
                response.commands_blocked.extend(vec![
                    "payload_activation".to_string(),
                    "transmitter_high_power".to_string(),
                    "attitude_control_intensive".to_string(),
                    "heating_systems".to_string(),
                ]);

                response.auto_recovery_attempted = true;
                if let Some(af) = self.active_faults.get_mut(fault_id) {
                    af.auto_recovery_attempted = true;
                }
                self.auto_recovery_attempts += 1;
            }

            Severity::High => {
                response.recommended_action = Some("Power Warning - Reduce Non-Essential Systems".to_string());
                response.commands_blocked.extend(vec![
                    "payload_high_power".to_string(),
                    "experimental_mode".to_string(),
                ]);
            }

            _ => {
                response.recommended_action = Some("Power Anomaly Detected - Monitor Battery And Consumption".to_string());
            }
        }
        Ok(response)
    }

    async fn handle_attitude_fault(
        &mut self,
        fault_event: FaultEvent,
        mut response: FaultResponse,
        fault_id: &str
    ) -> Result<FaultResponse> {
        match fault_event.severity {
            Severity::Critical => {
                response.recommended_action = Some("ATTITUDE CRITICAL - Stabilization Required".to_string());
                let interlock_id = "attitude_stabilization".to_string();
                self.activate_safety_interlock(
                    interlock_id.clone(),
                    vec![FaultType::AttitudeAnomaly],
                    vec!["pointing_required".to_string(), "precise_maneuver".to_string()],
                    vec!["attitude_control".to_string()],
                    format!("Critical Attitude Error Detected: {}", fault_event.description),
                    Some(fault_id.to_string()),
                    Some(fault_event.timestamp),
                );

                response.safety_interlocks_triggered.push(interlock_id);
                response.commands_blocked.extend(vec![
                    "earth_observation".to_string(),
                    "antenna_pointing".to_string(),
                    "solar_panel_tracking".to_string(),
                    "precision_maneuvers".to_string(),
                ]);

                response.auto_recovery_attempted = true;
                if let Some(af) = self.active_faults.get_mut(fault_id) {
                    af.auto_recovery_attempted = true;
                }
                self.auto_recovery_attempts += 1;
            }

            Severity::High => {
                response.recommended_action = Some("Attitude Warning - Verify Control System".to_string());
                response.commands_blocked.extend(vec![
                    "precision_pointing".to_string(),
                    "complex_maneuvers".to_string(),
                ]);
            }

            _ => {
                response.recommended_action = Some("Attitude Anomaly Detected - Monitor Stability".to_string());
            }
        }
        Ok(response)
    }

    async fn handle_system_overload(
        &mut self,
        _fault_event: FaultEvent,
        mut response: FaultResponse,
        fault_id: &str
    ) -> Result<FaultResponse> {
        response.recommended_action = Some("System Overload Detected - Reduce Computational Load".to_string());
        response.commands_blocked.extend(vec![
            "data_processing_intensive".to_string(),
            "multiple_simultaneous_operations".to_string(),
            "background_tasks".to_string(),
        ]);

        let interlock_id = "system_overload".to_string();
        self.activate_safety_interlock(
            interlock_id.clone(),
            vec![FaultType::SystemOverload],
            vec!["cpu_intensive".to_string(), "memory_intensive".to_string()],
            vec!["processing".to_string()],
            "System Resource Overload Detected".to_string(),
            Some(fault_id.to_string()),
            Some(Utc::now()),
        );
        response.safety_interlocks_triggered.push(interlock_id);
        response.auto_recovery_attempted = true;
        if let Some(af) = self.active_faults.get_mut(fault_id) {
            af.auto_recovery_attempted = true;
        }
        self.auto_recovery_attempts += 1;

        Ok(response)
    }

    async fn handle_generic_fault(
        &mut self,
        fault_event: FaultEvent,
        mut response: FaultResponse,
        _fault_id: &str
    ) -> Result<FaultResponse> {
        response.recommended_action = Some(format!(
            "Generic Fault Handling Path For {:?}: {}",
            fault_event.fault_type, fault_event.description
        ));

        if fault_event.severity <= Severity::High {
            response.commands_blocked.extend(vec![
                "experimental_operations".to_string(),
                "non_essential_systems".to_string(),
            ]);
        }
        Ok(response)
    }

    fn activate_safety_interlock(
        &mut self,
        interlock_id: String,
        fault_types: Vec<FaultType>,
        blocked_command_types: Vec<String>,
        blocked_systems: Vec<String>,
        activation_reason: String,
        fault_id: Option<String>,
        fault_detected_at: Option<DateTime<Utc>>,
    ) {
        let activation_time = Utc::now();
        let activation_latency_ms = if let Some(detected_at) = fault_detected_at {
            (activation_time - detected_at).num_microseconds().unwrap_or(0) as f64 / 1000.0
        } else { 0.0 };

        info!("Activating Safety Interlock: {} - {} (Latency: {:.3}ms)",
            interlock_id, activation_reason, activation_latency_ms);

        let interlock = SafetyInterlock {
            interlock_id: interlock_id.clone(),
            fault_types,
            blocked_command_types,
            blocked_systems,
            activated_at: activation_time,
            activation_reason,
            fault_id,
            fault_detected_at,
            activation_latency_ms,
            released_at: None,
        };

        self.safety_interlocks.insert(interlock_id, interlock);
        self.total_interlocks_activated += 1;
    }

    pub fn is_command_blocked(
        &mut self,
        command_type: &str,
        target_system: &str,
        command_id: &str,
        command_submission_time: DateTime<Utc>
    ) -> (bool, Vec<String>, Option<CommandBlockEvent>) {
        let block_check_time = command_submission_time;
        let mut blocking_reasons = Vec::new();
        let mut block_event: Option<CommandBlockEvent> = None;

        for (interlock_id, interlock) in &self.safety_interlocks {
            let is_blocked_by_command_type = interlock.blocked_command_types.iter().any(|blocked|
                command_type == *blocked || command_type.starts_with(&format!("{}_", blocked))
            );
            let is_blocked_by_system = interlock.blocked_systems.iter().any(|blocked|
                target_system == *blocked || target_system.starts_with(&format!("{}_", blocked))
            );

            if is_blocked_by_command_type || is_blocked_by_system {
                blocking_reasons.push(format!("Command '{}' blocked by interlock: {}",
                    command_type, interlock_id));

                if block_event.is_none() {
                    let fault_detection_time = interlock.fault_detected_at.or_else(|| {
                        interlock.fault_id.as_ref().and_then(|id| self.active_faults.get(id).map(|f| f.detected_at))
                    }).unwrap_or(interlock.activated_at);

                    let fault_to_interlock_latency =
                        (interlock.activated_at - fault_detection_time).num_microseconds().unwrap_or(0) as f64 / 1000.0;
                    let interlock_to_block_latency =
                        (block_check_time - interlock.activated_at).num_microseconds().unwrap_or(0) as f64 / 1000.0;
                    let total_latency = fault_to_interlock_latency + interlock_to_block_latency;

                    block_event = Some(CommandBlockEvent {
                        command_id: command_id.to_string(),
                        command_type: command_type.to_string(),
                        target_system: target_system.to_string(),
                        fault_detection_time,
                        interlock_activation_time: interlock.activated_at,
                        command_block_time: block_check_time,
                        blocking_interlock_id: interlock_id.clone(),
                        fault_id: interlock.fault_id.clone().unwrap_or("unknown".to_string()),
                        fault_to_interlock_latency_ms: fault_to_interlock_latency,
                        interlock_to_block_latency_ms: interlock_to_block_latency,
                        total_fault_to_block_latency_ms: total_latency,
                    });

                    info!(
                        "Command {} blocked by interlock {} (F→I {:.3}ms, I→B {:.3}ms, total {:.3}ms)",
                        command_id, interlock_id, fault_to_interlock_latency, interlock_to_block_latency, total_latency
                    );

                    if total_latency > self.interlock_latency_threshold_ms {
                        warn!(
                            "INTERLOCK LATENCY ELEVATED: cmd={} interlock={} total={:.3}ms (threshold {:.1}ms)",
                            command_id, interlock_id, total_latency, self.interlock_latency_threshold_ms
                        );
                    }
                }
            }
        }

        (!blocking_reasons.is_empty(), blocking_reasons, block_event)
    }

    pub fn record_command_block_event(&mut self, block_event: CommandBlockEvent) {
        info!(
            "Recorded block: {} via {} (total {:.3}ms, F→I {:.3}ms, I→B {:.3}ms)",
            block_event.command_id,
            block_event.blocking_interlock_id,
            block_event.total_fault_to_block_latency_ms,
            block_event.fault_to_interlock_latency_ms,
            block_event.interlock_to_block_latency_ms
        );

        if block_event.total_fault_to_block_latency_ms > self.interlock_latency_threshold_ms {
            warn!("INTERLOCK LATENCY VIOLATION: Command {} Blocked After {:.3}ms (Threshold: {:.1}ms)",
                block_event.command_id,
                block_event.total_fault_to_block_latency_ms,
                self.interlock_latency_threshold_ms);
        }
        if block_event.fault_to_interlock_latency_ms > self.max_acceptable_block_latency_ms {
            warn!("SLOW INTERLOCK ACTIVATION DETECTED: {:.3}ms (Threshold: {:.1}ms)",
                block_event.fault_to_interlock_latency_ms,
                self.max_acceptable_block_latency_ms);
        }

        // Persist rejected operation to CSV log
        Self::append_rejected_operation_to_csv(&block_event);

        self.command_block_events.push(block_event);
        if self.command_block_events.len() > 500 {
            self.command_block_events.remove(0);
        }
    }

    /// Write a rejected operation entry to logs/ground_control_rejected_ops.csv for persistent audit trail.
    fn initialize_rejected_operations_csv() {
        use std::fs::{self, OpenOptions};
        use std::io::Write;

        let _ = fs::create_dir_all("logs");
        let output_path = "logs/ground_control_rejected_ops.csv";

        if !std::path::Path::new(output_path).exists() {
            match OpenOptions::new().create(true).append(true).open(output_path) {
                Ok(mut csv_file) => {
                    let _ = writeln!(
                        csv_file,
                        "ts,command_id,command_type,target_system,blocking_interlock_id,fault_id,\
                         fault_to_interlock_ms,interlock_to_block_ms,total_latency_ms"
                    );
                }
                Err(write_error) => {
                    warn!("Unable To Initialize ground_control_rejected_ops.csv: {}", write_error);
                }
            }
        }
    }

    /// Write a rejected operation entry to logs/ground_control_rejected_ops.csv for persistent audit trail.
    fn append_rejected_operation_to_csv(event: &CommandBlockEvent) {
        use std::io::Write;
        use std::fs::{self, OpenOptions};

        let _ = fs::create_dir_all("logs");
        let output_path = "logs/ground_control_rejected_ops.csv";
        let should_write_header = !std::path::Path::new(output_path).exists();

        match OpenOptions::new().create(true).append(true).open(output_path) {
            Ok(mut csv_file) => {
                if should_write_header {
                    let _ = writeln!(csv_file,
                        "ts,command_id,command_type,target_system,blocking_interlock_id,fault_id,\
                         fault_to_interlock_ms,interlock_to_block_ms,total_latency_ms");
                }
                let event_timestamp = Utc::now().to_rfc3339();
                let _ = writeln!(csv_file,
                    "{},{},{},{},{},{},{:.3},{:.3},{:.3}",
                    event_timestamp,
                    event.command_id,
                    event.command_type,
                    event.target_system,
                    event.blocking_interlock_id,
                    event.fault_id,
                    event.fault_to_interlock_latency_ms,
                    event.interlock_to_block_latency_ms,
                    event.total_fault_to_block_latency_ms,
                );
            }
            Err(write_error) => {
                warn!("Unable To Append ground_control_rejected_ops.csv: {}", write_error);
            }
        }
    }

    pub fn resolve_fault_with_outcome(&mut self, fault_id: &str, outcome: RecoveryOutcome) -> Result<()> {
        if let Some(active_fault) = self.active_faults.get_mut(fault_id) {
            active_fault.is_resolved = true;
            let now = Utc::now();
            active_fault.resolution_time = Some(now);

            if let Some(dt_ms) = active_fault.resolution_time.map(|r| {
                (r - active_fault.detected_at).num_microseconds().unwrap_or(0) as f64 / 1000.0
            }) {
                Self::push_sample_bounded(&mut self.mttr_samples_ms, dt_ms, 200);
            }

            match outcome {
                RecoveryOutcome::AutoSuccess(mode) => {
                    self.auto_recovery_successes += 1;
                    if let Some(m) = mode {
                        active_fault.recovery_mode = Some(m);
                    }
                }
                RecoveryOutcome::AutoFailedThenManual | RecoveryOutcome::Manual => {
                    self.manual_recoveries += 1;
                }
            }

            let snapshot = active_fault.clone();
            self.fault_history.push(snapshot);
            self.active_faults.remove(fault_id);
            self.reconcile_safety_interlocks();
            Ok(())
        } else {
            Err(anyhow::anyhow!("Fault {} not found in active faults", fault_id))
        }
    }

    fn reconcile_safety_interlocks(&mut self) {
        let mut interlocks_to_remove = Vec::new();

        for (interlock_id, interlock) in &self.safety_interlocks {
            let still_relevant = interlock.fault_types.iter().any(|fault_type| {
                self.active_faults.values().any(|active_fault| {
                    active_fault.fault_event.fault_type == *fault_type && !active_fault.is_resolved
                })
            });

            if !still_relevant {
                interlocks_to_remove.push(interlock_id.clone());
            }
        }

        for interlock_id in interlocks_to_remove {
            if let Some(mut interlock) = self.safety_interlocks.remove(&interlock_id) {
                let now = Utc::now();
                interlock.released_at = Some(now);
                let dur_ms = (now - interlock.activated_at).num_microseconds().unwrap_or(0) as f64 / 1000.0;

                self.interlock_total_active_ms += dur_ms;
                self.interlock_releases += 1;

                info!(
                    "Deactivated Interlock {} (Active {:.3} ms)", interlock_id, dur_ms
                );
            }
        }
    }

    async fn trigger_critical_ground_alert(&self, fault_event: &FaultEvent, response_time: f64) -> Result<()> {
        error!("CRITICAL ALERT: Fault Response Time Exceeded {}ms", self.critical_response_time_ms);
        error!("Fault Detail: {:?} - {} (Response Time: {:.3}ms)",
            fault_event.fault_type, fault_event.description, response_time);
        Ok(())
    }

    pub fn increment_consecutive_failures(&mut self) {
        self.consecutive_network_failures += 1;
        warn!("Consecutive Network Failure #{} - Current Streak: {}",
            self.consecutive_network_failures, self.consecutive_network_failures);

        if self.consecutive_network_failures == self.loss_of_contact_threshold - 1 {
            warn!("WARNING: One Additional Failure Will Trigger Loss Of Contact!");
        }
    }

    pub fn record_successful_communication(&mut self) {
        let prev = self.consecutive_network_failures;
        if prev > 0 {
            info!("Communications Restored After {} Consecutive Failures", prev);
            self.consecutive_network_failures = 0;
        }
        self.last_successful_communication = Some(Utc::now());

        if let Some(since) = self.loc_active_since.take() {
            let dur_ms = (Utc::now() - since).num_microseconds().unwrap_or(0) as f64 / 1000.0;
            self.loc_total_duration_ms += dur_ms;
        }

        // Auto-resolve any active network / communication-loss faults now that
        // contact is restored.
        if prev > 0 {
            let net_fault_ids: Vec<String> = self.active_faults
                .iter()
                .filter(|(_, f)| matches!(
                    f.fault_event.fault_type,
                    FaultType::NetworkError | FaultType::CommunicationLoss
                ))
                .map(|(id, _)| id.clone())
                .collect();

            for id in net_fault_ids {
                info!("Auto-Resolving Network Fault {} - Communications Restored", id);
                let _ = self.resolve_fault_with_outcome(
                    &id,
                    RecoveryOutcome::AutoSuccess(Some(RecoveryMode::LinkFallback)),
                );
            }
        }

        self.reconcile_safety_interlocks();
    }

    /// Sweep active faults and resolve any that have exceeded their age timeout.
    ///
    /// Timeouts:
    ///   Critical / High → 120 s
    ///   Medium          →  60 s
    ///   Low             →  30 s
    ///
    /// Active communication-loss faults that still have consecutive failures are
    /// *not* resolved here; those are handled by `record_successful_communication`.
    ///
    /// Returns how many faults were resolved.
    pub fn auto_resolve_stale_faults(&mut self) -> u32 {
        let now = Utc::now();
        let current_consec  = self.consecutive_network_failures;
        let loc_threshold   = self.loss_of_contact_threshold;

        let stale_ids: Vec<String> = self.active_faults
            .iter()
            .filter(|(_, f)| {
                // Never auto-resolve an ongoing comms-loss (needs contact-restore).
                if matches!(f.fault_event.fault_type, FaultType::CommunicationLoss)
                    && current_consec >= loc_threshold
                {
                    return false;
                }
                let age_secs = (now - f.detected_at).num_seconds();
                let timeout_secs: i64 = match f.fault_event.severity {
                    Severity::Critical | Severity::High => 120,
                    Severity::Medium                    =>  60,
                    Severity::Low                       =>  30,
                };
                age_secs > timeout_secs
            })
            .map(|(id, _)| id.clone())
            .collect();

        let resolved = stale_ids.len() as u32;
        for id in stale_ids {
            info!("Auto-Resolving Stale Fault {} (Age Timeout Triggered)", id);
            let _ = self.resolve_fault_with_outcome(&id, RecoveryOutcome::AutoSuccess(None));
        }
        resolved
    }

    fn generate_fault_id(&self, fault_event: &FaultEvent) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        fault_event.fault_type.hash(&mut hasher);
        fault_event.affected_systems.hash(&mut hasher);

        format!("{:?}_{:016x}", fault_event.fault_type, hasher.finish())
    }

    fn update_response_time_stats(&mut self, response_time: f64) {
        let total_responses = self.total_faults_detected as f64;
        if total_responses > 1.0 {
            self.avg_fault_response_time_ms =
                (self.avg_fault_response_time_ms * (total_responses - 1.0) + response_time) / total_responses;
        } else {
            self.avg_fault_response_time_ms = response_time;
        }
    }

    pub fn get_stats(&self) -> FaultManagerStats {
        let active_critical_faults = self.active_faults.values()
            .filter(|fault| fault.fault_event.severity <= Severity::High)
            .count() as u32;

        let avg_resolution_time = if !self.fault_history.is_empty() {
            let total_time: i64 = self.fault_history.iter()
                .filter_map(|fault| {
                    fault.resolution_time.map(|res_time| {
                        (res_time - fault.detected_at).num_milliseconds()
                    })
                })
                .sum();
            total_time as f64 / self.fault_history.len() as f64
        } else { 0.0 };

        let (interlock_avg_activation_latency_ms, interlock_max_activation_latency_ms) =
            if self.safety_interlocks.is_empty() { (0.0, 0.0) } else {
                let sum: f64 = self.safety_interlocks.values().map(|i| i.activation_latency_ms).sum();
                let max: f64 = self.safety_interlocks.values().map(|i| i.activation_latency_ms).fold(0.0, f64::max);
                (sum / self.safety_interlocks.len() as f64, max)
            };

        let recent_block_latencies: Vec<f64> = self.command_block_events.iter()
            .rev()
            .take(50)
            .map(|e| e.total_fault_to_block_latency_ms)
            .collect();

        let latency_violations = recent_block_latencies.iter()
            .filter(|&&latency| latency > self.interlock_latency_threshold_ms)
            .count() as u32;

        let mttr_avg_ms = Self::compute_average(&self.mttr_samples_ms);
        let mtbf_avg_ms = Self::compute_average(&self.mtbf_samples_ms);

        FaultManagerStats {
            total_faults_detected: self.total_faults_detected,
            active_faults_count: self.active_faults.len() as u32,
            active_critical_faults,
            total_critical_alerts: self.total_critical_alerts,
            response_time_critical_alerts: self.total_response_time_critical_alerts,
            total_interlocks_activated: self.total_interlocks_activated,
            active_interlocks_count: self.safety_interlocks.len() as u32,
            consecutive_network_failures: self.consecutive_network_failures,
            avg_fault_response_time_ms: self.avg_fault_response_time_ms,
            avg_fault_resolution_time_ms: avg_resolution_time,
            last_successful_communication: self.last_successful_communication,
            interlock_avg_activation_latency_ms,
            interlock_max_activation_latency_ms,
            recent_latency_violations: latency_violations,
            total_commands_blocked: self.command_block_events.len() as u64,

            mttr_avg_ms,
            mtbf_avg_ms,
            auto_recovery_attempts: self.auto_recovery_attempts,
            auto_recovery_successes: self.auto_recovery_successes,
            manual_recoveries: self.manual_recoveries,
            interlock_total_active_ms: self.interlock_total_active_ms,
            interlock_releases: self.interlock_releases,
            loc_events: self.loc_events,
            loc_total_duration_ms: self.loc_total_duration_ms,
        }
    }

    fn push_sample_bounded(buf: &mut Vec<f64>, v: f64, max_len: usize) {
        buf.push(v);
        if buf.len() > max_len { buf.remove(0); }
    }
    fn compute_average(xs: &[f64]) -> f64 {
        if xs.is_empty() { 0.0 } else { xs.iter().sum::<f64>() / xs.len() as f64 }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct FaultManagerStats {
    pub total_faults_detected: u64,
    pub active_faults_count: u32,
    pub active_critical_faults: u32,
    pub total_critical_alerts: u64,
    pub response_time_critical_alerts: u64,
    pub total_interlocks_activated: u64,
    pub active_interlocks_count: u32,
    pub consecutive_network_failures: u32,
    pub avg_fault_response_time_ms: f64,
    pub avg_fault_resolution_time_ms: f64,
    pub last_successful_communication: Option<DateTime<Utc>>,
    pub interlock_avg_activation_latency_ms: f64,
    pub interlock_max_activation_latency_ms: f64,
    pub recent_latency_violations: u32,
    pub total_commands_blocked: u64,

    pub mttr_avg_ms: f64,
    pub mtbf_avg_ms: f64,
    pub auto_recovery_attempts: u64,
    pub auto_recovery_successes: u64,
    pub manual_recoveries: u64,
    pub interlock_total_active_ms: f64,
    pub interlock_releases: u64,
    pub loc_events: u32,
    pub loc_total_duration_ms: f64,
}

impl std::hash::Hash for FaultType {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            FaultType::Unknown(s) => s.hash(state),
            _ => {}
        }
    }
}

impl FaultSimulator {
    pub fn new() -> Self { Self { simulation_counter: 0 } }

    pub fn create_thermal_fault(&mut self, severity: u8, temperature: f64) -> EmergencyData {
        self.simulation_counter += 1;

        let (severity_enum, description, actions) = match severity {
            0 => (NetSeverity::Critical,
                format!("CRITICAL: Thermal sensor reading {}°C exceeds emergency threshold (85°C)", temperature),
                vec![
                    "IMMEDIATE: Activate emergency cooling".to_string(),
                    "IMMEDIATE: Reduce power consumption by 50%".to_string(),
                    "IMMEDIATE: Prepare for emergency shutdown".to_string(),
                ]),
            1 => (NetSeverity::High,
                format!("HIGH: Thermal sensor reading {}°C exceeds critical threshold (80°C)", temperature),
                vec![
                    "Activate cooling systems".to_string(),
                    "Increase thermal monitoring frequency".to_string(),
                    "Reduce non-essential power consumption".to_string(),
                ]),
            2 => (NetSeverity::Medium,
                format!("MEDIUM: Thermal sensor reading {}°C approaching warning threshold", temperature),
                vec![
                    "Monitor thermal trends".to_string(),
                    "Check cooling system status".to_string(),
                ]),
            _ => (NetSeverity::Low,
                format!("LOW: Thermal sensor anomaly detected at {}°C", temperature),
                vec!["Continue monitoring".to_string()]),
        };

        EmergencyData {
            alert_id: format!("THERMAL_FAULT_{:04}", self.simulation_counter),
            severity: severity_enum,
            alert_type: "thermal".to_string(),
            description,
            affected_systems: vec!["thermal_management".to_string(), "power_management".to_string()],
            recommended_actions: actions,
            auto_recovery_attempted: severity <= 2,
            timestamp: Utc::now(),
        }
    }

    pub fn create_power_fault(&mut self, severity: u8, battery_level: f64) -> EmergencyData {
        self.simulation_counter += 1;

        let (severity_enum, description, actions) = match severity {
            0 => (NetSeverity::Critical,
                format!("CRITICAL: Battery level {}% below critical threshold (20%)", battery_level),
                vec![
                    "IMMEDIATE: Enter power conservation mode".to_string(),
                    "IMMEDIATE: Shutdown non-essential systems".to_string(),
                    "IMMEDIATE: Prepare for emergency protocols".to_string(),
                ]),
            1 => (NetSeverity::High,
                format!("HIGH: Battery level {}% below low threshold (30%)", battery_level),
                vec![
                    "Reduce power consumption".to_string(),
                    "Optimize power usage".to_string(),
                    "Monitor charging systems".to_string(),
                ]),
            2 => (NetSeverity::Medium,
                format!("MEDIUM: Power consumption anomaly detected, battery at {}%", battery_level),
                vec![
                    "Monitor power trends".to_string(),
                    "Check power systems".to_string(),
                ]),
            _ => (NetSeverity::Low,
                format!("LOW: Minor power fluctuation, battery at {}%", battery_level),
                vec!["Continue monitoring".to_string()]),
        };

        EmergencyData {
            alert_id: format!("POWER_FAULT_{:04}", self.simulation_counter),
            severity: severity_enum,
            alert_type: "power".to_string(),
            description,
            affected_systems: vec!["power_management".to_string(), "all_systems".to_string()],
            recommended_actions: actions,
            auto_recovery_attempted: severity <= 1,
            timestamp: Utc::now(),
        }
    }

    pub fn create_attitude_fault(&mut self, severity: u8, attitude_error: f64) -> EmergencyData {
        self.simulation_counter += 1;

        let (severity_enum, description, actions) = match severity {
            0 => (NetSeverity::Critical,
                format!("CRITICAL: Attitude error {:.1}° exceeds critical threshold (10°)", attitude_error),
                vec![
                    "IMMEDIATE: Activate reaction wheels".to_string(),
                    "IMMEDIATE: Fire attitude correction thrusters".to_string(),
                    "IMMEDIATE: Stabilize spacecraft orientation".to_string(),
                ]),
            1 => (NetSeverity::High,
                format!("HIGH: Attitude error {:.1}° exceeds acceptable threshold (5°)", attitude_error),
                vec![
                    "Initiate attitude correction".to_string(),
                    "Increase attitude monitoring frequency".to_string(),
                    "Check thruster systems".to_string(),
                ]),
            2 => (NetSeverity::Medium,
                format!("MEDIUM: Attitude drift detected, error {:.1}°", attitude_error),
                vec![
                    "Monitor attitude trends".to_string(),
                    "Check attitude sensors".to_string(),
                ]),
            _ => (NetSeverity::Low,
                format!("LOW: Minor attitude adjustment needed, error {:.1}°", attitude_error),
                vec!["Continue monitoring".to_string()]),
        };

        EmergencyData {
            alert_id: format!("ATTITUDE_FAULT_{:04}", self.simulation_counter),
            severity: severity_enum,
            alert_type: "attitude".to_string(),
            description,
            affected_systems: vec!["attitude_control".to_string()],
            recommended_actions: actions,
            auto_recovery_attempted: severity <= 2,
            timestamp: Utc::now(),
        }
    }

    pub fn create_communication_fault(&mut self, severity: u8, packets_lost: u32) -> EmergencyData {
        self.simulation_counter += 1;

        let (severity_enum, description, actions) = match severity {
            0 => (NetSeverity::Critical,
                format!("CRITICAL: Communication failure - {} consecutive packets lost", packets_lost),
                vec![
                    "IMMEDIATE: Switch to backup communication".to_string(),
                    "IMMEDIATE: Increase transmission power".to_string(),
                    "IMMEDIATE: Attempt emergency contact".to_string(),
                ]),
            1 => (NetSeverity::High,
                format!("HIGH: Communication degradation - {} packets lost", packets_lost),
                vec![
                    "Increase packet retransmission".to_string(),
                    "Check communication systems".to_string(),
                    "Adjust transmission parameters".to_string(),
                ]),
            2 => (NetSeverity::Medium,
                format!("MEDIUM: Intermittent communication issues - {} packet losses", packets_lost),
                vec![
                    "Monitor communication quality".to_string(),
                    "Check antenna orientation".to_string(),
                ]),
            _ => (NetSeverity::Low,
                format!("LOW: Minor communication anomaly - {} packet retransmissions", packets_lost),
                vec!["Continue monitoring".to_string()]),
        };

        EmergencyData {
            alert_id: format!("COMM_FAULT_{:04}", self.simulation_counter),
            severity: severity_enum,
            alert_type: "communication".to_string(),
            description,
            affected_systems: vec!["communication".to_string(), "network_management".to_string()],
            recommended_actions: actions,
            auto_recovery_attempted: severity >= 1,
            timestamp: Utc::now(),
        }
    }

    pub fn create_random_fault(&mut self) -> EmergencyData {
        let mut rng = rand::rng();

        match rng.random_range(0..4) {
            0 => {
                let temp = rng.random_range(60.0..=95.0);
                let severity = if temp > 85.0 { 0 } else if temp > 80.0 { 1 } else { 2 };
                self.create_thermal_fault(severity, temp)
            },
            1 => {
                let battery = rng.random_range(10.0..=40.0);
                let severity = if battery < 20.0 { 0 } else if battery < 30.0 { 1 } else { 2 };
                self.create_power_fault(severity, battery)
            },
            2 => {
                let error = rng.random_range(2.0..=15.0);
                let severity = if error > 10.0 { 0 } else if error > 5.0 { 1 } else { 2 };
                self.create_attitude_fault(severity, error)
            },
            _ => {
                let packets = rng.random_range(1..=20);
                let severity = if packets > 10 { 0 } else if packets > 5 { 1 } else { 2 };
                self.create_communication_fault(severity, packets)
            }
        }
    }

    pub fn create_fault_packet(&mut self, fault_data: EmergencyData) -> CommunicationPacket {
        CommunicationPacket::new_emergency(fault_data, shared_protocol::Source::GroundControl)
    }

}
