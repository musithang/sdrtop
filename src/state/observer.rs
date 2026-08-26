#[derive(Clone, Default)]
pub struct ObserverState {
    pub active: bool,
    pub device: Option<String>,
    pub serial: Option<String>,
    pub usb: Option<String>,
    pub connected: Option<String>,
    pub owner: Option<String>,
    pub cmdline: Option<String>,
    pub owner_cpu_pct: f32,
    pub owner_ram_mb: u64,
    pub owner_uptime: Option<String>,
}
