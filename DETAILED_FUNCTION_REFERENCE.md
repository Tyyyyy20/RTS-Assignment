# Ground Control Station — Complete Function Reference
## Indexed by File · Annotated with Assignment Requirements

---

## command.rs — Command Uplink Scheduler [REQ 2]

```
CommandScheduler::new()                          REQ 2.1 — init real-time queue
CommandScheduler::schedule_command()             REQ 2.1, 2.3 — validate + enqueue
CommandScheduler::process_pending_commands()     REQ 2.2, 2.4 — enforce ≤2ms, track violations
CommandScheduler::get_commands_approaching_deadline()  REQ 2.2 — monitor imminent deadlines
CommandScheduler::cleanup_expired_commands()     REQ 2.1 — purge stale commands
CommandScheduler::update_safety_validation_cache()     REQ 2.3 — future safety hook
CommandScheduler::get_enhanced_stats()           REQ 2.4 — queue/dispatch snapshot
CommandScheduler::get_unified_deadline_report()  REQ 2.4 — adherence rates, violations
CommandScheduler::calculate_performance_trend()  REQ 4.1 [private] — improving/degrading/stable
CommandScheduler::record_send_time()             REQ 4.1 [private] — bounded send-time history
CommandScheduler::get_queue_stats()              REQ 4.2 — urgent vs normal queue counts
```

### Function Details

| Function | Signature | Lines | Notes |
|---|---|---|---|
| `new` | `() -> Self` | 69 | VecDeque queue, zero counters, cap-1000 send-time history |
| `get_enhanced_stats` | `(&self) -> EnhancedCommandSchedulerStats` | 81 | queued, dispatched, urgent_dispatched, avg_send_time_ms |
| `get_unified_deadline_report` | `(&self) -> UnifiedDeadlineReport` | 96 | network_violations, deadline_violations, adherence_rate, network_adherence_rate, performance_trend |
| `calculate_performance_trend` | `(&self) -> String` [private] | 127 | Splits send_times into halves; "improving" / "degrading" / "stable" / "insufficient_data" |
| `record_send_time` | `(&mut self, f64)` [private] | 150 | Evicts oldest when len ≥ 1000 |
| `process_pending_commands` | `(&mut self, &mut FaultManager, &NetworkManager, Option<&mpsc::Sender>) -> Vec<Command>` [async] | 157 | Calls `send_packet_with_deadline_check`; tracks network_violations (>2ms), deadline_violations; re-queues failed commands by priority |
| `get_commands_approaching_deadline` | `(&self) -> Vec<DeadlineWarning>` | 247 | Returns time_to_deadline_ms for each queued command |
| `cleanup_expired_commands` | `(&mut self)` [async] | 269 | Retains only commands with deadline > now |
| `update_safety_validation_cache` | `(&mut self)` [async] | 283 | Reserved hook |
| `schedule_command` | `(&mut self, Command) -> Result<String>` | 288 | Validates: deadline not past, ID non-empty, no duplicate IDs; urgent → push_front, normal → push_back |
| `get_queue_stats` | `(&self) -> HashMap<String, u64>` | 326 | Keys: total_queued, urgent_queued, normal_queued, total_dispatched, urgent_dispatched |

**Key Structures**
- `EnhancedCommandSchedulerStats` — queued, dispatched, urgent_dispatched, avg_send_time_ms
- `DeadlineWarning` — command_id, command_type, priority, target_system, time_to_deadline_ms
- `UnifiedDeadlineReport` — total_urgent, network_violations, deadline_violations, avg_net_send, adherence_rate, network_adherence_rate, performance_trend
- `Scheduled` [private] — command, enqueued_at, retry_count

---

## telemetry.rs — Telemetry Reception and Decoding [REQ 1]

```
TelemetryProcessor::new()                        REQ 1.1 — init with sensor thresholds
TelemetryProcessor::process_packet()             REQ 1.1 (≤3ms) — main decode entry point
TelemetryProcessor::check_packet_sequence()      REQ 1.3 — detect missing packets
TelemetryProcessor::check_packet_delay()         REQ 1.2, 1.3 [private] — detect delayed packets
TelemetryProcessor::process_sensor_reading()     REQ 3.1 [async, private] — sensor fault detection
TelemetryProcessor::check_sensor_faults()        REQ 3.1 [private] — threshold comparison
TelemetryProcessor::take_unreported_delayed_packet_details()  REQ 1.2 — surface delayed packets
TelemetryProcessor::get_missing_packets()        REQ 1.3 — list IDs for re-request
TelemetryProcessor::mark_packet_re_requested()   REQ 1.3 — prevent duplicate re-requests
TelemetryProcessor::cleanup_old_missing_packets()  — memory management
TelemetryProcessor::cleanup_old_delayed_packets()  — memory management
TelemetryProcessor::calculate_thermal_trend()    [private, placeholder]
TelemetryProcessor::calculate_sensor_trend()     [private]
TelemetryProcessor::get_stats()                  REQ 4.2 — telemetry statistics

SensorThresholds::default()                      — thermal 80/85°C, power 30/20%, attitude 5/10°
SensorAnalysis::from_sensor_type()               — map type string to analysis struct
SensorAnalysis::from_emergency_type()            — map emergency type string
SensorAnalysis::from_emergency_severity()        — map NetSeverity to analysis
```

### Function Details

| Function | Signature | Lines | Notes |
|---|---|---|---|
| `new` | `() -> Self` | 135 | Initialises sequence tracker, missing/delayed lists, packet history, expected sensor configs |
| `process_packet` | `(&mut self, CommunicationPacket, DateTime<Utc>) -> Result<TelemetryProcessingResult>` [async] | 182 | Calls check_packet_sequence, check_packet_delay, process_sensor_reading; 3ms deadline enforced by caller |
| `check_packet_delay` | `(&mut self, &CommunicationPacket, DateTime<Utc>) -> Option<DelayedPacketInfo>` [private] | 327 | Compares reception_time vs expected from last packet + interval |
| `process_sensor_reading` | `(&self, &SensorReading, &SensorAnalysis) -> Option<FaultEvent>` [async, private] | 378 | Calls check_sensor_faults; returns FaultEvent if threshold breached |
| `check_packet_sequence` | `(&mut self, &CommunicationPacket) -> Result<()>` | 496 | Detects sequence gaps; records missing packet IDs |
| `take_unreported_delayed_packet_details` | `(&mut self) -> Vec<DelayedPacketResult>` | 526 | Returns and clears unreported delayed packets |
| `get_missing_packets` | `(&mut self) -> Vec<String>` | 546 | Returns IDs not yet re-requested |
| `mark_packet_re_requested` | `(&mut self, &str)` | 560 | Flags packet to prevent duplicate re-request |
| `cleanup_old_missing_packets` | `(&mut self)` | 570 | Removes records > 5 seconds old |
| `cleanup_old_delayed_packets` | `(&mut self)` | 584 | Removes records > 5 seconds old |
| `check_sensor_faults` | `(&self, &SensorReading, &SensorAnalysis) -> Option<FaultEvent>` [private] | 599 | Compares against SensorThresholds; returns typed FaultEvent |
| `calculate_thermal_trend` | `(&self, u32, f64) -> Option<TrendInfo>` [private] | 647 | Placeholder |
| `calculate_sensor_trend` | `(&self, &SensorReading) -> Option<TrendInfo>` [private] | 656 | General trend analysis |
| `get_stats` | `(&self) -> TelemetryStats` | 669 | total_packets, sensor_readings, processing_time, consecutive_failures, delayed_count |

**Default Sensor Thresholds** (`SensorThresholds::default`, line 75)

| Sensor | Warning | Critical |
|--|--|--|
| Thermal | — | 80°C |
| Thermal | — | 85°C (emergency) |
| Power | 30% | 20% |
| Attitude | 5° | 10° |

**Key Structures**
- `TelemetryProcessingResult` — packet_id, processing_time_ms, sensor_count, detected_faults, missing_packets_detected, delayed_packets_detected, sensor_analysis
- `DelayedPacketInfo` — packet_id, packet_type, expected_time, actual_delay_ms, detected_at, reported
- `PacketRecord` [private] — reception and processing history per packet
- `ExpectedSensorConfig` [private] — sensor_id, expected_interval_ms, last_reading_time, critical_thresholds
- `TelemetryStats` — aggregate statistics

---

## fault.rs — Fault Management and Interlocks [REQ 3, 1.4]

### FaultManager

```
FaultManager::new()                              — init with thresholds
FaultManager::handle_fault()                     REQ 3.1, 3.2, 3.3, 3.5 — main fault handler
FaultManager::handle_network_fault()             REQ 1.4 [async, private]
FaultManager::handle_thermal_fault()             REQ 3.2 [async, private]
FaultManager::handle_power_fault()               REQ 3.2 [async, private]
FaultManager::handle_attitude_fault()            REQ 3.2 [async, private]
FaultManager::handle_system_overload()           REQ 3.2 [async, private]
FaultManager::handle_generic_fault()             REQ 3.2 [async, private]
FaultManager::activate_safety_interlock()        REQ 3.2, 3.3 [private] — records activation_latency_ms
FaultManager::is_command_blocked()               REQ 2.3, 3.2, 3.3, 3.4 — interlock check per command
FaultManager::record_command_block_event()       REQ 3.3, 3.4 — persist latency breakdown
FaultManager::resolve_fault()                    REQ 3.2 — manual resolution shortcut
FaultManager::resolve_fault_with_outcome()       REQ 4.2 — MTTR, auto/manual categorisation
FaultManager::review_safety_interlocks()         REQ 3.2 [private] — release resolved interlocks
FaultManager::trigger_critical_ground_alert()    REQ 3.5 [async, private] — >100ms → error log
FaultManager::handle_loss_of_contact()           REQ 1.4 [async] — ≥3 failures → emergency
FaultManager::is_loss_of_contact()               REQ 1.4 — check threshold
FaultManager::increment_consecutive_failures()   REQ 1.4 — bump counter
FaultManager::get_consecutive_failures()         REQ 1.4 — read counter
FaultManager::record_successful_communication()  REQ 1.4 — reset counter, release interlocks
FaultManager::get_stats()                        REQ 4.2, S.4 — comprehensive stats
FaultManager::generate_fault_id()               [private] — hash-based deterministic ID
FaultManager::update_response_time_stats()       REQ 4.2 [private] — running avg
FaultManager::push_bounded()                    [private static] — cap-bounded Vec push
FaultManager::avg()                             [private static] — safe f64 average
```

### FaultManager Function Details

| Function | Signature | Notes |
|---|---|---|
| `new` | `() -> Self` | loss_of_contact_threshold=3, critical_response_time_ms=100, interlock_latency_threshold_ms=10, max_acceptable_block_latency_ms=5 |
| `handle_fault` | `(&mut self, FaultEvent) -> Result<FaultResponse>` [async] | Generates fault_id; handles recurring; dispatches to type handler; measures response_time; calls trigger_critical_ground_alert if >100ms; updates MTBF |
| `handle_network_fault` | `(&mut self, FaultEvent, FaultResponse, &str) -> Result<FaultResponse>` [async, private] | Activates `emergency_comm_loss` interlock; blocks payload/experimental if failures ≥ threshold |
| `handle_thermal_fault` | `(&mut self, FaultEvent, FaultResponse, &str) -> Result<FaultResponse>` [async, private] | Critical: `thermal_emergency` interlock, blocks high-power/heating. High: blocks heater/CPU. Medium/Low: monitoring recommendation |
| `handle_power_fault` | `(&mut self, FaultEvent, FaultResponse, &str) -> Result<FaultResponse>` [async, private] | Critical: `power_conservation` interlock, blocks payload/transmitter/attitude/heating. High/Low: partial blocks |
| `handle_attitude_fault` | `(&mut self, FaultEvent, FaultResponse, &str) -> Result<FaultResponse>` [async, private] | Critical: `attitude_stabilization` interlock, blocks pointing/maneuvers. High/Low: partial blocks |
| `handle_system_overload` | `(&mut self, FaultEvent, FaultResponse, &str) -> Result<FaultResponse>` [async, private] | `system_overload` interlock; blocks CPU/memory-intensive operations |
| `handle_generic_fault` | `(&mut self, FaultEvent, FaultResponse, &str) -> Result<FaultResponse>` [async, private] | Blocks experimental/non-essential for High/Critical severity |
| `activate_safety_interlock` | `(&mut self, String, Vec<FaultType>, Vec<String>, Vec<String>, String, Option<String>, Option<DateTime<Utc>>)` [private] | Records `activation_latency_ms` = activation_time − fault_detected_at; stores SafetyInterlock struct; increments total_interlocks_activated |
| `is_command_blocked` | `(&mut self, &str, &str, &str, DateTime<Utc>) -> (bool, Vec<String>, Option<CommandBlockEvent>)` | Iterates active interlocks; matches command_type and target_system; builds CommandBlockEvent with full latency chain |
| `record_command_block_event` | `(&mut self, CommandBlockEvent)` | Persists (cap 500); logs INTERLOCK LATENCY VIOLATION if total > threshold; logs SLOW INTERLOCK ACTIVATION if F→I > max_acceptable |
| `resolve_fault` | `(&mut self, &str) -> Result<()>` | Delegates to `resolve_fault_with_outcome(Manual)` |
| `resolve_fault_with_outcome` | `(&mut self, &str, RecoveryOutcome) -> Result<()>` | Marks resolved; records resolution_time; updates MTTR sample; categorises auto/manual; moves to fault_history; calls review_safety_interlocks |
| `review_safety_interlocks` | `(&mut self)` [private] | Removes interlocks whose fault types are all resolved; records total_active_ms, increments interlock_releases |
| `trigger_critical_ground_alert` | `(&self, &FaultEvent, f64) -> Result<()>` [async, private] | Emits `error!` log: "CRITICAL ALERT: Fault response time exceeded 100ms" |
| `handle_loss_of_contact` | `(&mut self) -> Result<FaultResponse>` [async] | Creates CommunicationLoss FaultEvent; activates `emergency_loss_of_contact` interlock; blocks all_non_essential, experimental, high_power; sets loc_active_since |
| `is_loss_of_contact` | `(&self) -> bool` | Returns `consecutive_network_failures >= 3` |
| `increment_consecutive_failures` | `(&mut self)` | Increments counter; warns at threshold-1 |
| `get_consecutive_failures` | `(&self) -> u32` | Read-only accessor |
| `record_successful_communication` | `(&mut self)` | Resets to 0; records LoC duration; calls review_safety_interlocks |
| `get_stats` | `(&self) -> FaultManagerStats` | total_faults, active/critical faults, interlocks, consecutive_failures, avg_response/resolution_time, interlock_avg/max_latency, latency_violations, commands_blocked, MTTR, MTBF, auto/manual recoveries, LoC stats |

**CommandBlockEvent Fields** (REQ 3.3, 3.4)
- `fault_detection_time`, `interlock_activation_time`, `command_block_time`
- `fault_to_interlock_latency_ms` — time from fault detection to interlock going live
- `interlock_to_block_latency_ms` — time from interlock active to command checked
- `total_fault_to_block_latency_ms` — end-to-end interlock latency

**Active Interlocks Registry**

| Interlock ID | Fault Type | Blocked Commands |
|--|--|--|
| `emergency_loss_of_contact` | CommunicationLoss, NetworkError | non_essential, experimental, high_power |
| `emergency_comm_loss` | NetworkError, CommunicationLoss | payload_activation, experimental_mode, non_critical_systems |
| `thermal_emergency` | ThermalAnomaly | payload_high_power, transmitter_high_power, heater_activation |
| `power_conservation` | PowerAnomaly | payload_activation, transmitter_high_power, attitude_control_intensive, heating_systems |
| `attitude_stabilization` | AttitudeAnomaly | earth_observation, antenna_pointing, solar_panel_tracking, precision_maneuvers |
| `system_overload` | SystemOverload | data_processing_intensive, multiple_simultaneous_operations, background_tasks |

**Key Structures**
- `FaultEvent` — timestamp, fault_type, severity, description, affected_systems
- `FaultResponse` — fault_id, response_time_ms, recommended_action, safety_interlocks_triggered, commands_blocked, auto_recovery_attempted
- `ActiveFault` [private] — tracks detection_time, occurrence_count, response_time_ms, is_resolved, recovery_mode
- `SafetyInterlock` [private] — interlock_id, fault_types, blocked_command_types, blocked_systems, activation_latency_ms, released_at
- `FaultManagerStats` — comprehensive stats struct
- `FaultType` — ThermalAnomaly, PowerAnomaly, AttitudeAnomaly, NetworkError, TelemetryError, SystemOverload, CommunicationLoss, SensorFailure, CommandRejection, Unknown(String)
- `Severity` — Critical(0), High(1), Medium(2), Low(3)
- `RecoveryMode` — SoftReset, SafeMode, Cooldown, PowerSave, AttitudeHold, LinkFallback
- `RecoveryOutcome` — AutoSuccess(Option<RecoveryMode>), AutoFailedThenManual, Manual

### FaultSimulator [REQ 3.1]

```
FaultSimulator::new()                           — init simulation_counter = 0
FaultSimulator::create_thermal_fault()          REQ 3.1 — temperature-based, 4 severities
FaultSimulator::create_power_fault()            REQ 3.1 — battery-level based
FaultSimulator::create_attitude_fault()         REQ 3.1 — attitude-error based
FaultSimulator::create_communication_fault()    REQ 3.1 — packets-lost based
FaultSimulator::create_random_fault()           REQ 3.1 — random type + params
FaultSimulator::create_fault_packet()           REQ 3.1 — wrap EmergencyData → CommunicationPacket
FaultSimulator::create_thermal_fault_sequence() — escalating Medium→High→Critical
FaultSimulator::create_cascading_failure_sequence() — 5-step multi-system cascade
FaultSimulator::get_simulation_stats()          — total simulations count
```

| Function | Returns | Notes |
|---|---|---|
| `create_thermal_fault(severity, temp)` | `EmergencyData` | severity 0=Critical (>85°C), 1=High (>80°C), 2=Medium, 3=Low |
| `create_power_fault(severity, battery)` | `EmergencyData` | severity 0=Critical (<20%), 1=High (<30%), 2=Medium, 3=Low |
| `create_attitude_fault(severity, error)` | `EmergencyData` | severity 0=Critical (>10°), 1=High (>5°), 2=Medium, 3=Low |
| `create_communication_fault(severity, lost)` | `EmergencyData` | severity 0=Critical (>10), 1=High (>5), 2=Medium, 3=Low |
| `create_random_fault()` | `EmergencyData` | rand 0..4 selects type; parameters randomised in range |
| `create_fault_packet(fault_data)` | `CommunicationPacket` | `CommunicationPacket::new_emergency(fault_data, GroundControl)` |
| `create_thermal_fault_sequence()` | `Vec<EmergencyData>` | [65°C Medium, 82°C High, 87°C Critical] |
| `create_cascading_failure_sequence()` | `Vec<EmergencyData>` | [Power High, Thermal High, Comm Medium, Attitude Critical, Power Critical] |
| `get_simulation_stats()` | `u32` | simulation_counter |

---

## network.rs — Network Communication [REQ 1.1, 1.2, 1.3, 2.2]

### NetworkManager

```
NetworkManager::new()                           — bind UDP 127.0.0.1:7891, init crypto
NetworkManager::new_with_config()               — custom socket/timeout config
NetworkManager::receive_packet_with_timing()    REQ 1.1, 1.2, S.2, S.3 — main receive + timing
NetworkManager::receive_packet()                — backward-compat wrapper
NetworkManager::calculate_reception_timing()    REQ 1.2, S.2, S.3 [async, private]
NetworkManager::get_drift_stats()               REQ 1.2, S.2 — aggregate drift stats
NetworkManager::get_drift_report_by_type()      REQ 1.2 — per-type drift breakdown
NetworkManager::send_packet()                   — basic send (seal + send)
NetworkManager::send_packet_with_deadline_check()  REQ 2.2 — ≤2ms urgent enforcement
NetworkManager::request_retransmission()        REQ 1.3 — re-request by packet ID
NetworkManager::request_sensor_retransmission() REQ 1.3 — re-request by sensor params
NetworkManager::set_expected_interval()         — configure per-type interval
NetworkManager::is_connection_healthy()         — last packet within 5s check
NetworkManager::parse_packet_info()             [private] — extract sensor info from ID
```

### NetworkManager Function Details

| Function | Signature | Notes |
|---|---|---|
| `new` | `() -> Result<Self>` [async] | UDP bind to 7891; satellite at 7890; recv_timeout 100ms, send_timeout 50ms; default intervals: telemetry=100ms, heartbeat=1000ms, emergency=50ms, status=500ms |
| `new_with_config` | `(SocketAddr, SocketAddr, Duration, Duration) -> Result<Self>` [async] | Custom config constructor |
| `receive_packet_with_timing` | `(&self) -> Result<(CommunicationPacket, ReceptionTiming)>` [async] | Deframes 4-byte length prefix; decrypts JSON frame; measures decode_time_ms; logs DECODE VIOLATION if >3ms; calls calculate_reception_timing |
| `receive_packet` | `(&self) -> Result<CommunicationPacket>` [async] | Wrapper; discards timing |
| `calculate_reception_timing` | `(&self, &CommunicationPacket, DateTime<Utc>) -> ReceptionTiming` [async, private] | Computes drift_ms, jitter_ms, end_to_end_latency_ms; updates per-type schedule and sequence tracker; logs DRIFT DETECTED if jitter >25ms; classifies DriftSeverity |
| `get_drift_stats` | `(&self) -> DriftStats` [async] | avg_drift_ms, max_drift_ms, drift_violations (>25ms), total_packets_analyzed |
| `get_drift_report_by_type` | `(&self) -> HashMap<String, PacketTypeDriftStats>` [async] | Per packet-type: received, avg_drift_ms, max_drift_ms, violations |
| `send_packet` | `(&self, CommunicationPacket) -> Result<()>` [async] | Seals + sends framed bytes; updates counters |
| `send_packet_with_deadline_check` | `(&self, CommunicationPacket, bool, Option<DateTime<Utc>>) -> Result<SendResult>` [async] | Pre-checks deadline before seal/send; measures network_send_time_ms; logs URGENT COMMAND DEADLINE VIOLATION if urgent >2ms; returns SendResult |
| `request_retransmission` | `(&self, &str) -> Result<()>` [async] | Parses packet_id; creates `Command::re_request_command`; sends via send_packet; increments retransmission_requests |
| `request_sensor_retransmission` | `(&self, u32, &str, &str) -> Result<()>` [async] | Explicit sensor_id/type/reason; validates type string; sends re-request command |
| `set_expected_interval` | `(&self, &str, f64)` [async] | Mutates expected_intervals map |
| `is_connection_healthy` | `(&self) -> bool` [async] | last_packet_time within 5000ms |
| `parse_packet_info` | `(&self, &str) -> (u32, SensorType, String)` [private] | Heuristic: "thermal"→Thermal, "power"→Power, "attitude"→Attitude, "seq"/"delayed"→Thermal |

**DriftSeverity Classification**

| Label | Jitter Range |
|--|--|
| Normal | < 25ms |
| Minor | 25–50ms |
| Moderate | 50–100ms |
| Severe | > 100ms |

**Key Structures**
- `ReceptionTiming` — packet_id, packet_type, reception_time, packet_timestamp, end_to_end_latency_ms, reception_drift_ms, jitter_ms, is_delayed, delay_severity, decode_time_ms
- `DriftStats` — avg_drift_ms, max_drift_ms, drift_violations, total_packets_analyzed
- `PacketTypeDriftStats` — packet_type, packets_received, avg_drift_ms, max_drift_ms, violations
- `SendResult` — success, send_time_ms, deadline_met, deadline_violation_ms, packet_id
- `PacketSequenceInfo` [private] — per-type last_sequence, packets_received, total_drift_ms, max_drift_ms
- `DriftMeasurement` [private] — timestamped drift history record
- `ExpectedSchedule` [private] — start_time, interval_ms, next_expected, packets_expected

### NetworkStats helper (on `NetworkStats` struct)

```
NetworkStats::packets_per_second(duration)      — throughput rate
NetworkStats::avg_packet_size()                 — bytes ÷ packets_received
NetworkStats::connection_status()               — "Excellent" / "Good" / "Poor" / "Lost" / "No Contact"
NetworkStats::retransmission_rate()             — retransmissions ÷ packets_received
```

---

## performance.rs — System Performance Monitoring [REQ 4, S.1–S.3]

### PerformanceTracker

```
PerformanceTracker::new()                       REQ 4.1, 4.2 — init with all thresholds
PerformanceTracker::record_event()              REQ 4.3 — timestamped event ingestion
PerformanceTracker::update_metrics()            REQ 4.1, S.1–S.3 [private] — per-type metric update
PerformanceTracker::check_performance_violations()  [private] — log warnings
PerformanceTracker::cleanup_old_timing_data()   [private] — bound all deques + 30min window
PerformanceTracker::get_current_stats()         REQ 4.1, 4.2 — full PerformanceStats snapshot
PerformanceTracker::get_recent_events()         [private] — time-windowed event filter
PerformanceTracker::calculate_percentile()      [private] — p50/p95/p99 computation
PerformanceTracker::calculate_uptime_percentage()  [private] — simplified uptime
PerformanceTracker::calculate_system_health_score()  REQ 4.2 [private] — 0–100 score
PerformanceTracker::generate_performance_report()  REQ 4.2, 4.3 — full PerformanceReport
```

### PerformanceTracker Function Details

| Function | Signature | Notes |
|---|---|---|
| `new` | `() -> Self` | uplink_jitter_threshold=10ms; task_drift_warn=2ms, critical=5ms; backlog_warn=25%, crit=70%; cpu_warn=85%, mem_warn=85%, load_warn=0.8×cores; max_history=10000; window=30min |
| `record_event` | `(&mut self, PerformanceEvent)` | Calls update_metrics → check_performance_violations → push to deque → cleanup |
| `update_metrics` | `(&mut self, &PerformanceEvent)` [private] | ~20 EventType arms; see EventType table below |
| `check_performance_violations` | `(&mut self, &PerformanceEvent)` [private] | Warns: telemetry >3ms, urgent command >2ms, packet latency >200ms |
| `cleanup_old_timing_data` | `(&mut self)` [private] | Caps: processing_times, dispatch_times, latencies, task_drift, jitter, backlog len/age at 1000; removes events outside 30min window |
| `get_current_stats` | `(&self) -> PerformanceStats` | Computes all percentiles and aggregates; returns 40-field struct |
| `get_recent_events` | `(&self, Duration) -> Vec<&PerformanceEvent>` [private] | Filter by timestamp >= now - duration |
| `calculate_percentile` | `(&self, &VecDeque<f64>, f64) -> f64` [private] | Sort + index |
| `calculate_uptime_percentage` | `(&self) -> f64` [private] | 99.5 / 98.0 / 95.0 based on violations count |
| `calculate_system_health_score` | `(&self) -> f64` [private] | 100 − processing_violations×0.5 − timeouts×0.2 − drift_violations×0.1 − emergencies×2.0 − degradation×1.0; min 0 |
| `generate_performance_report` | `(&self, Duration) -> PerformanceReport` | Issues: 3ms violations, high delays (>100ms avg), drift violations (>10), timeouts (>5), scheduler drift p99, backlog len/age; recommendations |

**EventType → Metric Mapping** (in `update_metrics`)

| EventType | Metrics Updated | Req |
|--|--|--|
| `TelemetryProcessed` | avg/max processing_time, sensor_count, delayed_count | REQ 1.1 |
| `TelemetryProcessingViolation` | processing_violations_3ms ++, performance_violations ++ | REQ 1.1 |
| `PacketReceived` | total_packets_received, avg/max reception_latency | REQ 1.2 |
| `PacketDelayed` | delayed_packets_detected ++, reception_drift_violations ++ | REQ 1.2 |
| `PacketRetransmissionRequested` | retransmission_requests ++ | REQ 1.3 |
| `NetworkTimeout` | network_timeouts ++ | REQ 1.4 |
| `UplinkIntervalSample` | uplink_interarrival_ms, uplink_jitter_samples (cap 1000) | REQ 4.1, S.3 |
| `JitterViolation` | performance_violations ++ | S.3 |
| `TaskExecutionDrift` | task_drift_ms (cap 1000); warn >2ms, violation >5ms | REQ 4.1, S.1 |
| `SchedulerPrecisionViolation` | task_drift_ms, performance_violations ++ | S.1 |
| `TelemetryEnqueued` | backlog_len_samples; warn ≥25%, critical ≥70% of capacity | REQ 4.1 |
| `TelemetryDequeued` | backlog age sample (dequeue_time − enqueue_time), len sample | REQ 4.1 |
| `TelemetryDropped` | cleanup enqueue_times, performance_violations ++ | REQ 4.1 |
| `TelemetryBacklogWarning` | backlog_warn_events ++ | REQ 4.1 |
| `TelemetryBacklogCritical` | backlog_critical_events ++, performance_violations ++ | REQ 4.1 |
| `PacketDecodeViolation` | packet_decode_violations_3ms ++ if >3ms, violations ++ | REQ 1.1 |
| `NetworkDeadlineViolation` | network_deadline_violations_2ms ++, violations ++ | REQ 2.2 |
| `CommandDeadlineViolation` | command_deadline_violations ++, violations ++ | REQ 2.2 |
| `CommandDispatched` | command_dispatch_times history | REQ 2.4 |
| `UrgentCommandDelayed` | performance_violations ++ | REQ 2.2 |
| `EmergencyResponseTriggered` | emergency_responses ++ | REQ 3.5 |
| `SystemHealthUpdate` | cpu/mem/load1 latest, avg, peak; emits ResourceUtilizationHigh if above thresholds | REQ 4.2 |
| `ResourceUtilizationHigh` | system_degradation_events ++ | REQ 4.2 |
| `PerformanceDegradation` | system_degradation_events ++ | REQ 4.2 |

**Key Structures**
- `PerformanceEvent` — timestamp (Timestamp), event_type (EventType), duration_ms, metadata HashMap
- `PerformanceStats` — 40-field struct: processing times, violations, reception latency, uplink jitter (p95/p99/max), task drift (avg/max/p95/p99), backlog (avg/p95/max len + age), CPU/mem/load (latest/avg/peak), deadline violations
- `PerformanceReport` — report_timestamp, time_window_minutes, total_events, event_type_breakdown, performance_stats, identified_issues, recommendations
- `TelemetryMetrics` [private] — processing stats
- `NetworkMetrics` [private] — reception, retransmission, deadline violation stats
- `SystemMetrics` [private] — uptime, CPU/mem, emergency/degradation counts

---

## monitor.rs — System Load Monitoring [REQ 4.2]

```
SystemLoadMonitor::new()                        — init sysinfo, detect CPU cores
SystemLoadMonitor::get_system_load()            REQ 4.2 [async] — CPU%, memory%, load1
start_system_load_sampler()                     REQ 4.2, 4.3 [async task] — 1s sampling loop
```

| Function | Signature | Notes |
|---|---|---|
| `new` | `() -> Self` | Calls `sys.refresh_cpu_usage()`, `sys.refresh_memory()`; detects cpu_cores from sysinfo |
| `get_system_load` | `(&mut self) -> Result<SystemLoadMetrics>` [async] | cpu_percent from global_cpu_usage; memory_percent=(total-avail)/total×100; load1: sysinfo on Linux/macOS, cpu_pct/100×cores on Windows |
| `start_system_load_sampler` | `(Arc<Mutex<PerformanceTracker>>, u64)` [async task] | Loop at interval_ms; emits `SystemHealthUpdate` with cpu_pct/mem_pct/load1/cores metadata; warns: CPU >80%, mem >80%, load >0.8×cores (at 30-sample/min rate) |

**SystemLoadMetrics fields**: cpu_percent, memory_percent, load1, cpu_cores, timestamp

---

## main.rs — System Orchestration

```
GroundControlSystem::new()                      — create all subsystems in Arc<Mutex>
GroundControlSystem::performance_tracker_handle()  — return Arc clone
GroundControlSystem::run()                      — spawn 7 tasks + 1 simulator
GroundControlSystem::schedule_emergency_command_test()  — test command for deadline testing
GroundControlSystem::get_command_scheduler_metrics()   — stats + report snapshot
GroundControlSystem::validate_packet_format()   [private] — non-empty packet_id check
GroundControlSystem::shutdown()                 — set is_running=false + final stats log
main()                                          — tokio::main bootstrap
enum_s<T>()                                     — Debug→String for metadata
map_severity()                                  — string→Severity enum
```

### Concurrent Task Architecture (in `run()`)

| Task | Tick | Core Function Call | REQ |
|--|--|--|--|
| 1: Network receive | continuous | `receive_packet_with_timing()` | 1.1, 1.2 |
| 2: Telemetry processing | event-driven | `TelemetryProcessor::process_packet()` | 1.1, 1.3 |
| 3: Re-request missing packets | 500ms | `get_missing_packets()` + `request_retransmission()` | 1.3 |
| 4: Fault management | event-driven | `FaultManager::handle_fault()` | 3.1–3.5 |
| 5: Performance monitor | event-driven | `PerformanceTracker::record_event()` | 4.3 |
| 6: Health heartbeat | 5s (log 30s) | `get_current_stats()` + `get_drift_stats()` | 4.2 |
| 7: Command scheduler | 0.5ms | `process_pending_commands()` + deadline monitoring | 2.1–2.4 |
| Sim: Fault injector | 45s | `FaultSimulator::create_random_fault()` | 3.1 |

### Channels

| Channel | Type | Capacity | Producer → Consumer |
|--|--|--|--|
| `telemetry_tx/rx` | `mpsc<(CommunicationPacket, DateTime)>` | 100 | Task 1 → Task 2 |
| `fault_tx/rx` | `mpsc<FaultEvent>` | 50 | Tasks 1,2,3 → Task 4 |
| `performance_tx/rx` | `mpsc<PerformanceEvent>` | 200 | Tasks 1,2,3,7 → Task 5 |

### shutdown() Final Stats Logged
- Runtime duration
- Packets processed (total)
- Avg / p95 / p99 processing time  
- 3ms violations count
- Packets received, avg reception latency
- Delayed packets count, avg delay
- Reception drift violations
- Retransmission count, network timeouts
- Faults handled, critical active faults
- System health score, uptime %

---

## Complete Function Quick Reference

| Function | File | Line | REQ |
|---|---|---|---|
| `CommandScheduler::new` | command.rs | 69 | 2.1 |
| `CommandScheduler::get_enhanced_stats` | command.rs | 81 | 2.4 |
| `CommandScheduler::get_unified_deadline_report` | command.rs | 96 | 2.4 |
| `CommandScheduler::calculate_performance_trend` | command.rs | 127 | 4.1 |
| `CommandScheduler::record_send_time` | command.rs | 150 | 4.1 |
| `CommandScheduler::process_pending_commands` | command.rs | 157 | 2.2, 2.4 |
| `CommandScheduler::get_commands_approaching_deadline` | command.rs | 247 | 2.2 |
| `CommandScheduler::cleanup_expired_commands` | command.rs | 269 | 2.1 |
| `CommandScheduler::update_safety_validation_cache` | command.rs | 283 | 2.3 |
| `CommandScheduler::schedule_command` | command.rs | 288 | 2.1, 2.3 |
| `CommandScheduler::get_queue_stats` | command.rs | 326 | 4.2 |
| `SensorThresholds::default` | telemetry.rs | 75 | 1.1 |
| `TelemetryProcessor::new` | telemetry.rs | 135 | 1.1 |
| `TelemetryProcessor::process_packet` | telemetry.rs | 182 | 1.1 |
| `TelemetryProcessor::check_packet_delay` | telemetry.rs | 327 | 1.2, 1.3 |
| `TelemetryProcessor::process_sensor_reading` | telemetry.rs | 378 | 3.1 |
| `TelemetryProcessor::check_packet_sequence` | telemetry.rs | 496 | 1.3 |
| `TelemetryProcessor::take_unreported_delayed_packet_details` | telemetry.rs | 526 | 1.2 |
| `TelemetryProcessor::get_missing_packets` | telemetry.rs | 546 | 1.3 |
| `TelemetryProcessor::mark_packet_re_requested` | telemetry.rs | 560 | 1.3 |
| `TelemetryProcessor::cleanup_old_missing_packets` | telemetry.rs | 570 | — |
| `TelemetryProcessor::cleanup_old_delayed_packets` | telemetry.rs | 584 | — |
| `TelemetryProcessor::check_sensor_faults` | telemetry.rs | 599 | 3.1 |
| `TelemetryProcessor::calculate_thermal_trend` | telemetry.rs | 647 | — |
| `TelemetryProcessor::calculate_sensor_trend` | telemetry.rs | 656 | — |
| `TelemetryProcessor::get_stats` | telemetry.rs | 669 | 4.2 |
| `SensorAnalysis::from_sensor_type` | telemetry.rs | 710 | — |
| `SensorAnalysis::from_emergency_type` | telemetry.rs | 718 | — |
| `SensorAnalysis::from_emergency_severity` | telemetry.rs | 730 | — |
| `FaultManager::new` | fault.rs | ~155 | 3.2 |
| `FaultManager::handle_fault` | fault.rs | ~195 | 3.1, 3.2, 3.3, 3.5 |
| `FaultManager::handle_loss_of_contact` | fault.rs | ~280 | 1.4 |
| `FaultManager::is_loss_of_contact` | fault.rs | ~316 | 1.4 |
| `FaultManager::handle_network_fault` | fault.rs | ~320 | 1.4 |
| `FaultManager::handle_thermal_fault` | fault.rs | ~355 | 3.2 |
| `FaultManager::handle_power_fault` | fault.rs | ~407 | 3.2 |
| `FaultManager::handle_attitude_fault` | fault.rs | ~453 | 3.2 |
| `FaultManager::handle_system_overload` | fault.rs | ~500 | 3.2 |
| `FaultManager::handle_generic_fault` | fault.rs | ~530 | 3.2 |
| `FaultManager::activate_safety_interlock` | fault.rs | ~605 | 3.2, 3.3 |
| `FaultManager::is_command_blocked` | fault.rs | ~635 | 2.3, 3.2, 3.3, 3.4 |
| `FaultManager::record_command_block_event` | fault.rs | ~700 | 3.3, 3.4 |
| `FaultManager::resolve_fault_with_outcome` | fault.rs | ~730 | 4.2, S.4 |
| `FaultManager::resolve_fault` | fault.rs | ~762 | 3.2 |
| `FaultManager::review_safety_interlocks` | fault.rs | ~766 | 3.2 |
| `FaultManager::trigger_critical_ground_alert` | fault.rs | ~800 | 3.5 |
| `FaultManager::increment_consecutive_failures` | fault.rs | ~808 | 1.4 |
| `FaultManager::get_consecutive_failures` | fault.rs | ~820 | 1.4 |
| `FaultManager::record_successful_communication` | fault.rs | ~824 | 1.4 |
| `FaultManager::generate_fault_id` | fault.rs | ~840 | — |
| `FaultManager::update_response_time_stats` | fault.rs | ~855 | 4.2 |
| `FaultManager::get_stats` | fault.rs | ~865 | 4.2, S.4 |
| `FaultSimulator::new` | fault.rs | ~960 | 3.1 |
| `FaultSimulator::create_thermal_fault` | fault.rs | ~962 | 3.1 |
| `FaultSimulator::create_power_fault` | fault.rs | ~1010 | 3.1 |
| `FaultSimulator::create_attitude_fault` | fault.rs | ~1058 | 3.1 |
| `FaultSimulator::create_communication_fault` | fault.rs | ~1106 | 3.1 |
| `FaultSimulator::create_random_fault` | fault.rs | ~1154 | 3.1 |
| `FaultSimulator::create_fault_packet` | fault.rs | ~1178 | 3.1 |
| `NetworkManager::new` | network.rs | ~118 | 1.1 |
| `NetworkManager::new_with_config` | network.rs | ~128 | 1.1 |
| `NetworkManager::receive_packet_with_timing` | network.rs | ~178 | 1.1, 1.2 |
| `NetworkManager::receive_packet` | network.rs | ~235 | 1.1 |
| `NetworkManager::calculate_reception_timing` | network.rs | ~242 | 1.2, S.2, S.3 |
| `NetworkManager::set_expected_interval` | network.rs | ~355 | — |
| `NetworkManager::get_drift_stats` | network.rs | ~362 | 1.2, S.2 |
| `NetworkManager::get_drift_report_by_type` | network.rs | ~380 | 1.2 |
| `NetworkManager::send_packet` | network.rs | ~420 | 2.2 |
| `NetworkManager::send_packet_with_deadline_check` | network.rs | ~450 | 2.2 |
| `NetworkManager::request_retransmission` | network.rs | ~598 | 1.3 |
| `NetworkManager::request_sensor_retransmission` | network.rs | ~618 | 1.3 |
| `NetworkManager::parse_packet_info` | network.rs | ~640 | — |
| `NetworkManager::is_connection_healthy` | network.rs | ~658 | — |
| `PerformanceTracker::new` | performance.rs | 165 | 4.1, 4.2 |
| `PerformanceTracker::record_event` | performance.rs | 253 | 4.3 |
| `PerformanceTracker::update_metrics` | performance.rs | 277 | 4.1, S.1–S.3 |
| `PerformanceTracker::check_performance_violations` | performance.rs | ~600 | 4.1 |
| `PerformanceTracker::cleanup_old_timing_data` | performance.rs | ~625 | — |
| `PerformanceTracker::get_current_stats` | performance.rs | ~660 | 4.1, 4.2 |
| `PerformanceTracker::get_recent_events` | performance.rs | ~790 | — |
| `PerformanceTracker::calculate_percentile` | performance.rs | ~800 | — |
| `PerformanceTracker::calculate_uptime_percentage` | performance.rs | ~818 | 4.2 |
| `PerformanceTracker::calculate_system_health_score` | performance.rs | ~830 | 4.2 |
| `PerformanceTracker::generate_performance_report` | performance.rs | ~855 | 4.2, 4.3 |
| `SystemLoadMonitor::new` | monitor.rs | ~19 | 4.2 |
| `SystemLoadMonitor::get_system_load` | monitor.rs | ~33 | 4.2 |
| `start_system_load_sampler` | monitor.rs | ~78 | 4.2, 4.3 |
| `GroundControlSystem::new` | main.rs | ~52 | — |
| `GroundControlSystem::performance_tracker_handle` | main.rs | ~75 | — |
| `GroundControlSystem::run` | main.rs | ~79 | all |
| `GroundControlSystem::schedule_emergency_command_test` | main.rs | ~680 | 2.2 |
| `GroundControlSystem::get_command_scheduler_metrics` | main.rs | ~688 | 2.4 |
| `GroundControlSystem::validate_packet_format` | main.rs | ~695 | — |
| `GroundControlSystem::shutdown` | main.rs | ~700 | 4.3 |
| `main` | main.rs | ~720 | — |
| `enum_s` | main.rs | ~25 | — |
| `map_severity` | main.rs | ~38 | — |

