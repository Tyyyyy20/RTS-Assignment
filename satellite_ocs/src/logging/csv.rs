use std::sync::Arc;
use chrono::Utc;
use tokio::sync::{Mutex, OnceCell};
use tokio::{
    fs::{self, OpenOptions},
    io::{AsyncWriteExt, BufWriter},
};


// All logs use the same OnceCell type for simplicity/consistency.
static SENSORS: OnceCell<Arc<Mutex<BufWriter<tokio::fs::File>>>> = OnceCell::const_new();
static DROPS:   OnceCell<Arc<Mutex<BufWriter<tokio::fs::File>>>> = OnceCell::const_new();
static BATCHES: OnceCell<Arc<Mutex<BufWriter<tokio::fs::File>>>> = OnceCell::const_new();
static SCHED:   OnceCell<Arc<Mutex<BufWriter<tokio::fs::File>>>> = OnceCell::const_new();
static CPU:     OnceCell<Arc<Mutex<BufWriter<tokio::fs::File>>>> = OnceCell::const_new();
static DOWNLINK: OnceCell<Arc<Mutex<BufWriter<tokio::fs::File>>>> = OnceCell::const_new(); 
static FAULTS: OnceCell<Arc<Mutex<BufWriter<tokio::fs::File>>>> = OnceCell::const_new();

async fn ensure_dir() {
    let _ = fs::create_dir_all("logs").await;
}

async fn get_file(
    cell: &OnceCell<Arc<Mutex<BufWriter<tokio::fs::File>>>>,
    path: &str,
    header: &str,
) -> Arc<Mutex<BufWriter<tokio::fs::File>>> {
    let arc = cell.get_or_init(|| async move {
        ensure_dir().await;
        let fresh = !fs::try_exists(path).await.unwrap_or(false);
        let f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
            .expect("open log file");
        let writer = BufWriter::new(f);
        let m = Arc::new(Mutex::new(writer));
        if fresh {
            let mut g = m.lock().await;
            let _ = g.write_all(header.as_bytes()).await;
            let _ = g.flush().await;
        }
        m
    }).await;
    arc.clone()
} 

async fn get_faults_file() -> Arc<Mutex<BufWriter<tokio::fs::File>>> {
    super::csv::get_file(
        &FAULTS,
        "logs/faults.csv",
        "ts,event,fault_id,target,kind,duration_ms,component,recovery_ms,aborted,note\n",
    ).await
}

/// sensors.csv: ts,sensor,seq,jitter_ms,drift_ms,processing_latency_ms,priority,status
pub async fn log_sensor_reading(
    sensor: &str,
    seq: u64,
    jitter_ms: f64,
    drift_ms: f64,
    proc_ms: f64,
    priority: &str,
    status: &str,
) {
    let ts = Utc::now().to_rfc3339();
    let line = format!(
        "{ts},{sensor},{seq},{jitter_ms:.3},{drift_ms:.3},{proc_ms:.3},{priority},{status}\n"
    );
    let file = get_file(
        &SENSORS,
        "logs/sensors.csv",
        "ts,sensor,seq,jitter_ms,drift_ms,processing_latency_ms,priority,status\n",
    ).await;
    let mut f = file.lock().await;
    let _ = f.write_all(line.as_bytes()).await;
    let _ = f.flush().await;
}

/// drops.csv: ts,priority,dropped_count
pub async fn log_drop(priority: &str, dropped_count: usize) {
    let ts = Utc::now().to_rfc3339();
    let line = format!("{ts},{priority},{dropped_count}\n");
    let file = get_file(&DROPS, "logs/drops.csv", "ts,priority,dropped_count\n").await;
    let mut f = file.lock().await;
    let _ = f.write_all(line.as_bytes()).await;
    let _ = f.flush().await;
}

/// batches.csv: ts,total,critical,important,normal
pub async fn log_batch(total: usize, c: usize, i: usize, n: usize) {
    let ts = Utc::now().to_rfc3339();
    let line = format!("{ts},{total},{c},{i},{n}\n");
    let file = get_file(
        &BATCHES,
        "logs/batches.csv",
        "ts,total,critical,important,normal\n",
    ).await;
    let mut f = file.lock().await;
    let _ = f.write_all(line.as_bytes()).await;
    let _ = f.flush().await;
}

/// scheduler.csv: ts,task,seq,start_delay_ms,completion_delay_ms,runtime_ms,preemptions,deadline_ms
pub async fn log_sched_event(
    task: &str,
    seq: u64,
    start_delay_ms: f64,
    completion_delay_ms: f64,
    runtime_ms: f64,
    preemptions: u32,
    deadline_ms: f64,
) {
    let ts = Utc::now().to_rfc3339();
    let line = format!(
        "{ts},{task},{seq},{start_delay_ms:.3},{completion_delay_ms:.3},{runtime_ms:.3},{preemptions},{deadline_ms:.3}\n"
    );
    let file = get_file(
        &SCHED,
        "logs/scheduler.csv",
        "ts,task,seq,start_delay_ms,completion_delay_ms,runtime_ms,preemptions,deadline_ms\n",
    ).await;
    let mut f = file.lock().await;
    let _ = f.write_all(line.as_bytes()).await;
    let _ = f.flush().await;
}

/// cpu.csv: ts,window_ms,active_ms,idle_ms,active_pct
pub async fn log_cpu(window_ms: u64, active_ms: f64, idle_ms: f64) {
    let ts = Utc::now().to_rfc3339();
    let active_pct = if window_ms > 0 {
    (active_ms / window_ms as f64) * 100.0
} else {
    0.0
};

    let line = format!("{ts},{window_ms},{active_ms:.3},{idle_ms:.3},{active_pct:.2}\n");
    let file = get_file(
        &CPU,
        "logs/cpu.csv",
        "ts,window_ms,active_ms,idle_ms,active_pct\n",
    ).await;
    let mut f = file.lock().await;
    let _ = f.write_all(line.as_bytes()).await;
    let _ = f.flush().await;
} 

/// downlink.csv: ts,event,info
pub async fn log_downlink(
    batch_size: usize,
    avg_queue_ms: f64,
    max_queue_ms: f64,
    fill_pct: f64,
    event: &str,
) {
    let ts = Utc::now().to_rfc3339();
    let line = format!("{ts},{batch_size},{avg_queue_ms:.3},{max_queue_ms:.3},{fill_pct:.1},{event}\n");
    let file = get_file(
        &DOWNLINK,
        "logs/downlink.csv",
        "ts,batch_size,avg_queue_ms,max_queue_ms,fill_pct,event\n",
    ).await;
    let mut f = file.lock().await;
    let _ = f.write_all(line.as_bytes()).await;
    let _ = f.flush().await;
} 

/// faults.csv (injection): ts=now, event="inject"
pub async fn log_fault_inject(fault_id: &str, target: &str, kind: &str, duration_ms: u64) {
    let ts = Utc::now().to_rfc3339();
    let line = format!("{ts},inject,{fault_id},{target},{kind},{duration_ms},,,,\n");
    let f = get_faults_file().await;
    let mut g = f.lock().await;
    let _ = g.write_all(line.as_bytes()).await;
    let _ = g.flush().await;
}

/// faults.csv (recovery): ts=now, event="recovery"
pub async fn log_fault_recovery(
    fault_id: &str,
    component: &str,
    recovery_ms: f64,
    aborted: bool,
) {
    let ts = Utc::now().to_rfc3339();
    let line = format!(
        "{ts},recovery,{fault_id},,,,{component},{recovery_ms:.1},{aborted},\n"
    );
    let f = get_faults_file().await;
    let mut g = f.lock().await;
    let _ = g.write_all(line.as_bytes()).await;
    let _ = g.flush().await;
} 

// txqueue.csv: ts,oldest_ms,fill_pct
pub async fn log_tx_queue(oldest_ms: f64, fill_pct: f64) {
    use tokio::sync::OnceCell;
    use tokio::{fs::{self, OpenOptions}, io::AsyncWriteExt, sync::Mutex};
    use chrono::Utc;

    static TXQ: OnceCell<Mutex<tokio::fs::File>> = OnceCell::const_new();

    async fn file() -> &'static Mutex<tokio::fs::File> {
        TXQ.get_or_init(|| async {
            let _ = fs::create_dir_all("logs").await;
            let fresh = !fs::try_exists("logs/txqueue.csv").await.unwrap_or(false);
            let f = OpenOptions::new().create(true).append(true).open("logs/txqueue.csv").await.unwrap();
            let m = Mutex::new(f);
            if fresh {
                let mut g = m.lock().await;
                let _ = g.write_all(b"ts,oldest_ms,fill_pct\n").await;
            }
            m
        }).await
    }

    let ts = Utc::now().to_rfc3339();
    let line = format!("{ts},{oldest_ms:.3},{fill_pct:.1}\n");
    let mut f = file().await.lock().await;
    let _ = f.write_all(line.as_bytes()).await;
}

 
