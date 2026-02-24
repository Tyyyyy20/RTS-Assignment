# Ground Control Station — Functions Overview
## Mapped to Assignment Requirements

---

## Assignment Requirement Coverage

| Req | Description | Status |
|-----|-------------|--------|
| 1.1 | Receive telemetry via sockets, decode within 3ms | `network.rs`, `telemetry.rs` |
| 1.2 | Log packet reception latency and reception drift | `network.rs`, `performance.rs` |
| 1.3 | Trigger re-requests on missing/delayed packets | `telemetry.rs`, `network.rs`, `main.rs` |
| 1.4 | Simulate "loss of contact" if ≥3 packets fail in sequence | `fault.rs`, `main.rs` |
| 2.1 | Maintain a real-time command schedule | `command.rs` |
| 2.2 | Enforce deadlines for urgent commands (≤2ms dispatch) | `command.rs`, `network.rs` |
| 2.3 | Validate commands against system state using safety interlocks | `fault.rs`, `command.rs` |
| 2.4 | Log command deadline adherence and rejection reasons | `command.rs`, `fault.rs`, `performance.rs` |
| 3.1 | Simulate reception of fault messages from satellite | `fault.rs` (`FaultSimulator`) |
| 3.2 | Block unsafe commands until resolved | `fault.rs` |
| 3.3 | Track interlock latency (detection to command block) | `fault.rs` |
| 3.4 | Document and explain all command rejections | `fault.rs`, `command.rs` |
| 3.5 | If fault response time >100ms, trigger critical ground alert | `fault.rs` |
| 4.1 | Benchmark uplink jitter, telemetry backlog, task execution drift | `performance.rs` |
| 4.2 | Record missed deadlines, system load, fault recovery metrics | `performance.rs`, `monitor.rs` |
| 4.3 | All real-time actions logged with timestamps | `performance.rs`, all modules |
| S.1 | Log scheduling drift | `performance.rs` |
| S.2 | Track latency in all pipelines | `network.rs`, `telemetry.rs`, `performance.rs` |
| S.3 | Record jitter | `performance.rs`, `network.rs` |
| S.4 | Handle simulated faults and log recovery time | `fault.rs` |
| S.5 | Implement safety interlocks, document all rejections | `fault.rs`, `command.rs` |

---

## REQ 1 — Telemetry Reception and Decoding

### network.rs — UDP socket reception and timing

- **`NetworkManager::new() -> Self`** [async, line ~118]
  - Binds UDP socket to `127.0.0.1:7891`, configures crypto context, sets receive timeout (100ms) and send timeout (50ms).
  - Initialises expected packet intervals: telemetry 100ms, heartbeat 1000ms, emergency 50ms, status 500ms.

- **`NetworkManager::new_with_config(local, satellite, recv_timeout, send_timeout) -> Self`** [async]
  - Custom-configuration constructor used by `new()`.

- **`NetworkManager::receive_packet_with_timing() -> Result<(CommunicationPacket, ReceptionTiming)>`** [async, line ~178]
  - **REQ 1.1, 1.2** — Receives raw UDP bytes, deframes, decrypts packet.
  - Measures `decode_time_ms`; logs `DECODE VIOLATION` if >3ms.
  - Calls `calculate_reception_timing()` to populate all drift/jitter/latency fields.
  - Updates `packets_received`, `bytes_received`, `last_packet_time`.

- **`NetworkManager::receive_packet() -> Result<CommunicationPacket>`** [async, line ~235]
  - Backward-compatibility wrapper around `receive_packet_with_timing()`.

- **`NetworkManager::calculate_reception_timing(packet, reception_time) -> ReceptionTiming`** [async, private, line ~242]
  - **REQ 1.2, S.2, S.3** — Computes `reception_drift_ms` (actual minus expected arrival), `end_to_end_latency_ms`, and `jitter_ms`.
  - Maintains per-packet-type expected schedule, sequence tracker, and rolling drift history (cap 1000).
  - Classifies `DriftSeverity`: Normal (<25ms), Minor (25–50ms), Moderate (50–100ms), Severe (>100ms).
  - Logs `DRIFT DETECTED` warning when jitter >25ms.

- **`NetworkManager::get_drift_stats() -> DriftStats`** [async, line ~350]
  - **REQ 1.2, S.2** — Returns average drift, max drift, violation count, and total packets analysed across all packet types.

- **`NetworkManager::get_drift_report_by_type() -> HashMap<String, PacketTypeDriftStats>`** [async, line ~375]
  - **REQ 1.2** — Per-packet-type breakdown of received count, avg/max drift, and violation count.

- **`NetworkManager::request_retransmission(packet_id) -> Result<()>`** [async, line ~598]
  - **REQ 1.3** — Parses `packet_id` to derive sensor type, constructs a `Command::re_request_command`, sends to satellite, increments `retransmission_requests`.

- **`NetworkManager::request_sensor_retransmission(sensor_id, sensor_type, reason) -> Result<()>`** [async, line ~618]
  - **REQ 1.3** — Explicit-parameter variant: validates sensor type string, builds re-request command, sends to satellite.

- **`NetworkManager::send_packet(packet) -> Result<()>`** [async, line ~450]
  - Basic send: seals packet with crypto, sends framed bytes to satellite.

- **`NetworkManager::send_packet_with_deadline_check(packet, is_urgent, deadline) -> Result<SendResult>`** [async, line ~490]
  - **REQ 2.2** — Checks deadline before send; measures `network_send_time_ms`; logs `URGENT COMMAND DEADLINE VIOLATION` if >2ms; returns `SendResult` with `deadline_met` and `deadline_violation_ms`.

- **`NetworkManager::set_expected_interval(packet_type, interval_ms)`** [async]
  - Updates per-type expected interval used by drift calculation.

- **`NetworkManager::is_connection_healthy() -> bool`** [async]
  - Returns `true` if a packet was received within the last 5 seconds.

- **`NetworkManager::parse_packet_info(packet_id) -> (u32, SensorType, String)`** [private]
  - Extracts sensor ID and type from a packet ID string for re-request commands.

#### `NetworkStats` helper methods
- **`packets_per_second(duration) -> f64`** — Throughput rate.
- **`avg_packet_size() -> f64`** — Average bytes per received packet.
- **`connection_status() -> String`** — Human-readable status: Excellent / Good / Poor / Lost / No Contact.
- **`retransmission_rate() -> f64`** — Retransmissions ÷ packets received.

---

### telemetry.rs — Packet decoding and sensor analysis

- **`TelemetryProcessor::new() -> Self`** [line 135]
  - Initialises empty sequence tracker, missing/delayed packet lists, packet history, and expected sensor configs with default thresholds (thermal critical 80°C, power low 30%).

- **`TelemetryProcessor::process_packet(packet, reception_time) -> Result<TelemetryProcessingResult>`** [async, line 182]
  - **REQ 1.1 (≤3ms)** — Main entry point: calls `check_packet_sequence()`, `check_packet_delay()`, processes each `SensorReading` via `process_sensor_reading()`.
  - Returns `TelemetryProcessingResult` containing: `processing_time_ms`, detected faults, missing/delayed packet lists, sensor analyses.
  - Caller (main.rs task 2) measures elapsed time and emits `TelemetryProcessingViolation` if >3ms.

- **`TelemetryProcessor::check_packet_sequence(&mut self, packet) -> Result<()>`** [line 496]
  - **REQ 1.3** — Compares `sequence_number` against last seen; if a gap is detected, records missing packet IDs.

- **`TelemetryProcessor::check_packet_delay(&mut self, packet, reception_time) -> Option<DelayedPacketInfo>`** [private, line 327]
  - **REQ 1.2, 1.3** — Compares `reception_time` to the expected arrival derived from last packet time + interval; returns `DelayedPacketInfo` if late.

- **`TelemetryProcessor::process_sensor_reading(reading, analysis) -> Option<FaultEvent>`** [async, private, line 378]
  - Calls `check_sensor_faults()` to compare reading values against thresholds; returns a `FaultEvent` if a threshold is breached.

- **`TelemetryProcessor::check_sensor_faults(&self, reading, analysis) -> Option<FaultEvent>`** [private, line 599]
  - **REQ 3.1** — Checks thermal, power, and attitude values against `SensorThresholds`; builds a `FaultEvent` with appropriate `FaultType` and `Severity`.

- **`TelemetryProcessor::take_unreported_delayed_packet_details() -> Vec<DelayedPacketResult>`** [line 526]
  - Returns and clears delayed packet reports not yet surfaced to callers. Used by main.rs to emit `PacketDelayed` performance events.

- **`TelemetryProcessor::get_missing_packets() -> Vec<String>`** [line 546]
  - **REQ 1.3** — Returns IDs of packets not yet received (and not already re-requested). Called every 500ms by the re-request task.

- **`TelemetryProcessor::mark_packet_re_requested(packet_id)`** [line 560]
  - **REQ 1.3** — Marks a missing packet as having a re-request sent to prevent duplicate requests.

- **`TelemetryProcessor::cleanup_old_missing_packets()`** [line 570]
  - Removes missing-packet records older than 5 seconds to bound memory usage.

- **`TelemetryProcessor::cleanup_old_delayed_packets()`** [line 584]
  - Removes delayed-packet records older than 5 seconds.

- **`TelemetryProcessor::calculate_thermal_trend(&self, sensor_id, temp) -> Option<TrendInfo>`** [private, line 647]
  - Placeholder for thermal trend analysis.

- **`TelemetryProcessor::calculate_sensor_trend(&self, reading) -> Option<TrendInfo>`** [private, line 656]
  - General sensor trend calculation based on recent readings.

- **`TelemetryProcessor::get_stats() -> TelemetryStats`** [line 669]
  - Returns aggregate telemetry statistics: total packets, sensor readings, processing time, consecutive failures, delayed packet count.

#### `SensorAnalysis` helper constructors
- **`from_sensor_type(sensor_type: &str) -> Self`** [line 710] — Maps type string to analysis struct.
- **`from_emergency_type(emergency_type: &str) -> Self`** [line 718] — Maps emergency type string.
- **`from_emergency_severity(severity: &NetSeverity) -> Self`** [line 730] — Maps network severity enum.

#### `SensorThresholds::default()` [line 75]
- Default safety thresholds: thermal critical 80°C, emergency 85°C; power low 30%, critical 20%; attitude warning 5°, critical 10°.

---

### fault.rs — Loss of contact tracking

- **`FaultManager::increment_consecutive_failures()`**
  - **REQ 1.4** — Increments `consecutive_network_failures`; warns when one failure away from loss-of-contact threshold (3).

- **`FaultManager::is_loss_of_contact() -> bool`**
  - **REQ 1.4** — Returns `true` when `consecutive_network_failures >= 3`.

- **`FaultManager::handle_loss_of_contact() -> Result<FaultResponse>`** [async]
  - **REQ 1.4** — Triggered when ≥3 packets fail: creates a `CommunicationLoss` fault, activates `emergency_loss_of_contact` safety interlock, blocks non-essential/high-power commands, marks `auto_recovery_attempted`.

- **`FaultManager::record_successful_communication()`**
  - Resets `consecutive_network_failures` to 0; calls `review_safety_interlocks()` to release interlocks; records LoC duration.

- **`FaultManager::get_consecutive_failures() -> u32`**
  - Read-only accessor for current consecutive network failure count.

---

## REQ 2 — Command Uplink Scheduler

### command.rs — Real-time command queue

- **`CommandScheduler::new() -> Self`** [line 69]
  - **REQ 2.1** — Creates scheduler with empty `VecDeque` queue, zero counters, and bounded send-time history (cap 1000).

- **`CommandScheduler::schedule_command(command) -> Result<String>`** [line 288]
  - **REQ 2.1, 2.3** — Validates: deadline not in the past, command ID not empty, no duplicate IDs.
  - Urgent commands (`priority ≤ 1`) are pushed to the **front** of the queue; others to the back.
  - Returns the command ID on success, or an error with the reason (logged for REQ 2.4).

- **`CommandScheduler::process_pending_commands(fault_manager, network, perf_tx) -> Vec<Command>`** [async, line 157]
  - **REQ 2.2, 2.4** — Drains the queue: calls `network.send_packet_with_deadline_check()` for each command.
  - Tracks `network_violations` (send >2ms), `deadline_violations` (deadline missed), `total_urgent`.
  - Failed commands are re-queued: high-priority at front, low-priority at back.
  - Emits `CommandDispatchError` performance events with error and retry count metadata.

- **`CommandScheduler::get_commands_approaching_deadline() -> Vec<DeadlineWarning>`** [line 247]
  - **REQ 2.2** — Returns list of queued commands with their time-to-deadline in ms. Called every 0.5ms tick by the scheduler task to detect imminent misses.

- **`CommandScheduler::get_unified_deadline_report() -> UnifiedDeadlineReport`** [line 96]
  - **REQ 2.4** — Returns: total urgent commands, network violations, deadline violations, average network send time, adherence rate (%), network adherence rate (%), performance trend string.

- **`CommandScheduler::get_enhanced_stats() -> EnhancedCommandSchedulerStats`** [line 81]
  - **REQ 2.4** — Returns: queued count, total dispatched, urgent dispatched, average send time ms.

- **`CommandScheduler::calculate_performance_trend() -> String`** [private, line 127]
  - **REQ 4.1** — Compares older vs recent send times; returns "improving", "degrading", "stable", or "insufficient_data".

- **`CommandScheduler::record_send_time(send_time_ms)`** [private, line 150]
  - Appends send time to rolling history; evicts oldest when >1000 entries.

- **`CommandScheduler::get_queue_stats() -> HashMap<String, u64>`** [line 326]
  - **REQ 4.2** — Returns: total_queued, urgent_queued, normal_queued, total_dispatched, urgent_dispatched.

- **`CommandScheduler::cleanup_expired_commands()`** [async, line 269]
  - Removes commands whose deadline has already passed.

- **`CommandScheduler::update_safety_validation_cache()`** [async, line 283]
  - Hook reserved for future per-tick safety validation of queued commands.

### fault.rs — Safety interlock validation (REQ 2.3)

- **`FaultManager::is_command_blocked(command_type, target_system, command_id, submission_time) -> (bool, Vec<String>, Option<CommandBlockEvent>)`**
  - **REQ 2.3, 3.2, 3.4** — Checks all active `SafetyInterlock` entries; matches by `blocked_command_types` and `blocked_systems`.
  - Returns: `(is_blocked, blocking_reasons, CommandBlockEvent)`.
  - `CommandBlockEvent` contains full latency breakdown: `fault_to_interlock_latency_ms`, `interlock_to_block_latency_ms`, `total_fault_to_block_latency_ms`.
  - Logs `INTERLOCK LATENCY HIGH` warning when total latency exceeds threshold.

### network.rs (see REQ 1)
- **`NetworkManager::send_packet_with_deadline_check()`** — enforces ≤2ms for urgent commands.

---

## REQ 3 — Fault Management and Interlocks

### fault.rs — FaultManager

- **`FaultManager::new() -> Self`**
  - Initialises with `loss_of_contact_threshold = 3`, `critical_response_time_ms = 100.0`, `interlock_latency_threshold_ms = 10.0`, empty fault maps and history.

- **`FaultManager::handle_fault(fault_event) -> Result<FaultResponse>`** [async]
  - **REQ 3.1, 3.2, 3.3, 3.5** — Main fault processing entry point.
  - Generates deterministic `fault_id` from type + affected systems hash.
  - Handles recurring faults (escalates recommendation at 3+ occurrences).
  - Dispatches to type-specific handlers.
  - Measures `response_time_ms` and calls `trigger_critical_ground_alert()` if >100ms.
  - Updates MTBF samples and running average response time.

- **`FaultManager::handle_network_fault(fault_event, response, fault_id) -> Result<FaultResponse>`** [async, private]
  - Checks if `consecutive_network_failures >= 3`; if so activates `emergency_comm_loss` interlock and blocks payload/experimental commands.

- **`FaultManager::handle_thermal_fault(fault_event, response, fault_id) -> Result<FaultResponse>`** [async, private]
  - Critical: activates `thermal_emergency` interlock, blocks high-power/heating commands.
  - High: blocks heater and CPU-intensive commands without full interlock.

- **`FaultManager::handle_power_fault(fault_event, response, fault_id) -> Result<FaultResponse>`** [async, private]
  - Critical: activates `power_conservation` interlock, blocks payload/transmitter/attitude/heating commands.

- **`FaultManager::handle_attitude_fault(fault_event, response, fault_id) -> Result<FaultResponse>`** [async, private]
  - Critical: activates `attitude_stabilization` interlock, blocks pointing/maneuver commands.

- **`FaultManager::handle_system_overload(fault_event, response, fault_id) -> Result<FaultResponse>`** [async, private]
  - Activates `system_overload` interlock; blocks CPU/memory-intensive commands.

- **`FaultManager::handle_generic_fault(fault_event, response, fault_id) -> Result<FaultResponse>`** [async, private]
  - Blocks experimental/non-essential operations for High/Critical severity generic faults.

- **`FaultManager::activate_safety_interlock(interlock_id, fault_types, blocked_command_types, blocked_systems, reason, fault_id, fault_detected_at)`** [private]
  - **REQ 3.2, 3.3** — Records `activation_latency_ms` = time from fault detection to interlock activation; stores `SafetyInterlock` in active map; increments `total_interlocks_activated`.

- **`FaultManager::is_command_blocked(...) -> (bool, Vec<String>, Option<CommandBlockEvent>)`**
  - See REQ 2 section. Also serves REQ 3.2, 3.3, 3.4.

- **`FaultManager::record_command_block_event(block_event)`**
  - **REQ 3.3, 3.4** — Persists `CommandBlockEvent` (cap 500); logs `INTERLOCK LATENCY VIOLATION` if `total_fault_to_block_latency_ms > threshold`; logs `SLOW INTERLOCK ACTIVATION` if `fault_to_interlock_latency_ms > max_acceptable`.

- **`FaultManager::resolve_fault(fault_id) -> Result<()>`**
  - **REQ 3.2** — Manual resolution; delegates to `resolve_fault_with_outcome(Manual)`.

- **`FaultManager::resolve_fault_with_outcome(fault_id, outcome) -> Result<()>`**
  - **REQ 4.2** — Marks fault resolved, records resolution time, computes MTTR sample, categorises as auto/manual recovery, moves to `fault_history`, calls `review_safety_interlocks()`.

- **`FaultManager::review_safety_interlocks()`** [private]
  - **REQ 3.2** — Releases interlocks whose fault types are all resolved; records `interlock_total_active_ms` and `interlock_releases`.

- **`FaultManager::trigger_critical_ground_alert(fault_event, response_time)`** [async, private]
  - **REQ 3.5** — Emits `error!` log entry when `response_time > critical_response_time_ms` (100ms).

- **`FaultManager::generate_fault_id(fault_event) -> String`** [private]
  - Deterministic ID from `FaultType` discriminant + `affected_systems` hash, ensuring recurring faults are recognised.

- **`FaultManager::update_response_time_stats(response_time)`** [private]
  - **REQ 4.2** — Maintains running average `avg_fault_response_time_ms`.

- **`FaultManager::get_stats() -> FaultManagerStats`**
  - **REQ 4.2, S.4** — Returns: total/active faults, critical alerts, interlocks activated/active, consecutive failures, avg response/resolution time, interlock avg/max latency, latency violations, commands blocked, MTTR, MTBF, auto/manual recovery counts, LoC stats.

- **`FaultManager::push_bounded(buf, v, max_len)`** [private static] — Bounds-checked push for MTTR/MTBF sample vecs.
- **`FaultManager::avg(xs) -> f64`** [private static] — Safe average (returns 0.0 for empty).

### fault.rs — FaultSimulator (REQ 3.1)

- **`FaultSimulator::new() -> Self`** — Initialises simulation counter.
- **`FaultSimulator::create_thermal_fault(severity, temperature) -> EmergencyData`** — Generates thermal fault packet with appropriate severity, description, and recommended actions.
- **`FaultSimulator::create_power_fault(severity, battery_level) -> EmergencyData`** — Battery-level based power fault simulation.
- **`FaultSimulator::create_attitude_fault(severity, attitude_error) -> EmergencyData`** — Attitude error based fault simulation.
- **`FaultSimulator::create_communication_fault(severity, packets_lost) -> EmergencyData`** — Communication packet-loss fault simulation.
- **`FaultSimulator::create_random_fault() -> EmergencyData`** — Randomly selects one of the four fault types with randomised parameters.
- **`FaultSimulator::create_fault_packet(fault_data) -> CommunicationPacket`** — Wraps `EmergencyData` into a `CommunicationPacket::new_emergency` for injection into the telemetry path.
- **`FaultSimulator::create_thermal_fault_sequence() -> Vec<EmergencyData>`** — Escalating sequence: Medium → High → Critical thermal.
- **`FaultSimulator::create_cascading_failure_sequence() -> Vec<EmergencyData>`** — Multi-system cascading failure: power → thermal → comms → attitude → power critical.
- **`FaultSimulator::get_simulation_stats() -> u32`** — Returns total simulated fault count.

---

## REQ 4 — System Performance Monitoring

### performance.rs — PerformanceTracker

- **`PerformanceTracker::new() -> Self`** [line 165]
  - **REQ 4.1, 4.2** — Sets thresholds: uplink jitter 10ms, task drift warn 2ms / critical 5ms, backlog warn 25% / critical 70%, CPU warn 85%, mem warn 85%, load warn 80% per core.

- **`PerformanceTracker::record_event(event)`** [line 253]
  - **REQ 4.3** — All timestamped events flow through here: calls `update_metrics()`, `check_performance_violations()`, appends to rolling history (cap 10 000), calls `cleanup_old_timing_data()`.

- **`PerformanceTracker::update_metrics(event)`** [private, line 277]
  - **REQ 4.1, 4.2, S.1, S.2, S.3** — Per-EventType logic:
    - `TelemetryProcessed` → update avg/max processing time, sensor count, delayed count.
    - `PacketReceived` → update avg/max reception latency.
    - `PacketDelayed` → increment `delayed_packets_detected`, `reception_drift_violations`.
    - `PacketRetransmissionRequested` → increment retransmission count.
    - `NetworkTimeout` → increment timeout count.
    - `CommandDispatched` / `UrgentCommandDelayed` → dispatch time history.
    - `TelemetryProcessingViolation` → increment `processing_violations_3ms`, `performance_violations`.
    - `PacketDecodeViolation` → increment `packet_decode_violations_3ms` if >3ms.
    - `UplinkIntervalSample` → **REQ 4.1, S.3** record inter-arrival and jitter samples (cap 1000).
    - `TaskExecutionDrift` → **REQ 4.1, S.1** record drift sample; log warn at >2ms, violation at >5ms.
    - `SchedulerPrecisionViolation` → record drift + increment violations.
    - `TelemetryEnqueued / Dequeued / Dropped` → **REQ 4.1** backlog length and age tracking; warn at 25% / critical at 70% capacity.
    - `NetworkDeadlineViolation` → increment `network_deadline_violations_2ms`.
    - `CommandDeadlineViolation` → increment `command_deadline_violations`.
    - `SystemHealthUpdate` → **REQ 4.2** update running avg/peak for CPU, memory, load1; emit `ResourceUtilizationHigh` if above thresholds.
    - `EmergencyResponseTriggered` → increment emergency responses.
    - `PerformanceDegradation` → increment degradation events.

- **`PerformanceTracker::check_performance_violations(event)`** [private]
  - Logs warnings for: telemetry >3ms, urgent command >2ms, packet reception >200ms.

- **`PerformanceTracker::cleanup_old_timing_data()`** [private]
  - Bounds all rolling deques at 1000 samples; removes events outside the 30-minute performance window.

- **`PerformanceTracker::get_current_stats() -> PerformanceStats`**
  - **REQ 4.1, 4.2** — Computes and returns full `PerformanceStats`: processing times (avg/max/p95/p99), violations, reception latency, uplink jitter (p95/p99/max), task drift (avg/max/p95/p99), backlog (avg/p95/max length and age), CPU/memory/load (latest/avg/peak), deadline violations.

- **`PerformanceTracker::get_recent_events(duration) -> Vec<&PerformanceEvent>`** [private]
  - Filters event history to the specified time window.

- **`PerformanceTracker::calculate_percentile(values, percentile) -> f64`** [private]
  - Sorts a `VecDeque<f64>` and returns the requested percentile value.

- **`PerformanceTracker::calculate_uptime_percentage() -> f64`** [private]
  - Returns 99.5% / 98% / 95% depending on total violation count.

- **`PerformanceTracker::calculate_system_health_score() -> f64`** [private]
  - **REQ 4.2** — Score starts at 100; deductions for: processing violations, timeouts, drift violations, emergency responses, degradation events. Minimum 0.

- **`PerformanceTracker::generate_performance_report(duration) -> PerformanceReport`**
  - **REQ 4.2, 4.3** — Produces `PerformanceReport` with: `PerformanceStats`, event-type breakdown, identified issues (violations, drift, backlog, deadline violations), and recommendations.

### monitor.rs — System Load Monitoring (REQ 4.2)

- **`SystemLoadMonitor::new() -> Self`**
  - Initialises `sysinfo::System`, refreshes CPU and memory, detects CPU core count.

- **`SystemLoadMonitor::get_system_load() -> Result<SystemLoadMetrics>`** [async]
  - **REQ 4.2** — Refreshes CPU and memory; computes: `cpu_percent`, `memory_percent`, `load1` (via sysinfo on Linux/macOS; derived from CPU% × cores on Windows).

- **`start_system_load_sampler(performance_tracker, interval_ms)`** [async task]
  - **REQ 4.2, 4.3** — Background loop that calls `get_system_load()` every `interval_ms`, emits `SystemHealthUpdate` performance events with CPU/memory/load1/cores metadata; warns when CPU >80%, memory >80%, or load >80% per core.

---

## Main System Orchestration (main.rs)

- **`GroundControlSystem::new() -> Result<Self>`** [async]
  - Constructs all subsystem components (NetworkManager, TelemetryProcessor, FaultManager, PerformanceTracker, CommandScheduler) wrapped in `Arc<Mutex<>>` for shared async access.

- **`GroundControlSystem::performance_tracker_handle() -> Arc<Mutex<PerformanceTracker>>`**
  - Returns cloned Arc handle for use by external async tasks.

- **`GroundControlSystem::run() -> Result<()>`** [async — main execution loop]
  - Starts 7 concurrent tokio tasks + 1 simulator task:
    - **Task 1** (network receive): calls `receive_packet_with_timing()`; resets/increments failure counters; emits `TelemetryEnqueued`, `PacketDecodeViolation`, `PacketReceived`, `PacketDelayed` events.
    - **Task 2** (telemetry processing, ≤3ms): calls `TelemetryProcessor::process_packet()`; emits `TelemetryProcessingViolation` if >3ms; forwards detected faults to fault channel; emits `PacketDelayed`, `TelemetryProcessed` events.
    - **Task 3** (missing packet re-request, 500ms tick): calls `get_missing_packets()`, `request_retransmission()`, `mark_packet_re_requested()`; emits `PacketRetransmissionRequested`.
    - **Task 4** (fault management): receives `FaultEvent` from channel; calls `handle_fault()`.
    - **Task 5** (performance monitor): receives `PerformanceEvent` from channel; calls `record_event()`.
    - **Task 6** (health heartbeat, 5s tick): logs stats summary; emits report every 30s.
    - **Task 7** (command scheduler, 0.5ms tick): calls `process_pending_commands()`; monitors `get_commands_approaching_deadline()`; emits `CommandDeadlineViolation`; logs deadline report every 10ms; runs `cleanup_expired_commands()` every 500ms.
    - **Simulator**: injects `create_random_fault()` packet every 45 seconds.

- **`GroundControlSystem::schedule_emergency_command_test() -> Result<String>`** [async]
  - Creates and schedules a thermal emergency response command for deadline testing.

- **`GroundControlSystem::get_command_scheduler_metrics() -> (EnhancedCommandSchedulerStats, UnifiedDeadlineReport)`** [async]
  - Returns combined stats snapshot from command scheduler.

- **`GroundControlSystem::validate_packet_format(packet) -> bool`** [private]
  - Returns `true` if `packet_id` is non-empty.

- **`GroundControlSystem::shutdown()`** [async]
  - Sets `is_running = false`; logs final stats: packets processed, avg/p95/p99 processing time, 3ms violations, latency, drift, retransmissions, faults, health score.

- **`main() -> Result<()>`** [tokio::main]
  - Bootstraps tracing/logging, creates `GroundControlSystem`, starts system-load sampler task, starts CPU/memory heartbeat task, registers Ctrl+C handler for graceful shutdown, calls `run()`.

- **`enum_s<T: Debug>(e) -> String`** — Formats any Debug enum to string for metadata maps.
- **`map_severity(s) -> fault::Severity`** — Maps string severity label to `Severity` enum.

---

## Key Real-Time Requirements Summary

| Requirement | Deadline | Enforcing Function | Violation Event |
|---|---|---|---|
| Telemetry decode | ≤3ms | `receive_packet_with_timing()` | `PacketDecodeViolation` |
| Telemetry processing | ≤3ms | `TelemetryProcessor::process_packet()` | `TelemetryProcessingViolation` |
| Urgent command dispatch (network) | ≤2ms | `send_packet_with_deadline_check()` | `NetworkDeadlineViolation` |
| Command deadline | Per-command | `process_pending_commands()` | `CommandDeadlineViolation` |
| Fault response | ≤100ms | `handle_fault()` → `trigger_critical_ground_alert()` | `error!` log |
| Loss of contact | ≥3 consecutive failures | `is_loss_of_contact()` / `handle_loss_of_contact()` | Safety interlock + `error!` |
| Scheduler tick resolution | 0.5ms | Task 7 in `run()` | Deadline warnings |

---

## Data Flow

```
Satellite UDP
    │
    ▼  Task 1
NetworkManager::receive_packet_with_timing()
    │ [latency, drift, jitter → performance events]
    ▼
telemetry_channel (mpsc, cap 100)   ←  FaultSimulator injects every 45s
    │
    ▼  Task 2
TelemetryProcessor::process_packet()   [≤3ms target]
    │  ├─ check_packet_sequence()      [missing packets → re-request queue]
    │  ├─ check_packet_delay()         [delayed → performance event]
    │  └─ process_sensor_reading()     [faults → fault_channel]
    │
    ├──► fault_channel
    │        │
    │        ▼  Task 4
    │    FaultManager::handle_fault()
    │        ├─ activate_safety_interlock()
    │        ├─ trigger_critical_ground_alert()  [if >100ms]
    │        └─ resolve_fault_with_outcome()
    │
    └──► performance_channel
             │
             ▼  Task 5
         PerformanceTracker::record_event()
             └─ update_metrics() / check_performance_violations()

Task 3 (500ms)
    CommandScheduler → get_missing_packets()
    → NetworkManager::request_retransmission()

Task 7 (0.5ms)
    CommandScheduler::process_pending_commands()
    → FaultManager::is_command_blocked()   [safety interlock check]
    → NetworkManager::send_packet_with_deadline_check()   [≤2ms]
```
