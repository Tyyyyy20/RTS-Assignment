use criterion::{criterion_group, criterion_main, Criterion};
use std::cmp::Ordering;
use std::time::Duration;
use tokio::time::Instant as TokioInstant;

// Simplified Job struct for benchmarking
#[derive(Clone)]
struct Job {
    task_idx: usize,
    deadline: TokioInstant,
    rm_priority: u8,
}

// Simplified task struct
struct Task {
    rm_priority: u8,
}

// Benchmark the sorting of ready queue (key performance bottleneck in RM scheduler)
fn bench_ready_queue_sort(c: &mut Criterion) {
    c.bench_function("ready_queue_sort_10_jobs", |b| {
        let now = TokioInstant::now();
        let tasks = vec![
            Task { rm_priority: 1 },
            Task { rm_priority: 2 },
            Task { rm_priority: 3 },
            Task { rm_priority: 1 },
            Task { rm_priority: 2 },
            Task { rm_priority: 3 },
            Task { rm_priority: 1 },
            Task { rm_priority: 2 },
            Task { rm_priority: 3 },
            Task { rm_priority: 1 },
        ];

        b.iter(|| {
            let mut ready: Vec<Job> = (0..10).map(|i| Job {
                task_idx: i,
                deadline: now + Duration::from_millis(100),
                rm_priority: tasks[i % tasks.len()].rm_priority,
            }).collect();

            // Simulate the RM sorting logic
            ready.sort_by(|a, b| {
                let pa = a.rm_priority;
                let pb = b.rm_priority;
                match pa.cmp(&pb) {
                    Ordering::Equal => a.deadline.cmp(&b.deadline),
                    other => other,
                }
            });

            std::hint::black_box(ready);
        });
    });

    c.bench_function("ready_queue_sort_100_jobs", |b| {
        let now = TokioInstant::now();
        let tasks = (0..100).map(|i| Task { rm_priority: (i % 5) + 1 }).collect::<Vec<_>>();

        b.iter(|| {
            let mut ready: Vec<Job> = (0..100).map(|i| Job {
                task_idx: i,
                deadline: now + Duration::from_millis(100 + i as u64),
                rm_priority: tasks[i].rm_priority,
            }).collect();

            // Simulate the RM sorting logic
            ready.sort_by(|a, b| {
                let pa = a.rm_priority;
                let pb = b.rm_priority;
                match pa.cmp(&pb) {
                    Ordering::Equal => a.deadline.cmp(&b.deadline),
                    other => other,
                }
            });

            std::hint::black_box(ready);
        });
    });
}

// Benchmark a simple telemetry batching operation
fn bench_telemetry_batching(c: &mut Criterion) {
    c.bench_function("telemetry_batch_100_items", |b| {
        let data: Vec<f64> = (0..100).map(|i| i as f64 * 0.1).collect();

        b.iter(|| {
            let mut batch = Vec::with_capacity(100);
            for &item in &data {
                batch.push(item * 2.0); // simulate some processing
            }
            std::hint::black_box(batch);
        });
    });
}

criterion_group!(benches, bench_ready_queue_sort, bench_telemetry_batching);
criterion_main!(benches);