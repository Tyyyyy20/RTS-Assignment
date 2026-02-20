// logging/packets.rs (new helper)
use tokio::{io::AsyncWriteExt, fs::OpenOptions, sync::Mutex};
use tokio::sync::OnceCell;
use chrono::Utc;

static PACKETS: OnceCell<Mutex<tokio::fs::File>> = OnceCell::const_new();

pub async fn log_packet(pkt_type: &str, seq: u32, size: usize) {
    let file = PACKETS.get_or_init(|| async {
        let f = OpenOptions::new().create(true).append(true).open("logs/packets.csv").await.unwrap();
        let m = Mutex::new(f);
        let mut g = m.lock().await;
        let _ = g.write_all(b"ts,packet_type,seq,bytes\n").await;
        m
    }).await;

    let ts = Utc::now().to_rfc3339();
    let line = format!("{ts},{pkt_type},{seq},{size}\n");
    let mut f = file.lock().await;
    let _ = f.write_all(line.as_bytes()).await;
}
