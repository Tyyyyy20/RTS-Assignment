# Ground Control Requirements Mapping and Evidence Integrity

This file is a practical traceability matrix for Student B and shared real-time requirements.
For each requirement, it records:

1. Where implementation lives in code.
2. Where evidence appears (runtime log, final summary, CSV artifact).
3. Whether evidence is natural, simulated, or mixed.
4. Whether the evidence can be faked/simulated by test hooks.

## Evidence Confidence Legend

- Natural: Produced by live telemetry/network/scheduler behavior without test injection.
- Simulated: Produced by explicit synthetic injection logic.
- Mixed: Natural pipeline exists, but synthetic paths also exist and can produce evidence.

## B1 Telemetry Reception and Decoding

| ID | Requirement | Implementation (Code) | Output and Recording | Confidence / Fakability |
|---|---|---|---|---|
| B1.1 | Receive telemetry over sockets and decode within 3ms budget | `ground_control/src/network_manager.rs` (`NetworkManager::new_default`, `receive_packet_with_reception_timing`), `ground_control/src/main.rs` Task 1, `ground_control/src/telemetry_processor.rs` (`process_telemetry_packet`) | Runtime logs from Task 1 and Task 2; final summary includes processing time and violations in `GroundControlSystem::shutdown` | Mixed. Live traffic drives real values, but synthetic packets can also feed the same path. |
| B1.2 | Log reception latency and drift | `ground_control/src/network_manager.rs` (`compute_reception_timing`, `collect_drift_stats`) | Runtime drift logs (including severe drift), final summary line `Network Performance Report` in `ground_control/src/main.rs` | Mixed. Natural on real traffic; can be influenced by synthetic command/fault load. |
| B1.3 | Re-request missing/delayed packets | `ground_control/src/main.rs` Task 3 loop, `ground_control/src/telemetry_processor.rs` (`collect_missing_packet_uplink_candidates`, pruning), `ground_control/src/network_manager.rs` (`send_retransmission_request`) | Runtime logs for retransmission attempts/failures; final summary `Retransmission Requests Issued`; performance events in tracker | Natural-first. Mostly real behavior, but depends on missing-packet generation patterns (which can be induced). |
| B1.4 | Simulate loss-of-contact after consecutive failures | `ground_control/src/fault_management.rs` (`increment_consecutive_failures`, `has_loss_of_contact_condition`, `handle_loss_of_contact`, `record_successful_communication`) | Runtime error/warn logs for LOC declaration and recovery; reflected in fault stats and health logs | Mixed. This requirement explicitly includes simulation behavior. |

## B2 Command Uplink Scheduler

| ID | Requirement | Implementation (Code) | Output and Recording | Confidence / Fakability |
|---|---|---|---|---|
| B2.1 | Maintain real-time command schedule | `ground_control/src/command_scheduler.rs` (`schedule_command`, `process_dispatch_queue`), `ground_control/src/main.rs` Task 7 (0.5ms tick) | Runtime scheduler logs and periodic `COMMAND SCHEDULER REPORT`; summary scheduler metrics | Natural. Queue behavior is from live dispatch workload. |
| B2.2 | Enforce urgent deadlines (2ms network send target) | `ground_control/src/command_scheduler.rs` (`NETWORK_DEADLINE_THRESHOLD_MS`, violation counters), `ground_control/src/network_manager.rs` (`send_packet_with_deadline_guard`) | Runtime `DEADLINE CRITICAL/URGENT` and `COMMAND SCHEDULER REPORT`; final summary scheduler drift and network/deadline metrics | Natural. No current forced deadline injection path in scheduler (intentional deadline test was removed). |
| B2.3 | Validate commands with safety interlocks | `ground_control/src/command_scheduler.rs` (`map_command_type_to_interlock_category`, `map_target_system_to_interlock_category`, interlock gate in `process_dispatch_queue`), `ground_control/src/fault_management.rs` (`is_command_blocked`) | Runtime `COMMAND REJECTED` logs with reason; `CommandValidationFailed` performance events | Mixed. Real interlocks work naturally; synthetic unsafe command injection can deliberately produce rejections. |
| B2.4 | Log deadline adherence and rejection reasons | `ground_control/src/main.rs` (deadline alerts + scheduler adherence report), `ground_control/src/command_scheduler.rs` (`COMMAND REJECTED ... reason`) | Runtime adherence percentages and rejection reasons; rejected-op CSV for blocked commands | Mixed. Adherence is natural; rejection stream can be naturally or synthetically driven. |

## B3 Fault Management and Interlocks

| ID | Requirement | Implementation (Code) | Output and Recording | Confidence / Fakability |
|---|---|---|---|---|
| B3.1 | Process fault messages and maintain active/resolved state | `ground_control/src/main.rs` (`launch_fault_management_task`), `ground_control/src/fault_management.rs` (`handle_fault`, `resolve_fault_with_outcome`, `get_stats`) | Runtime fault intake/processing logs; final summary fault and recovery lines | Mixed. Real telemetry faults and synthetic faults both use same path. |
| B3.2 | Block unsafe commands until fault/interlock clears | `ground_control/src/fault_management.rs` (`activate_safety_interlock`, `reconcile_safety_interlocks`), `ground_control/src/command_scheduler.rs` interlock gate in `process_dispatch_queue` | Runtime command discard/rejection logs; blocked count in stats; rejected-op CSV | Mixed. Natural under real faults, plus explicit synthetic unsafe command injection exists. |
| B3.3 | Track interlock latency and block timing | `ground_control/src/fault_management.rs` (`is_command_blocked`, `record_command_block_event`) | Runtime latency logs (`F->I`, `I->B`, total); persistent CSV `logs/ground_control_rejected_ops.csv` via `append_rejected_operation_to_csv` | High confidence for recorded rows. CSV is persistent evidence once block occurs. |
| B3.4 | Trigger critical ground alert when fault response >100ms | `ground_control/src/fault_management.rs` (`critical_response_time_ms = 100.0`, threshold check in `handle_fault`, `trigger_critical_ground_alert`) | Runtime `CRITICAL ALERT: Fault Response Time Exceeded`; final summary line `Critical Ground Alerts (>100ms Fault Response)` in `ground_control/src/main.rs` | Mixed. Natural if real processing exceeds threshold; also explicit synthetic marker path exists (see Simulation Hooks). |

## B4 Performance Monitoring and Reporting

| ID | Requirement | Implementation (Code) | Output and Recording | Confidence / Fakability |
|---|---|---|---|---|
| B4.1 | Benchmark jitter, backlog, and drift | `ground_control/src/performance_tracker.rs` (event aggregation and percentiles), `ground_control/src/main.rs` (Task 7 drift events, summary lines) | Final summary includes `Uplink Jitter`, `Telemetry Backlog Queue`, `Scheduler Drift`; runtime severe drift windows | Mixed. Metrics are computed from event stream; synthetic activity can affect numbers. |
| B4.2 | Track latency across pipelines (packet->uplink and command->response) | `ground_control/src/main.rs` Task 3 + Task 7 event emission, `ground_control/src/performance_tracker.rs` latency samples and percentiles, `ground_control/src/telemetry_processor.rs` ack/missing-packet candidate support | Final summary lines `Packet-To-Uplink Latency` and `Command-To-Response Latency` | Mixed. Derived metrics are real within pipeline, but source traffic can be synthetic. |
| B4.3 | Track system health and recovery indicators | `ground_control/src/main.rs` health task, `ground_control/src/performance_tracker.rs` health score, `ground_control/src/fault_management.rs` MTTR/MTBF/recovery counters | Runtime `Health: pkts=...`; final summary `Fault Recovery` and `System Health Score` | Natural for ongoing run; still dependent on scenario realism. |
| B4.4 | Include required performance metrics in final ground report | `ground_control/src/main.rs` (`GroundControlSystem::shutdown`) | `=== FINAL GROUND CONTROL SUMMARY ===` section and all summary lines | High confidence for existence. Value realism depends on workload and simulation settings. |

## Shared Requirements

| ID | Requirement | Implementation (Code) | Output and Recording | Confidence / Fakability |
|---|---|---|---|---|
| S1 | Log scheduling drift (expected vs actual start) | `ground_control/src/main.rs` Task 7 drift computation and `TaskExecutionDrift` event; `ground_control/src/performance_tracker.rs` aggregation | Runtime severe drift windows and final summary `Scheduler Drift` | Natural with runtime jitter; not hardcoded values. |
| S2 | Record jitter and latency metrics with timestamps | `ground_control/src/network_manager.rs`, `ground_control/src/performance_tracker.rs`, `ground_control/src/main.rs` tracer setup | Timestamped tracing output; summary metrics at shutdown | Mixed. Timestamping is real; workloads can be synthetic. |
| S3 | Implement safety interlocks and document rejected operations | `ground_control/src/fault_management.rs` interlock lifecycle + CSV append; `ground_control/src/command_scheduler.rs` rejection path | Runtime rejection reason logs + CSV file `logs/ground_control_rejected_ops.csv` | High confidence once a blocked command occurs. |

## Current Simulation Hooks and Their Impact

These hooks are intentional test mechanisms and can generate evidence without real external conditions:

1. Periodic random fault packet injection:
	`ground_control/src/main.rs` (`launch_fault_simulation_task`) and `ground_control/src/fault_management.rs` (`FaultSimulator::create_random_fault`).
2. Synthetic critical ground-alert trigger every 60s:
	`ground_control/src/main.rs` health task marker `SIM_CRITICAL_GROUND_ALERT_100MS`.
3. Forced >100ms processing for that marker:
	`ground_control/src/fault_management.rs` in `handle_fault` (sleep for marker fault).
4. Synthetic unsafe command after synthetic fault to force interlock block/rejected-op evidence:
	`ground_control/src/main.rs` health task injects `sensor_self_test` after synthetic fault.

Operational implication:
- If these hooks are enabled, evidence for critical alerts and rejected ops is valid for requirement demonstration but should be labeled as simulated evidence in reports.

## Evidence Collection Checklist for Report

Use this checklist to avoid overstating simulated data:

1. Capture final summary section from `GroundControlSystem::shutdown`.
2. Capture runtime logs showing deadline adherence and command rejection reasons.
3. Attach `logs/ground_control_rejected_ops.csv` for interlock latency evidence.
4. Explicitly label whether run mode included synthetic hooks listed above.
5. If claiming natural behavior, provide a run where synthetic hooks are disabled.

## Notes

- This map documents where evidence is produced, not whether the current run succeeded end-to-end.
- In this workspace, compile checks pass, but several `cargo run --release` attempts have exited with code 1; runtime evidence quality depends on successful runs.
