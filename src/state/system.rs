use std::collections::VecDeque;
use std::sync::Arc;

#[derive(Clone)]
pub struct SystemState {
    pub board_name: Arc<str>,
    #[allow(dead_code)]
    pub serial: Arc<str>,
    pub fw_version: Arc<str>,
    // Device identity captured at launch and logged once; kept on the struct for
    // completeness like `serial`, though no panel currently renders them.
    #[allow(dead_code)]
    pub board_rev: u8,
    #[allow(dead_code)]
    pub usb_api_version: u16,
    pub process_cpu_pct: f32,
    pub process_rss_mb: u64,
    /// CPU % × 10 per sample (0.1 % resolution), one entry per system task poll (1 s).
    pub cpu_history: VecDeque<u64>,
}
