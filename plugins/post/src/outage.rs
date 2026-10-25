//! A broker that goes away while vegad runs (docs/design/vegad.md, Broker
//! outage, vega-lhc.205): every broker call in the loop is bounded, the loop
//! leaves the broker alone while the client is disconnected, and after
//! LOST_AFTER vegad switches to local mode until a probe, with backoff,
//! reaches the broker again. The human gets one note per outage.

use std::future::Future;
use std::time::Duration;

use vega_herdr::notes::{Note, Store};
use vega_herdr::Failure;

/// The longest one broker call in vegad's loop may take. async-nats bounds a
/// request's answer, not its queueing: while it reconnects it stops reading
/// its command channel (2048 slots), and once that is full a send waits
/// forever. That hung vegad for hours on 2026-10-25.
pub const CALL: Duration = Duration::from_secs(10);

/// How long the client may stay disconnected before vegad gives up on it and
/// switches to local mode; a broker restart is usually back sooner.
pub const LOST_AFTER: Duration = Duration::from_secs(10);

/// The note kind and cause of an outage; the cause clears it.
pub const KIND: &str = "broker";
const CAUSE: &str = "broker:unreachable";

/// `fut`, or `broker_unreachable` when it takes longer than CALL.
pub async fn bounded<T>(
    what: &str,
    fut: impl Future<Output = Result<T, Failure>>,
) -> Result<T, Failure> {
    within(CALL, what, fut).await
}

pub async fn within<T>(
    limit: Duration,
    what: &str,
    fut: impl Future<Output = Result<T, Failure>>,
) -> Result<T, Failure> {
    tokio::time::timeout(limit, fut).await.unwrap_or_else(|_| {
        Err(Failure::new(
            "broker_unreachable",
            format!("{what}: no answer within {} ms", limit.as_millis()),
            "check the broker: `vega post relay status`",
        ))
    })
}

/// Probe delays in local mode: 2 s, doubling, at most `max`.
pub struct Backoff {
    next: Duration,
    max: Duration,
}

impl Backoff {
    pub const FIRST: Duration = Duration::from_secs(2);

    pub fn new(max: Duration) -> Self {
        Backoff {
            next: Self::FIRST.min(max),
            max,
        }
    }

    /// The delay before the next probe.
    pub fn next(&mut self) -> Duration {
        let d = self.next;
        self.next = (self.next * 2).min(self.max);
        d
    }
}

fn open(notes: &[Note]) -> bool {
    notes
        .iter()
        .any(|n| n.cause.as_deref() == Some(CAUSE) && n.resolved.is_none())
}

/// Tells the human the broker is away: a herdr toast plus a host note (the
/// vega UAC's badge), once per outage, also across a vegad restart.
pub fn noted(url: &str) {
    if Store::local().is_some_and(|s| open(&s.list())) {
        return;
    }
    vega_herdr::notes::notify_project(
        KIND,
        None,
        None,
        Some(CAUSE),
        &format!("vegad: broker {url} unreachable"),
        "Mail, registry and commands are off. The sidebar, badges, attention and \
         wakes keep running here; vegad reconnects by itself and clears this note.",
    );
}

/// The broker is back: clears the outage note. Returns whether one was open.
pub fn cleared(now_s: i64) -> bool {
    Store::local().is_some_and(|s| s.resolve(now_s, |n| n.cause.as_deref() == Some(CAUSE)) > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_up_to_its_cap() {
        let mut b = Backoff::new(Duration::from_secs(60));
        let got: Vec<u64> = (0..7).map(|_| b.next().as_secs()).collect();
        assert_eq!(got, [2, 4, 8, 16, 32, 60, 60]);
    }

    #[tokio::test]
    async fn a_call_that_never_answers_fails_after_the_limit() {
        let limit = Duration::from_millis(20);
        let r: Result<(), Failure> = within(limit, "count", std::future::pending()).await;
        let f = r.unwrap_err();
        assert_eq!(f.code, "broker_unreachable");
        assert!(
            f.message.starts_with("count: no answer within"),
            "{}",
            f.message
        );
    }

    #[test]
    fn only_an_unresolved_outage_note_counts_as_open() {
        let n = |cause: &str, resolved| Note {
            cause: Some(cause.into()),
            resolved,
            ..Default::default()
        };
        assert!(open(&[n(CAUSE, None)]));
        assert!(!open(&[n(CAUSE, Some(1))]));
        assert!(!open(&[n("bd-sync:X", None)]));
    }
}
