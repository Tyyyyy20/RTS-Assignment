# Student B - Ground Control Station (GCS): Requirements-to-Code Mapping

This document maps Student B requirements to the current Ground Control implementation after
module/function refactors and timing tuning.

---

## B1 - Telemetry Reception and Decoding

### Requirement: Receive telemetry via sockets and decode within 3ms

| What the code does | Where |
|---|---|
| Binds UDP and receives framed encrypted packets with default timeouts (receive 150ms, send 75ms) | ground_control/src/network_manager.rs -> NetworkManager::new_default, NetworkManager::new_with_addresses |
| Decrypts/deserializes packets and measures decode duration | ground_control/src/network_manager.rs -> NetworkManager::receive_packet_with_reception_timing |
| Enforces decode budget by flagging decode durations above 3ms | ground_control/src/network_manager.rs -> NetworkManager::receive_packet_with_reception_timing |
| Pushes packets into bounded telemetry queue (capacity 100) for async processing | ground_control/src/main.rs -> GroundControlStation::run (Task 1 loop) |
| Processes telemetry payloads and measures end-to-end packet processing time | ground_control/src/telemetry_processor.rs -> TelemetryProcessor::process_telemetry_packet |
| Emits processing violations when processing time exceeds 3ms | ground_control/src/main.rs -> GroundControlStation::run (Task 2 loop), ground_control/src/performance_tracker.rs -> PerformanceTracker::record_performance_event + apply_event_metrics |

### Requirement: Log reception latency and drift

| What the code does | Where |
|---|---|
| Computes end-to-end latency, reception drift, and jitter for each packet | ground_control/src/network_manager.rs -> NetworkManager::compute_reception_timing |
| Classifies drift severity (Normal, Minor, Moderate, Severe) | ground_control/src/network_manager.rs -> DriftSeverity |
| Logs severe drift cases and emits packet timing performance events | ground_control/src/network_manager.rs -> NetworkManager::compute_reception_timing, ground_control/src/main.rs -> GroundControlStation::run (Task 1 loop) |
| Maintains drift history and provides drift statistics for heartbeat reports | ground_control/src/network_manager.rs -> NetworkManager::collect_drift_stats |

### Requirement: Trigger re-requests on missing/delayed packets

| What the code does | Where |
|---|---|
| Tracks sequence gaps and creates missing packet records with bounded resync logic (`MAX_SEQUENCE_GAP = 10`) | ground_control/src/telemetry_processor.rs -> TelemetryProcessor::track_packet_sequence |
| Collects missing packet IDs and sends retransmission requests periodically (every 650ms) | ground_control/src/main.rs -> GroundControlStation::run (Task 3 loop), ground_control/src/telemetry_processor.rs -> TelemetryProcessor::collect_missing_packet_ids |
| Sends retransmission command packets and increments retransmission counters | ground_control/src/network_manager.rs -> NetworkManager::send_retransmission_request |
| Tracks and reports delayed packets in both network and telemetry layers | ground_control/src/telemetry_processor.rs -> TelemetryProcessor::detect_packet_delay + TelemetryProcessor::collect_unreported_delayed_packet_details, ground_control/src/main.rs -> GroundControlStation::run (Task 2 delayed confirmation logs) |

### Requirement: Simulate loss of contact when failures are consecutive

| What the code does | Where |
|---|---|
| Increments consecutive network failures and triggers LOC behavior at threshold 3 | ground_control/src/fault_management.rs -> FaultManager::increment_consecutive_failures, has_loss_of_contact_condition |
| Declares LOC and activates emergency interlock once per LOC episode | ground_control/src/fault_management.rs -> FaultManager::handle_loss_of_contact |
| On successful communication, resets failure streak and auto-resolves relevant network faults | ground_control/src/fault_management.rs -> FaultManager::record_successful_communication |

---

## B2 - Command Uplink Scheduler

### Requirement: Maintain a real-time command schedule

| What the code does | Where |
|---|---|
| Holds scheduled commands in an internal queue with retry metadata | ground_control/src/command_scheduler.rs -> CommandScheduler::schedule_command (enqueue policy) + CommandScheduler struct queue fields |
| Dispatches command queue on scheduler loop with priority handling | ground_control/src/command_scheduler.rs -> CommandScheduler::process_dispatch_queue |
| Runs scheduler on 0.5ms tick cadence | ground_control/src/main.rs -> GroundControlStation::run (Task 7, tokio::time::interval(Duration::from_micros(500))) |
| Performs periodic queue cleanup and validation refresh | ground_control/src/main.rs -> GroundControlStation::run (Task 7 periodic branch), ground_control/src/command_scheduler.rs -> CommandScheduler::get_commands_approaching_deadline |

### Requirement: Enforce urgent deadlines (<= 2ms send budget)

| What the code does | Where |
|---|---|
| Keeps urgent network-send threshold at 2ms (`NETWORK_DEADLINE_THRESHOLD_MS = 2.0`) | ground_control/src/command_scheduler.rs -> NETWORK_DEADLINE_THRESHOLD_MS (used in CommandScheduler::process_dispatch_queue) |
| Performs deadline-aware packet sends and calculates violations | ground_control/src/network_manager.rs -> NetworkManager::send_packet_with_deadline_guard |
| Tracks network and deadline violations and reports adherence | ground_control/src/command_scheduler.rs -> CommandScheduler::get_unified_deadline_report |

### Requirement: Validate commands using safety interlocks

| What the code does | Where |
|---|---|
| Maps command types/target systems to interlock categories before dispatch | ground_control/src/command_scheduler.rs -> CommandScheduler::map_command_type_to_interlock_category + CommandScheduler::map_target_system_to_interlock_category |
| Checks blocking state with fault manager before command send | ground_control/src/command_scheduler.rs -> CommandScheduler::process_dispatch_queue; ground_control/src/fault_management.rs -> FaultManager::is_command_blocked |
| Bypasses interlock checks for emergency/recovery command types | ground_control/src/command_scheduler.rs -> CommandScheduler::process_dispatch_queue |

### Requirement: Log rejection reasons and deadline adherence

| What the code does | Where |
|---|---|
| Logs rejection details and emits validation-failed performance events | ground_control/src/command_scheduler.rs -> CommandScheduler::process_dispatch_queue |
| Persists rejected operations to CSV audit log | ground_control/src/fault_management.rs -> FaultManager::append_rejected_operation_to_csv |
| Records block latency breakdown for each blocked command | ground_control/src/fault_management.rs -> FaultManager::record_command_block_event |

---

## B3 - Fault Management and Interlocks

### Requirement: Simulate and process satellite fault messages

| What the code does | Where |
|---|---|
| Generates synthetic emergency fault packets periodically (45s interval) | ground_control/src/main.rs -> GroundControlStation::launch_fault_simulation_task, ground_control/src/fault_management.rs -> FaultSimulator::create_random_fault + FaultSimulator::create_fault_packet |
| Converts emergency/health/sensor anomalies into FaultEvent instances | ground_control/src/telemetry_processor.rs -> TelemetryProcessor::process_telemetry_packet + TelemetryProcessor::detect_sensor_fault_event |
| Handles faults in dedicated task and updates active/resolved fault state | ground_control/src/main.rs -> GroundControlStation::launch_fault_management_task + GroundControlStation::run (Task 4), ground_control/src/fault_management.rs -> FaultManager::handle_fault + FaultManager::resolve_fault_with_outcome |

### Requirement: Block unsafe commands until resolved

| What the code does | Where |
|---|---|
| Activates fault-specific safety interlocks with command/system block scopes | ground_control/src/fault_management.rs -> FaultManager::activate_safety_interlock (called by handle_network_fault/handle_thermal_fault/handle_power_fault/handle_attitude_fault/handle_system_overload) |
| Releases interlocks when triggering conditions resolve | ground_control/src/fault_management.rs -> FaultManager::reconcile_safety_interlocks |

### Requirement: Track interlock latency and response time

| What the code does | Where |
|---|---|
| Tracks fault-to-interlock and interlock-to-block latency for blocked commands | ground_control/src/fault_management.rs -> FaultManager::is_command_blocked + FaultManager::record_command_block_event |
| Maintains interlock latency thresholds (10ms total, 5ms activation) and warnings | ground_control/src/fault_management.rs -> FaultManager::new (threshold fields) + FaultManager::record_command_block_event |
| Triggers critical fault alert when response exceeds 100ms | ground_control/src/fault_management.rs -> FaultManager::trigger_critical_ground_alert |

---

## B4 - System Performance Monitoring

### Requirement: Benchmark jitter, backlog, and task drift

| What the code does | Where |
|---|---|
| Tracks uplink interval/jitter samples and jitter violations | ground_control/src/performance_tracker.rs -> PerformanceTracker::record_performance_event + apply_event_metrics + detect_performance_violations |
| Tracks telemetry queue length/age and backlog warning/critical states | ground_control/src/performance_tracker.rs -> PerformanceTracker::apply_event_metrics + PerformanceTracker::snapshot_current_stats |
| Tracks scheduler/task execution drift and precision violations | ground_control/src/main.rs -> GroundControlStation::run (Task 7 emits TaskExecutionDrift/SchedulerPrecisionViolation), ground_control/src/performance_tracker.rs -> PerformanceTracker::record_performance_event + apply_event_metrics |

### Requirement: Track deadlines, system load, and recovery metrics

| What the code does | Where |
|---|---|
| Aggregates processing/network/deadline/fault metrics and computes health score | ground_control/src/performance_tracker.rs -> PerformanceTracker::record_performance_event + PerformanceTracker::snapshot_current_stats + PerformanceTracker::compute_system_health_score |
| Samples CPU/memory/load and sends periodic system health events | ground_control/src/system_monitor.rs -> run_system_load_sampler |
| Maintains MTTR/MTBF and auto-recovery counters in fault manager | ground_control/src/fault_management.rs -> FaultManager::resolve_fault_with_outcome + FaultManager::handle_fault + FaultManager::get_stats |

### Requirement: Timestamped real-time logging

| What the code does | Where |
|---|---|
| Uses tracing logs for operational events and transitions | ground_control/src/main.rs -> main (tracing_subscriber setup) + GroundControlStation::run + GroundControlStation::shutdown |
| Stores timestamps on performance/fault/block events | ground_control/src/performance_tracker.rs -> PerformanceEvent::timestamp handling in PerformanceTracker::record_performance_event, ground_control/src/fault_management.rs -> FaultManager::handle_fault + FaultManager::is_command_blocked |
| Emits final runtime summary with timing and reliability metrics | ground_control/src/main.rs -> shutdown |

---

## Shared System Requirements

### Requirement: Log scheduling drift (scheduled vs actual task start)

| What the code does | Where |
|---|---|
| Emits scheduler drift events each RT scheduler tick | ground_control/src/main.rs -> GroundControlStation::run (Task 7 emits EventType::TaskExecutionDrift) |
| Emits scheduler precision-violation events when drift exceeds critical bound | ground_control/src/main.rs -> GroundControlStation::run (Task 7 emits EventType::SchedulerPrecisionViolation) |
| Aggregates drift samples and computes drift percentiles (P95/P99) | ground_control/src/performance_tracker.rs -> PerformanceTracker::record_performance_event + PerformanceTracker::snapshot_current_stats |

### Requirement: Track latency in all pipelines

| Pipeline latency | Where |
|---|---|
| Packet reception latency + drift + jitter | ground_control/src/network_manager.rs -> NetworkManager::compute_reception_timing |
| Packet decode latency (<= 3ms target monitored) | ground_control/src/network_manager.rs -> NetworkManager::receive_packet_with_reception_timing |
| Telemetry processing latency (<= 3ms target monitored) | ground_control/src/telemetry_processor.rs -> TelemetryProcessor::process_telemetry_packet |
| Uplink send latency + urgent deadline check (<= 2ms monitored) | ground_control/src/network_manager.rs -> NetworkManager::send_packet_with_deadline_guard |
| Fault-to-interlock and interlock-to-command-block latency | ground_control/src/fault_management.rs -> FaultManager::is_command_blocked + FaultManager::record_command_block_event |

### Requirement: Record jitter (variation in periodic timing)

| What the code does | Where |
|---|---|
| Captures uplink interval jitter and flags jitter violations | ground_control/src/performance_tracker.rs -> PerformanceTracker::apply_event_metrics + detect_performance_violations |
| Records reception jitter from packet arrival timing | ground_control/src/network_manager.rs -> NetworkManager::compute_reception_timing |
| Reports jitter percentile statistics (P95/P99) | ground_control/src/performance_tracker.rs -> PerformanceTracker::snapshot_current_stats |

### Requirement: Handle simulated faults and log recovery time

| What the code does | Where |
|---|---|
| Injects synthetic emergency faults periodically | ground_control/src/main.rs -> GroundControlStation::launch_fault_simulation_task, ground_control/src/fault_management.rs -> FaultSimulator::create_random_fault + FaultSimulator::create_fault_packet |
| Handles faults and measures fault-response time | ground_control/src/fault_management.rs -> FaultManager::handle_fault |
| Resolves faults and tracks MTTR/MTBF samples | ground_control/src/fault_management.rs -> FaultManager::resolve_fault_with_outcome + FaultManager::get_stats |
| Performs stale-fault auto-recovery sweep | ground_control/src/fault_management.rs -> FaultManager::auto_resolve_stale_faults |

### Requirement: Implement safety interlocks and document all rejected operations

| What the code does | Where |
|---|---|
| Activates and reconciles safety interlocks from active fault state | ground_control/src/fault_management.rs -> FaultManager::activate_safety_interlock + FaultManager::reconcile_safety_interlocks |
| Blocks dispatch when interlocks match command/system categories | ground_control/src/command_scheduler.rs -> CommandScheduler::process_dispatch_queue, ground_control/src/fault_management.rs -> FaultManager::is_command_blocked |
| Logs command rejection reason and emits validation-failed perf event | ground_control/src/command_scheduler.rs -> CommandScheduler::process_dispatch_queue |
| Persists rejected operations to CSV audit log | ground_control/src/fault_management.rs -> FaultManager::append_rejected_operation_to_csv |

### Requirement: Include performance metrics and logs in final report

| What the code does | Where |
|---|---|
| Builds consolidated performance report (health score, percentile metrics, violations) | ground_control/src/performance_tracker.rs -> PerformanceTracker::build_performance_report |
| Produces runtime summary at shutdown for report capture | ground_control/src/main.rs -> GroundControlStation::shutdown |
| Persists rejected-command audit artifacts | ground_control/src/fault_management.rs -> FaultManager::append_rejected_operation_to_csv (logs/ground_control_rejected_ops.csv) |

---

## Current Task Map

| Async task | File | Purpose |
|---|---|---|
| Task 1: network reception | ground_control/src/main.rs -> GroundControlStation::run | Receive/decode/timing events/telemetry queue feed |
| Task 2: telemetry processing | ground_control/src/main.rs -> GroundControlStation::run | Packet analysis, fault extraction, processing metrics |
| Task 3: missing packet re-request | ground_control/src/main.rs -> GroundControlStation::run | Retransmission loop and stale record cleanup |
| Task 4: fault management | ground_control/src/main.rs -> GroundControlStation::launch_fault_management_task + GroundControlStation::run | Fault handling and interlock lifecycle |
| Task 5: performance event aggregation | ground_control/src/main.rs -> GroundControlStation::launch_performance_monitor_task | PerformanceTracker event ingestion |
| Task 6: health heartbeat | ground_control/src/main.rs -> GroundControlStation::run | Periodic status and drift summary logs |
| Task 7: command scheduler | ground_control/src/main.rs -> GroundControlStation::run | Real-time dispatch, deadline monitoring |
| Task 8: fault simulation | ground_control/src/main.rs -> GroundControlStation::launch_fault_simulation_task | Periodic synthetic emergency injection |

---

## Notes On Tuned Timings

| Setting | Current value | Location |
|---|---|---|
| UDP receive timeout | 150ms | ground_control/src/network_manager.rs -> NetworkManager::new_default |
| UDP send timeout | 75ms | ground_control/src/network_manager.rs -> NetworkManager::new_default |
| Timeout-path backoff sleep | 20ms | ground_control/src/main.rs -> GroundControlStation::run (Task 1 timeout branch) |
| Generic receive-error backoff sleep | 75ms | ground_control/src/main.rs -> GroundControlStation::run (Task 1 error branch) |
| Missing packet re-request interval | 650ms | ground_control/src/main.rs -> GroundControlStation::run (Task 3 interval) |

Hard requirement thresholds intentionally unchanged:
- Telemetry/decode budget checks at 3ms
- Urgent command network deadline threshold at 2ms
- Scheduler tick at 0.5ms
- Loss-of-contact threshold at 3 consecutive failures
