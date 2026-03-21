
# Ground Control Requirements Mapping (2026-03-22)

This file maps each Student B and Shared requirement from assignment_text.txt to the current codebase. Status and evidence are up-to-date as of the latest system changes.

Legend:
- **Implemented**: Fully satisfied by code and logs.
- **Partial**: Code/logs exist, but some reporting or documentation is manual.
- **Not Applicable**: Not required or handled in this component.

## Student B Requirements

| ID   | Requirement                                                                 | Status      | Code Equivalent & Notes                                                                                                                                                                                                 | Evidence Path/Log |
|------|-----------------------------------------------------------------------------|-------------|------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|-------------------|
| B1.1 | Receive telemetry via IPC/sockets and decode within 3ms                     | Implemented | main.rs (Task 1), network_manager.rs (receive_packet_with_reception_timing), telemetry_processor.rs (process_telemetry_packet), performance events (PacketDecodeViolation, TelemetryProcessingViolation)              | Runtime logs, shutdown summary |
| B1.2 | Log packet reception latency and drift                                      | Implemented | network_manager.rs (compute_reception_timing, collect_drift_stats), main.rs (drift/jitter logs, PacketReceived events)                                                                                                | Drift/jitter logs, shutdown report |
| B1.3 | Trigger re-requests on missing or delayed packets                           | Implemented | main.rs (Task 3), telemetry_processor.rs (collect_missing_packet_uplink_candidates, collect_delayed_packet_uplink_candidates), network_manager.rs (send_retransmission_request)                                        | Re-request logs, tracker events |
| B1.4 | Simulate loss of contact after 3 consecutive failures                       | Implemented | main.rs (network failure count), fault_management.rs (increment_consecutive_failures, has_loss_of_contact_condition, handle_loss_of_contact, record_successful_communication). **Note:** No safety interlock is triggered for LOC; only logging and recovery logic are executed. | LOC warnings/errors, fault stats |
| B2.1 | Maintain a real-time command schedule                                       | Implemented | main.rs (Task 7, scheduler tick), command_scheduler.rs (schedule_command, process_dispatch_queue)                                                                               | Scheduler logs, periodic reports |
| B2.2 | Enforce urgent command deadlines (<=2ms dispatch)                           | Implemented | command_scheduler.rs (NETWORK_DEADLINE_THRESHOLD_MS, deadline counters), network_manager.rs (send_packet_with_deadline_guard)                                                   | Deadline logs, adherence metrics |
| B2.3 | Validate commands against system state using safety interlocks               | Implemented | command_scheduler.rs (interlock gate in process_dispatch_queue), fault_management.rs (is_command_blocked)                                                                       | Command rejected logs, validation events |
| B2.4 | Log command deadline adherence and rejection reasons                        | Implemented | command_scheduler.rs (get_unified_deadline_report, rejection reason logging), main.rs (adherence reporting)                                                                     | Adherence/rejection logs |
| B3.1 | Simulate reception of fault messages from satellite                         | Implemented | telemetry_processor.rs (PacketPayload::EmergencyAlert → FaultEvent), main.rs (fault forwarding)                                                                                  | Emergency alert/fault intake logs |
| B3.2 | Block unsafe commands until fault resolved                                  | Implemented | fault_management.rs (activate_safety_interlock, reconcile_safety_interlocks), command_scheduler.rs (blocked command discard). **Note:** Loss of contact does NOT trigger a safety interlock. | Block/reject logs, counters |
| B3.3 | Track interlock latency (detection to command block)                        | Implemented | fault_management.rs (is_command_blocked, record_command_block_event)                                                                                                            | Latency logs, CSV rows |
| B3.4 | Document and explain all command rejections                                 | Partial     | fault_management.rs (logs/ground_control_rejected_ops.csv), command_scheduler.rs (rejection reason logs). Written explanation is a report deliverable.                           | CSV, logs, report |
| B3.5 | Trigger critical ground alert if fault response time >100ms                 | Implemented | fault_management.rs (critical_response_time_ms, handle_fault, trigger_critical_ground_alert)                                                                                    | Critical alert logs, summary counter |
| B4.1 | Benchmark uplink jitter, telemetry backlog, and task execution drift        | Implemented | main.rs (UplinkIntervalSample, TelemetryQueueDepthSample, TaskExecutionDrift), performance_tracker.rs (aggregation)                                                             | Metrics, severe drift logs |
| B4.2 | Record missed deadlines, system load, and fault recovery metrics            | Implemented | command_scheduler.rs (deadline counters), system_monitor.rs (system load), fault_management.rs (MTTR/MTBF, recovery stats), performance_tracker.rs (aggregation)                | Health logs, shutdown summary |
| B4.3 | Log all real-time actions with timestamps                                   | Implemented | tracing (across runtime tasks), performance_tracker.rs (PerformanceEvent timestamps)                                                                                            | Timestamped logs |

## Shared Requirements

| ID  | Requirement                                                        | Status      | Code Equivalent & Notes                                                                                                 | Evidence Path/Log |
|-----|--------------------------------------------------------------------|-------------|------------------------------------------------------------------------------------------------------------------------|-------------------|
| S1  | Log scheduling drift (scheduled vs actual)                         | Implemented | main.rs (scheduler tick, TaskExecutionDrift events)                                                                     | Drift logs, shutdown summary |
| S2  | Track latency in all pipelines (sensor->buffer, packet->uplink, command->response) | Partial     | packet->uplink (PacketToUplinkLatencySample), command->response (CommandResponseRttSample in main.rs/performance_tracker.rs). sensor->buffer is satellite-side only. | Latency logs, partial coverage |
| S3  | Record jitter variation in periodic tasks                          | Implemented | main.rs (UplinkIntervalSample, jitter metrics), network_manager.rs (timing/jitter)                                      | Jitter metrics, logs |
| S4  | Handle simulated faults and log recovery time                      | Implemented | telemetry_processor.rs (emergency/fault events), fault_management.rs (recovery stats, resolution timing)                | Fault/recovery logs, summary |
| S5  | Implement safety interlocks and document rejected operations       | Implemented | fault_management.rs (interlock lifecycle, logs/ground_control_rejected_ops.csv), command_scheduler.rs (rejection path). **Note:** Not all faults (e.g., loss of contact) trigger interlocks. | CSV, logs |
| S6  | Include all performance metrics and logs in final report           | Partial     | main.rs (shutdown summary), logs (persisted during runtime). Final report assembly is a manual deliverable.             | Final report/manual |

## Gaps and Non-Code Deliverables

Some requirements require manual reporting or documentation:

1. **B3.4**: Written explanation of command rejections is a report deliverable.
2. **S6**: Final report must manually assemble and present all required evidence.

No fully missing (Status = Not Applicable) Student B runtime requirement was found in ground_control after removing synthetic trigger hooks and updating loss of contact logic.

## Comment Coverage Audit (Ground Control Source)

All Student B and Shared requirement IDs are explicitly listed in source comments.

- No missing requirement IDs from comments.
- S2: The `sensor->buffer` pipeline is implemented in `satellite_ocs`; ground_control covers `packet->uplink` and `command->response` only.

### Shared Points (Comment Listing)

| ID | Listed In Comments | Primary Comment Anchors |
|---|---|---|
| S1 | Yes | `src/main.rs` (Task 7 map), `src/network_manager.rs`, `src/performance_tracker.rs` |
| S2 | Yes | `src/main.rs` (Task 1/Task 3 + RTT helper), `src/network_manager.rs`, `src/performance_tracker.rs` |
| S3 | Yes | `src/main.rs` (Task 1/Task 7 map), `src/network_manager.rs`, `src/telemetry_processor.rs`, `src/performance_tracker.rs` |
| S4 | Yes | `src/main.rs` (Task 2/Task 4 map), `src/telemetry_processor.rs`, `src/fault_management.rs`, `src/performance_tracker.rs` |
| S5 | Yes | `src/main.rs` (Task 4/Task 7 map), `src/command_scheduler.rs`, `src/fault_management.rs` |
| S6 | Yes | `src/main.rs` (top map + Task 5), `src/performance_tracker.rs`, `src/system_monitor.rs` |
