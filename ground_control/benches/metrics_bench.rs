// Benchmarking for uplink jitter, telemetry backlog, and task execution drift
// Uses Criterion for robust benchmarking
use criterion::{criterion_group, criterion_main, Criterion};
use std::hint::black_box;
use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
use std::time::{Duration, Instant};

// Simulated intervals (ms) for uplink events (realistic, e.g. 1ms +/- jitter)
fn sample_uplink_intervals() -> Vec<f64> {
    vec![1.0, 1.05, 0.97, 1.1, 0.92, 1.08, 1.0, 1.12, 0.95, 1.03, 1.07, 0.99, 1.01, 1.15, 0.93]
}

// Retransmission uplink jitter calculation (as in PerformanceTracker)
fn calc_uplink_jitter(intervals: &[f64]) -> Vec<f64> {
    let mut jitter_samples = Vec::with_capacity(intervals.len());
    for &interval_ms in intervals {
        let jitter_ms = (interval_ms - 1.0).abs();
        jitter_samples.push(jitter_ms);
    }
    jitter_samples
}


fn bench_uplink_jitter(c: &mut Criterion) {
    let intervals = sample_uplink_intervals();
    c.bench_function("uplink_jitter", |b| {
        b.iter(|| {
            let jitters = calc_uplink_jitter(black_box(&intervals));
            black_box(jitters);
        })
    });
}

fn bench_telemetry_backlog(c: &mut Criterion) {
    let backlog = Arc::new(AtomicUsize::new(0));
    c.bench_function("telemetry_backlog", |b| {
        b.iter(|| {
            backlog.fetch_add(1, Ordering::Relaxed);
            backlog.fetch_sub(1, Ordering::Relaxed);
            black_box(backlog.load(Ordering::Relaxed));
        })
    });
}

// Simulate telemetry backlog age: enqueue times and age calculation
fn sample_enqueue_times() -> Vec<Instant> {
    let start = Instant::now();
    // Simulate packets enqueued every 2ms
    (0..15).map(|i| start + Duration::from_millis(i * 2)).collect()
}

fn bench_telemetry_backlog_age(c: &mut Criterion) {
    let enqueues = sample_enqueue_times();
    // Simulate dequeue at a later time (e.g., after 30ms)
    let dequeue_time = enqueues[0] + Duration::from_millis(30);
    c.bench_function("telemetry_backlog_age", |b| {
        b.iter(|| {
            let ages: Vec<u128> = enqueues.iter()
                .map(|&t| dequeue_time.duration_since(t).as_millis())
                .collect();
            black_box(ages);
        })
    });
}


// Simulate ticks and scheduler start for drift calculation
fn sample_task_execution_drift_data() -> Vec<(u64, f64)> {
    // (tick, actual_elapsed_ms)
    // Simulate a scheduler running at 0.5ms per tick, with some jitter
    vec![
        (1, 0.6), (2, 1.1), (3, 1.5), (4, 2.1), (5, 2.5),
        (6, 3.0), (7, 3.7), (8, 4.1), (9, 4.6), (10, 5.2),
        (11, 5.6), (12, 6.1), (13, 6.7), (14, 7.2), (15, 7.8)
    ]
}

fn bench_task_execution_drift(c: &mut Criterion) {
    let samples = sample_task_execution_drift_data();
    c.bench_function("task_execution_drift", |b| {
        b.iter(|| {
            let drifts: Vec<f64> = samples.iter().map(|&(tick, actual_elapsed_ms)| {
                let expected_elapsed_ms = tick as f64 * 0.5;
                (actual_elapsed_ms - expected_elapsed_ms).abs()
            }).collect();
            black_box(drifts);
        })
    });
}

criterion_group!(benches,
    bench_uplink_jitter,
    bench_telemetry_backlog,
    bench_telemetry_backlog_age,
    bench_task_execution_drift,
);
criterion_main!(benches);
