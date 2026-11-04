//! Budget evidence is independent of log suppression. No broker or subprocess calls.
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use vega_herdr::daemon_health::Health;

#[derive(Default)]
struct Monitor {
    health: Health,
    written: Option<Instant>,
    log_at: Option<Instant>,
    suppressed: u64,
}

const LOG_EVERY: Duration = Duration::from_secs(600);
static MONITOR: OnceLock<Mutex<Monitor>> = OnceLock::new();

fn monitor() -> &'static Mutex<Monitor> {
    MONITOR.get_or_init(Default::default)
}

pub fn observe(step: &str, took: Duration, budget: Duration) {
    let Ok(mut m) = monitor().lock() else { return };
    let path = vega_herdr::daemon_health::path();
    m.observe(
        step,
        took,
        budget,
        Instant::now(),
        (crate::now_ms() / 1000) as i64,
        path.as_deref(),
    );
}

impl Monitor {
    fn observe(
        &mut self,
        step: &str,
        took: Duration,
        budget: Duration,
        now: Instant,
        at: i64,
        path: Option<&std::path::Path>,
    ) {
        self.health
            .observe(step, took.as_millis() as u64, budget.as_millis() as u64);
        // Only the completed loop stamps evidence. A busy worker cannot make a stuck loop look healthy.
        if matches!(step, "loop" | "local loop")
            && self
                .written
                .is_none_or(|last| now.duration_since(last) >= Duration::from_secs(15))
        {
            self.health.at = at;
            if let Some(path) = path {
                if self.health.write(path).is_ok() {
                    self.written = Some(now);
                }
            }
        }
    }

    fn slow_log(&mut self, line: &str, now: Instant) -> Option<String> {
        if self
            .log_at
            .is_some_and(|at| now.duration_since(at) < LOG_EVERY)
        {
            self.suppressed = self.suppressed.saturating_add(1);
            return None;
        }
        let skipped = std::mem::take(&mut self.suppressed);
        self.log_at = Some(now);
        Some(format!("{line} (slow logs suppressed since last summary: {skipped}; next summary in 10 min; see vega uac)"))
    }
}

pub fn slow_log(line: &str) -> Option<String> {
    monitor().lock().ok()?.slow_log(line, Instant::now())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_completed_loops_publish_and_workers_cannot_hide_a_stall() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("health.json");
        let mut m = Monitor::default();
        let start = Instant::now();
        let budget = Duration::from_secs(5);
        let slow = Duration::from_secs(6);
        let read = || serde_json::from_slice::<Health>(&std::fs::read(&path).unwrap()).unwrap();
        m.observe("sidebar", slow, budget, start, 100, Some(&path));
        assert!(!path.exists());
        m.observe("loop", slow, budget, start, 100, Some(&path));
        assert_eq!(read().at, 100);
        m.observe(
            "loop",
            slow,
            budget,
            start + Duration::from_secs(6),
            106,
            Some(&path),
        );
        assert_eq!(read().at, 100);
        m.observe(
            "loop",
            slow,
            budget,
            start + Duration::from_secs(16),
            116,
            Some(&path),
        );
        assert!(read().samples["loop"].active);
        m.observe(
            "sidebar",
            slow,
            budget,
            start + Duration::from_secs(200),
            300,
            Some(&path),
        );
        assert_eq!(read().at, 116);
        assert!(vega_herdr::daemon_health::render(Some(&read()), 300).contains("unknown"));
        m.observe(
            "loop",
            Duration::from_secs(1),
            budget,
            start + Duration::from_secs(201),
            301,
            Some(&path),
        );
        assert!(!read().samples["loop"].active);
    }

    #[test]
    fn slow_logs_are_summarized_but_health_keeps_every_observation() {
        let mut m = Monitor::default();
        let now = Instant::now();
        assert!(m.slow_log("slow: registry", now).is_some());
        for i in 1..600 {
            m.health.observe("registry", 6000, 5000);
            assert!(m
                .slow_log("slow: loop", now + Duration::from_secs(i))
                .is_none());
        }
        let summary = m.slow_log("slow: metadata", now + LOG_EVERY).unwrap();
        assert!(summary.contains("599"));
        assert!(m.health.samples["registry"].active);
        assert_eq!(m.health.samples["registry"].consecutive, 599);
    }
}
