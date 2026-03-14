// logging/packets.rs (helper for packet transmission logs)

use tokio::{io::AsyncWriteExt, fs::OpenOptions, sync::Mutex};
use tokio::sync::OnceCell;
use chrono::Utc;

// Global writer used to log packet transmission data.
// OnceCell ensures the file is opened only once during runtime.
static PACKETS: OnceCell<Mutex<tokio::fs::File>> = OnceCell::const_new();

/// Log a packet transmission record.
/// packets.csv format:
/// ts,packet_type,seq,bytes
pub async fn log_packet(pkt_type: &str, seq: u32, size: usize) {

    // Initialize the log file on first use
    let log_file = PACKETS.get_or_init(|| async {
        let writer = OpenOptions::new()
            .create(true)
            .append(true)
            .open("logs/packets.csv")
            .await
            .unwrap();

        let m = Mutex::new(writer);

        // Write CSV header when the file is first created
        let mut writer = m.lock().await;
        let _ = writer.write_all(b"ts,packet_type,seq,bytes\n").await;

        m
    }).await;

    // Generate timestamp for this log entry
    let ts = Utc::now().to_rfc3339();

    // Build the CSV log line
    let csv_line = format!("{ts},{pkt_type},{seq},{size}\n");

    // Write the log line to the file
    let mut writer = log_file.lock().await;
    let _ = writer.write_all(csv_line.as_bytes()).await;
}