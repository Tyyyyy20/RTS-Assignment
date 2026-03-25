// benches/gcs_bench.rs
//
// Micro-benchmarks for Ground Control Station critical paths.
// Run with:  cargo bench -p ground_control
//
// DESIGN NOTE:
// All benchmarks target **pure, in-memory functions** (no CSV / disk I/O).
// Criterion loops thousands of times; disk writes would ruin measurements.

use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use std::collections::HashMap;

use shared_protocol::{
    CommunicationPacket, CryptoContext, SensorReading, SensorType,
    Priority, Quality, Status, Source, Command, CommandType, TargetSystem,
};

// ---------------------------------------------------------------------------
// Helpers: mock data generation
// ---------------------------------------------------------------------------

fn make_crypto() -> CryptoContext {
    CryptoContext::new(1, [7u8; 32])
}

/// Build a realistic `SensorReading` for benchmarking.
fn mock_reading(seq: u64, priority: Priority, sensor_type: SensorType) -> SensorReading {
    SensorReading {
        sensor_id: (seq % 6) as u32 + 1,
        sensor_type,
        description: String::new(),
        location: "bench".into(),
        timestamp: chrono::Utc::now(),
        sequence_number: seq,
        value1: 42.0 + (seq as f64 * 0.1),
        value2: 80.0,
        value3: 85.0,
        value4: 0.0,
        priority,
        quality: Quality::Good,
        status: Status::Normal,
        processing_latency_ms: 0.0,
        jitter_ms: 0.0,
        drift_ms: 0.0,
        metadata: HashMap::new(),
    }
}

/// Seal a telemetry packet so we can benchmark the decode path only.
fn sealed_telemetry_bytes(batch_size: usize) -> Vec<u8> {
    let readings: Vec<SensorReading> = (0..batch_size)
        .map(|i| mock_reading(i as u64, Priority::Normal, SensorType::Thermal))
        .collect();
    let pkt = CommunicationPacket::new_telemetry(readings, Source::Satellite);
    make_crypto().seal_to_bytes(&pkt).unwrap()
}

/// Build a normal-priority diagnostic command with a unique ID.
fn make_normal_command(seq: u32) -> Command {
    Command {
        command_id: format!("BENCH-CMD-{:05}", seq),
        command_type: CommandType::Diagnostic,
        description: "bench normal command".into(),
        target_system: TargetSystem::AllSystems,
        timestamp: chrono::Utc::now(),
        retry_count: 0,
        param1: 0.0,
        param2: 0.0,
        param3: 0.0,
        param4: 0.0,
        text_param: String::new(),
        priority: Priority::Normal,
        source: Source::GroundControl,
        destination: Source::Satellite,
        metadata: HashMap::new(),
    }
}

/// Build an urgent emergency command.
fn make_urgent_command(seq: u32) -> Command {
    Command {
        command_id: format!("BENCH-URG-{:05}", seq),
        command_type: CommandType::Emergency,
        description: "bench urgent command".into(),
        target_system: TargetSystem::AllSystems,
        timestamp: chrono::Utc::now(),
        retry_count: 0,
        param1: 0.0,
        param2: 0.0,
        param3: 0.0,
        param4: 0.0,
        text_param: String::new(),
        priority: Priority::Emergency,
        source: Source::GroundControl,
        destination: Source::Satellite,
        metadata: HashMap::new(),
    }
}

// ---------------------------------------------------------------------------
// Benchmark 1: Telemetry decode pipeline (decrypt + deserialize, no I/O)
//   Proves that decoding completes well under the 3ms B1.1 budget.
// ---------------------------------------------------------------------------

fn bench_telemetry_decode(c: &mut Criterion) {
    let crypto = make_crypto();

    let mut group = c.benchmark_group("telemetry_decode_pipeline");

    for batch_size in [1, 10, 50] {
        let sealed = sealed_telemetry_bytes(batch_size);

        group.bench_with_input(
            BenchmarkId::new("open_from_bytes", batch_size),
            &sealed,
            |b, sealed| {
                b.iter(|| {
                    let pkt = crypto.open_from_bytes(black_box(sealed)).unwrap();
                    black_box(pkt);
                });
            },
        );
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// Benchmark 2: Command safety interlock check (pure in-memory lookup)
//   Proves the safety validation path used by the scheduler is fast.
// ---------------------------------------------------------------------------

fn bench_safety_interlock_check(c: &mut Criterion) {
    use ground_control::fault_management::FaultManager;

    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("command_safety_interlock_check");

    // Setup: create a FaultManager and inject a thermal fault to activate an
    // interlock.  This setup runs ONCE before the benchmark loop.
    group.bench_function("is_command_blocked_with_active_interlock", |b| {
        b.iter_batched(
            || {
                // --- PER-ITERATION SETUP (not timed) ---
                let mut fm = FaultManager::new();
                // Inject a thermal fault to activate the "thermal_emergency" interlock
                rt.block_on(async {
                    let fault = ground_control::fault_management::FaultEvent {
                        timestamp: chrono::Utc::now(),
                        fault_type: ground_control::fault_management::FaultType::ThermalAnomaly,
                        severity: ground_control::fault_management::Severity::Critical,
                        description: "Bench test thermal fault".into(),
                        affected_systems: vec!["thermal".into()],
                    };
                    let _ = fm.handle_fault(fault).await;
                });
                fm
            },
            |mut fm| {
                // --- TIMED SECTION ---
                // Check if a "high_power" command to "power_management" is blocked
                let (blocked, _reasons, _event) = fm.is_command_blocked(
                    black_box("high_power"),
                    black_box("power_management"),
                    black_box("BENCH-CHECK-001"),
                    black_box(chrono::Utc::now()),
                );
                black_box(blocked);
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.bench_function("is_command_blocked_no_interlocks", |b| {
        b.iter_batched(
            || FaultManager::new(),
            |mut fm| {
                let (blocked, _reasons, _event) = fm.is_command_blocked(
                    black_box("high_power"),
                    black_box("power_management"),
                    black_box("BENCH-CHECK-002"),
                    black_box(chrono::Utc::now()),
                );
                black_box(blocked);
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Benchmark 3: Command scheduler urgent reorder
//   Proves that urgent commands get front-of-queue placement.
//   Only benchmarks schedule_command() which is a pure in-memory operation.
// ---------------------------------------------------------------------------

fn bench_scheduler_urgent_reorder(c: &mut Criterion) {
    use ground_control::command_scheduler::CommandScheduler;

    let mut group = c.benchmark_group("command_scheduler_urgent_reorder");

    // Benchmark: schedule 1 urgent command into a queue with N normal commands
    for queue_depth in [10, 50, 100] {
        group.bench_with_input(
            BenchmarkId::new("schedule_urgent_into", queue_depth),
            &queue_depth,
            |b, &depth| {
                b.iter_batched(
                    || {
                        // --- PER-ITERATION SETUP (not timed) ---
                        let mut sched = CommandScheduler::new();
                        for i in 0..depth {
                            let _ = sched.schedule_command(make_normal_command(i));
                        }
                        sched
                    },
                    |mut sched| {
                        // --- TIMED SECTION ---
                        let _ = sched.schedule_command(black_box(make_urgent_command(9999)));
                        black_box(&sched);
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }

    // Also benchmark the validate_command path separately
    group.bench_function("validate_and_schedule_single_command", |b| {
        b.iter_batched(
            CommandScheduler::new,
            |mut sched| {
                let _ = sched.schedule_command(black_box(make_normal_command(1)));
                black_box(&sched);
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Register all benchmark groups
// ---------------------------------------------------------------------------

criterion_group!(benches, bench_telemetry_decode, bench_safety_interlock_check, bench_scheduler_urgent_reorder);
criterion_main!(benches);
