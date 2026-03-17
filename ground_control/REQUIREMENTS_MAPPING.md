# Ground Control Requirements Mapping

This file maps each Student B and Shared requirement from assignment_text.txt to concrete code in ground_control.

Legend:
- Implemented: clear code equivalent exists in this repository.
- Partial: code support exists, but part of the requirement is documentation/reporting work outside code.
- Missing: no direct code equivalent found.

## Student B Requirements

| ID | Requirement | Status | Code Equivalent | Evidence Path |
|---|---|---|---|---|
| B1.1 | Receive telemetry via IPC/sockets and decode within 3ms | Implemented | main.rs task 1 uses NetworkManager::receive_packet_with_reception_timing; telemetry_processor.rs process_telemetry_packet; performance events PacketDecodeViolation and TelemetryProcessingViolation | Runtime logs and final summary in main.rs shutdown |
| B1.2 | Log packet reception latency and drift | Implemented | network_manager.rs compute_reception_timing and collect_drift_stats; main.rs task 1 logs drift and emits PacketReceived metadata | Runtime drift/jitter logs plus shutdown report |
| B1.3 | Trigger re-requests on missing or delayed packets | Implemented | main.rs task 3 uses telemetry_processor.rs collect_missing_packet_uplink_candidates and collect_delayed_packet_uplink_candidates; network_manager.rs send_retransmission_request | Runtime re-request logs and tracker events |
| B1.4 | Simulate loss of contact after 3 consecutive failures | Implemented | main.rs network task increments failures; fault_management.rs increment_consecutive_failures, has_loss_of_contact_condition, handle_loss_of_contact, record_successful_communication | LOC warnings/errors and fault stats |
| B2.1 | Maintain a real-time command schedule | Implemented | main.rs task 7 uses 0.5ms scheduler tick; command_scheduler.rs schedule_command and process_dispatch_queue | Scheduler runtime logs and periodic reports |
| B2.2 | Enforce urgent command deadlines (<=2ms dispatch) | Implemented | command_scheduler.rs NETWORK_DEADLINE_THRESHOLD_MS and deadline counters; network_manager.rs send_packet_with_deadline_guard | DEADLINE logs and adherence metrics |
| B2.3 | Validate commands against system state using safety interlocks | Implemented | command_scheduler.rs interlock gate in process_dispatch_queue; fault_management.rs is_command_blocked | COMMAND REJECTED logs and validation failure events |
| B2.4 | Log command deadline adherence and rejection reasons | Implemented | command_scheduler.rs get_unified_deadline_report and rejection reason logging; main.rs periodic adherence reporting | Runtime adherence + rejection reason logs |
| B3.1 | Simulate reception of fault messages from satellite | Implemented | telemetry_processor.rs handles PacketPayload::EmergencyAlert and converts to FaultEvent; main.rs forwards detected faults to fault task | Emergency alert and fault intake logs |
| B3.2 | Block unsafe commands until fault resolved | Implemented | fault_management.rs activate_safety_interlock and reconcile_safety_interlocks; command_scheduler.rs discards blocked commands | Block/reject logs and counters |
| B3.3 | Track interlock latency (detection to command block) | Implemented | fault_management.rs is_command_blocked computes latency and record_command_block_event stores it | Runtime latency logs and CSV rows |
| B3.4 | Document and explain all command rejections | Partial | fault_management.rs writes logs/ground_control_rejected_ops.csv; command_scheduler.rs logs rejection reasons | Documentation/explanation narrative is report-side work |
| B3.5 | Trigger critical ground alert if fault response time >100ms | Implemented | fault_management.rs critical_response_time_ms threshold check in handle_fault and trigger_critical_ground_alert | CRITICAL ALERT logs and final summary counter |
| B4.1 | Benchmark uplink jitter, telemetry backlog, and task execution drift | Implemented | main.rs emits UplinkIntervalSample, TelemetryQueueDepthSample, TaskExecutionDrift; performance_tracker.rs aggregates | Final summary metrics and runtime severe drift logs |
| B4.2 | Record missed deadlines, system load, and fault recovery metrics | Implemented | command_scheduler.rs deadline counters; system_monitor.rs system load; fault_management.rs MTTR/MTBF and recovery stats; performance_tracker.rs aggregation | Shutdown summary and runtime health logs |
| B4.3 | Log all real-time actions with timestamps | Implemented | tracing is used across runtime tasks; PerformanceEvent includes timestamp for tracked events | Timestamped runtime logs |

## Shared Requirements

| ID | Requirement | Status | Code Equivalent | Evidence Path |
|---|---|---|---|---|
| S1 | Log scheduling drift (scheduled vs actual) | Implemented | main.rs computes drift each scheduler tick and emits TaskExecutionDrift | Drift logs + shutdown scheduler drift summary |
| S2 | Track latency in all pipelines (sensor->buffer, packet->uplink, command->response) | Partial | packet->uplink via PacketToUplinkLatencySample; command->response via CommandResponseRttSample in main.rs/performance_tracker.rs | sensor->buffer is primarily satellite-side, not implemented in ground_control crate |
| S3 | Record jitter variation in periodic tasks | Implemented | main.rs emits UplinkIntervalSample with jitter; network timing includes jitter | Jitter metrics in summary and logs |
| S4 | Handle simulated faults and log recovery time | Implemented | telemetry emergency/fault events handled by fault_management.rs; recovery stats and resolution timing tracked | Fault and recovery logs plus summary |
| S5 | Implement safety interlocks and document rejected operations | Implemented | fault_management.rs interlock lifecycle + rejected-op CSV append; command_scheduler.rs rejection path | logs/ground_control_rejected_ops.csv and runtime logs |
| S6 | Include all performance metrics and logs in final report | Partial | main.rs shutdown prints consolidated final metrics; logs are persisted during runtime | Final report assembly is a manual deliverable outside code |

## Gaps and Non-Code Deliverables

The following requirements are not fully satisfiable by code alone and require report/documentation work:

1. B3.4 explanation quality: code logs and CSV exist, but written explanation belongs in the final report.
2. S6 final report inclusion: code emits metrics/logs, but selecting and presenting all required evidence is manual.

No fully missing (Status = Missing) Student B runtime requirement was found in ground_control after removing synthetic trigger hooks.

## Comment Coverage Audit (Ground Control Source)

Audit date: 2026-03-17

Result: all Student B and Shared requirement IDs are explicitly listed in source comments.

- Missing requirement IDs from comments: none.
- Scope note for S2: the `sensor->buffer` pipeline is mainly implemented in `satellite_ocs`; Ground Control comments still reference S2 for the `packet->uplink` and `command->response` portions.

### Student B Points (Comment Listing)

| ID | Listed In Comments | Primary Comment Anchors |
|---|---|---|
| B1.1 | Yes | `src/main.rs` (Task 1/Task 2 map), `src/network_manager.rs`, `src/telemetry_processor.rs` |
| B1.2 | Yes | `src/main.rs` (Task 1 map), `src/network_manager.rs`, `src/telemetry_processor.rs` |
| B1.3 | Yes | `src/main.rs` (Task 3 map), `src/network_manager.rs`, `src/telemetry_processor.rs` |
| B1.4 | Yes | `src/main.rs` (Task 1 map), `src/fault_management.rs` |
| B2.1 | Yes | `src/main.rs` (Task 7 map), `src/command_scheduler.rs` |
| B2.2 | Yes | `src/main.rs` (Task 7 map), `src/command_scheduler.rs`, `src/network_manager.rs` |
| B2.3 | Yes | `src/main.rs` (Task 7 map), `src/command_scheduler.rs` |
| B2.4 | Yes | `src/main.rs` (Task 7 map), `src/command_scheduler.rs` |
| B3.1 | Yes | `src/main.rs` (Task 2 map), `src/telemetry_processor.rs` |
| B3.2 | Yes | `src/main.rs` (Task 4/Task 7 map), `src/command_scheduler.rs`, `src/fault_management.rs` |
| B3.3 | Yes | `src/main.rs` (Task 4 map), `src/fault_management.rs` |
| B3.4 | Yes | `src/main.rs` (Task 7 map), `src/command_scheduler.rs`, `src/fault_management.rs` |
| B3.5 | Yes | `src/main.rs` (Task 4 map), `src/fault_management.rs` |
| B4.1 | Yes | `src/main.rs` (top map + Task 5), `src/performance_tracker.rs` |
| B4.2 | Yes | `src/main.rs` (Task 6 map), `src/performance_tracker.rs`, `src/system_monitor.rs` |
| B4.3 | Yes | `src/main.rs` (top map), `src/performance_tracker.rs`, `src/system_monitor.rs` |

### Shared Points (Comment Listing)

| ID | Listed In Comments | Primary Comment Anchors |
|---|---|---|
| S1 | Yes | `src/main.rs` (Task 7 map), `src/network_manager.rs`, `src/performance_tracker.rs` |
| S2 | Yes | `src/main.rs` (Task 1/Task 3 + RTT helper), `src/network_manager.rs`, `src/performance_tracker.rs` |
| S3 | Yes | `src/main.rs` (Task 1/Task 7 map), `src/network_manager.rs`, `src/telemetry_processor.rs`, `src/performance_tracker.rs` |
| S4 | Yes | `src/main.rs` (Task 2/Task 4 map), `src/telemetry_processor.rs`, `src/fault_management.rs`, `src/performance_tracker.rs` |
| S5 | Yes | `src/main.rs` (Task 4/Task 7 map), `src/command_scheduler.rs`, `src/fault_management.rs` |
| S6 | Yes | `src/main.rs` (top map + Task 5), `src/performance_tracker.rs`, `src/system_monitor.rs` |
