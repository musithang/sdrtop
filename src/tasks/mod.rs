mod observer;
mod rx;
mod sweep;
mod system;

pub use observer::spawn_observer_task;
pub use rx::spawn_rx_task;
pub use sweep::spawn_sweep_task;
pub use system::spawn_sys_resource_task;

pub fn fmt_duration(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{}h {}m {}s", h, m, s)
    } else if m > 0 {
        format!("{}m {}s", m, s)
    } else {
        format!("{}s", s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_duration_formats_correctly() {
        assert_eq!(fmt_duration(0), "0s");
        assert_eq!(fmt_duration(45), "45s");
        assert_eq!(fmt_duration(90), "1m 30s");
        assert_eq!(fmt_duration(3661), "1h 1m 1s");
    }
}
