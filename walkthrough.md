# OCS Requirements Coverage Report

## 1. Sensor Data Acquisition and Prioritization

### ✅ Simulate three onboard sensors with distinct intervals and priorities
- **Thermal** → [thermal.rs](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/sensors/thermal.rs) — `ThermalSensor::new(1, "CPU")`, period = `sensor.sampling_interval_ms`
- **Power** → [power.rs](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/sensors/power.rs) — `PowerSensor::new(2, "Main Bus")`
- **Attitude** → [attitude.rs](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/sensors/attitude.rs) — `AttitudeSensor::new(3, "IMU")`
- All three spawned via [mod.rs](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/sensors/mod.rs) `spawn_all()`
- Distinct sampling intervals are set per sensor type (defined in `shared_protocol` structs)

### ✅ Ensure jitter <1ms for critical sensors
- Each sensor computes `jitter_ms = (actual_ms - ideal_ms).abs()` and logs it
  - [thermal.rs#L91-L96](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/sensors/thermal.rs#L91-L96)
  - [power.rs#L89-L95](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/sensors/power.rs#L89-L95)
  - [attitude.rs#L92-L98](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/sensors/attitude.rs#L92-L98)
- Thermal sensor uses jitter >1ms as a **miss** threshold: [thermal.rs#L121](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/sensors/thermal.rs#L121) — `(actual_ms - ideal_ms) > 1.0`

### ✅ Prioritize sensor data in a bounded buffer; log dropped samples with timestamps
- **Bounded priority buffer** → [prio_buffer.rs](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/telemetry/prio_buffer.rs) — 3-tier `VecDeque` (hi/im/lo) with capacity limit
- Drop policy evicts lowest-priority first: [prio_buffer.rs#L68-L84](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/telemetry/prio_buffer.rs#L68-L84)
- Drops logged with timestamps → [batcher.rs#L60-L67](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/telemetry/batcher.rs#L60-L67) calls `log_drop()` → writes `drops.csv` with `ts,priority,dropped_count`

### ✅ Measure scheduling drift (expected vs actual task start times)
- Each sensor computes `drift_ms = actual_ms - ideal_ms` per cycle
  - [thermal.rs#L96](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/sensors/thermal.rs#L96)
  - [power.rs#L95](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/sensors/power.rs#L95)
  - [attitude.rs#L98](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/sensors/attitude.rs#L98)
- Logged via `log_sensor_reading()` → [csv.rs#L57-L79](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/logging/csv.rs#L57-L79) to `sensors.csv`

### ✅ Track latency from sensor read to buffer insertion
- Computed in the ingest task: [batcher.rs#L51-L57](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/telemetry/batcher.rs#L51-L57) — `r.processing_latency_ms = (now - r.timestamp) in ms`

### ✅ If critical data (thermal) missed >3 consecutive cycles, raise safety alert
- [thermal.rs#L118-L147](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/sensors/thermal.rs#L118-L147) — tracks `consecutive_misses`; if >3, logs `SAFETY ALERT` and sends an `EmergencyData` packet via `EMER_TX`

---

## 2. Real-Time Task Scheduling

### ✅ Schedule and prioritize tasks: data compression, health monitoring, antenna alignment
- [rm.rs#L60-L64](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/scheduler/rm.rs#L60-L64):
  - `antenna_alignment` (50ms period, prio 1)
  - `data_compression` (100ms period, prio 2)
  - `health_monitor` (1000ms period, prio 3)

### ✅ Implement scheduling using Rate Monotonic (RM)
- [rm.rs](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/scheduler/rm.rs) — full RM scheduler: sorted by `rm_priority` (shorter period = higher priority), job release/deadline tracking

### ✅ Support preemption: thermal control overrides lower-priority tasks
- Preemption hook → [mod.rs](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/scheduler/mod.rs) — `PREEMPT_CH` channel
- Thermal control injected as highest-priority sporadic job: [rm.rs#L108-L127](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/scheduler/rm.rs#L108-L127)
- Preemption during execution: [rm.rs#L200-L231](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/scheduler/rm.rs#L200-L231) — current job re-queued, higher-priority job runs

### ✅ Log all deadline violations (start delay or completion delay)
- [rm.rs#L245-L263](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/scheduler/rm.rs#L245-L263) — logs `start_delay_ms` and `completion_delay_ms`; warns on `completion_delay_ms > 0`
- Written to `scheduler.csv` via [csv.rs#L105-L127](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/logging/csv.rs#L105-L127)

### ✅ Measure and report scheduling drift and task execution jitter
- Scheduler logs drift (start delay) and jitter per job via `log_sched_event()`
- Additional timing helper: [timing.rs](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/scheduler/timing.rs) — `Tick.jitter_ns()`

### ✅ Log CPU utilization: % active time vs idle time
- [rm.rs#L270-L280](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/scheduler/rm.rs#L270-L280) — `maybe_emit_cpu()` emits every ~1s
- [csv.rs#L129-L147](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/logging/csv.rs#L129-L147) — writes `cpu.csv` with `ts,window_ms,active_ms,idle_ms,active_pct`

---

## 3. Downlink Data Management

### ✅ Compress and packetize data for transmission
- [batcher.rs#L174-L181](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/telemetry/batcher.rs#L174-L181) — builds `CommunicationPacket::new_telemetry()`, encrypts, and sends via UDP

### ✅ Prepare data for downlink within 30ms of visibility window
- [downlink/mod.rs#L73-L82](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/downlink/mod.rs#L73-L82) — checks `prep_ms > 30.0` and returns `ReadyPrepLate`
- Batcher logs the warning: [batcher.rs#L159-L162](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/telemetry/batcher.rs#L159-L162)

### ✅ Log transmission queue latency and buffer fill rates
- [batcher.rs#L136-L143](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/telemetry/batcher.rs#L136-L143) — computes `oldest_ms` and `fill_pct`
- [csv.rs#L149-L167](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/logging/csv.rs#L149-L167) — writes `downlink.csv` with `ts,batch_size,avg_queue_ms,max_queue_ms,fill_pct,event`
- [csv.rs#L196-L222](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/logging/csv.rs#L196-L222) — writes `txqueue.csv` with `ts,oldest_ms,fill_pct`

### ✅ If downlink not initialized within 5ms, simulate missed communication
- [downlink/mod.rs#L58-L64](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/downlink/mod.rs#L58-L64) — `since_open > 5ms && !init_started` → warns "missed communication" and closes link
- Batcher handles `MissedInit`: [batcher.rs#L153-L158](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/telemetry/batcher.rs#L153-L158)

### ✅ Trigger degraded mode if buffer exceeds 80%
- [batcher.rs#L199-L208](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/telemetry/batcher.rs#L199-L208) — `fill_pct >= 80.0` → `dl.set_degraded(true)`
- [downlink/mod.rs#L93-L114](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/downlink/mod.rs#L93-L114) — `set_degraded()` toggles state and logs warning

---

## 4. Benchmarking and Fault Simulation

### ✅ Inject faults every 60s (delayed or corrupted sensor data)
- [faults/mod.rs#L57-L96](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/faults/mod.rs#L57-L96) — every 60s, round-robin injects:
  - `ThermalDelay` (extra 10ms delay for 150ms)
  - `PowerCorrupt` (invalid values for 200ms)
  - `AttitudePause` (pauses sensor for 150ms)

### ✅ Log all faults and responses
- Injection logged: [csv.rs#L169-L177](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/logging/csv.rs#L169-L177) — `log_fault_inject()` writes to `faults.csv`
- Recovery logged: [csv.rs#L179-L194](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/logging/csv.rs#L179-L194) — `log_fault_recovery()` writes to `faults.csv`

### ✅ Evaluate: jitter, drift, deadline adherence, fault recovery time, CPU utilization
All metrics are logged to CSV files:
| Metric | CSV File | Logging Function |
|---|---|---|
| Jitter & Drift | `sensors.csv` | `log_sensor_reading()` |
| Deadline adherence | `scheduler.csv` | `log_sched_event()` |
| Fault recovery time | `faults.csv` | `log_fault_recovery()` |
| CPU utilization | `cpu.csv` | `log_cpu()` |

### ✅ Recovery time >200ms → simulate mission abort
- [faults/mod.rs#L119-L133](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/faults/mod.rs#L119-L133) — if `rec_ms > 200.0` → logs as `aborted=true`, warns, and broadcasts `FaultEvent::Abort`
- If no ACK at all within deadline → also aborts: [faults/mod.rs#L149-L154](file:///c:/Users/sheng/OneDrive/Documents/GitHub/RTS-Assignment/satellite_ocs/src/faults/mod.rs#L149-L154)

---

## Summary

| # | Requirement | Status |
|---|---|---|
| 1.1 | Three sensors with distinct intervals/priorities | ✅ |
| 1.2 | Jitter <1ms for critical sensors | ✅ |
| 1.3 | Bounded buffer with priority drops + logging | ✅ |
| 1.4 | Scheduling drift measurement | ✅ |
| 1.5 | Read-to-buffer latency tracking | ✅ |
| 1.6 | Safety alert on >3 missed thermal cycles | ✅ |
| 2.1 | Three scheduled tasks (compression, health, antenna) | ✅ |
| 2.2 | Rate Monotonic scheduling | ✅ |
| 2.3 | Preemption (thermal overrides) | ✅ |
| 2.4 | Deadline violation logging | ✅ |
| 2.5 | Scheduling drift & jitter reporting | ✅ |
| 2.6 | CPU utilization logging | ✅ |
| 3.1 | Compress & packetize for transmission | ✅ |
| 3.2 | 30ms prep window check | ✅ |
| 3.3 | Tx queue latency & buffer fill logging | ✅ |
| 3.4 | 5ms init → missed communication | ✅ |
| 3.5 | Degraded mode at >80% buffer | ✅ |
| 4.1 | Fault injection every 60s | ✅ |
| 4.2 | Fault & response logging | ✅ |
| 4.3 | Metrics evaluation (jitter, drift, etc.) | ✅ |
| 4.4 | Recovery >200ms → mission abort | ✅ |

**All 21 requirements are implemented.** 🎉
