use serde::Serialize;

#[derive(Debug, Default, Serialize)]
pub struct Snapshot {
    pub ts: String,
    pub role: String,
    pub pid: u32,
    pub packets_tx: u64,
    pub packets_rx: u64,
    pub rtp_loss: u64,
    pub jitter_ms_p50: f32,
    pub jitter_ms_p95: f32,
    pub cpu_pct: f32,
}

impl Snapshot {
    pub fn json_line(&self) -> String { serde_json::to_string(self).unwrap_or_default() }
}

