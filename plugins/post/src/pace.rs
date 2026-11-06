//! vegad's loop cadences, sped up for its process tests.
//!
//! `VEGA_POST_TEST_SPEEDUP=<n>` divides the intervals vegad waits on between
//! loop steps (broker loss and probe backoff, registry and metadata refresh,
//! sidebar renewal, bounce and identity retries) by `n`, so a test of an
//! outage or a cutover runs in seconds, not minutes. Unset, invalid or 0
//! means 1: production timing. Timeouts of single calls are not scaled.

use std::sync::OnceLock;
use std::time::Duration;

const VAR: &str = "VEGA_POST_TEST_SPEEDUP";

fn speedup() -> u32 {
    static N: OnceLock<u32> = OnceLock::new();
    *N.get_or_init(|| {
        std::env::var(VAR)
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|&n| n > 0)
            .unwrap_or(1)
    })
}

/// `d` at the configured pace.
pub fn of(d: Duration) -> Duration {
    d / speedup()
}
