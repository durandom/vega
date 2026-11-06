//! Owner-scoped deletion and bounded migration of old KV tombstones.

use std::collections::VecDeque;
use std::error::Error;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use async_nats::jetstream::{kv, ErrorCode};

const MARKER_TTL: Duration = Duration::from_secs(3600);
const CLEANUP_BATCH: usize = 32;
const CLEANUP_EVERY: Duration = Duration::from_secs(15);
static WARNED: AtomicBool = AtomicBool::new(false);

fn ttl_disabled(error: &kv::PurgeError) -> bool {
    let mut source = error.source();
    while let Some(e) = source {
        if e.downcast_ref::<async_nats::jetstream::Error>()
            .is_some_and(|e| e.error_code() == ErrorCode::MESSAGE_TTL_DISABLED)
        {
            return true;
        }
        source = e.source();
    }
    false
}

async fn purge(kv: &kv::Store, key: &str, revision: u64) -> Result<(), String> {
    let result = kv
        .purge_expect_revision_with_ttl(key, revision, MARKER_TTL)
        .await;
    match result {
        Ok(()) => Ok(()),
        // A CLI may have put a live value since the inspection. Leave it alone.
        Err(e) if e.kind() == kv::PurgeErrorKind::WrongLastRevision => Ok(()),
        Err(e) if ttl_disabled(&e) => {
            if !WARNED.swap(true, Ordering::Relaxed) {
                eprintln!("registry: message TTL disabled; falling back to plain delete (enable allow_msg_ttl on KV_VEGA_REGISTRY)");
            }
            match kv.delete_expect_revision(key, Some(revision)).await {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == kv::DeleteErrorKind::WrongLastRevision => Ok(()),
                Err(e) => Err(e.to_string()),
            }
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Remove a live registry value using a one-hour, subject-rollup PURGE marker.
/// Existing tombstones and absent keys are no-ops, so reconcile never renews
/// their lifetime. The broker enforces the caller's own-host write scope.
pub async fn remove(kv: &kv::Store, key: &str) -> Result<(), String> {
    if let Some(entry) = kv.entry(key).await.map_err(|e| e.to_string())? {
        if entry.operation == kv::Operation::Put {
            purge(kv, key, entry.revision).await?;
        }
    }
    Ok(())
}

/// One startup snapshot, at most 32 own subjects inspected per 15 seconds.
/// Retained across reconnects; actual entries distinguish skipped note/janitor
/// values from tombstones. Conditional purge preserves concurrent new values.
#[derive(Default)]
pub struct DeadSubjects {
    pending: Option<VecDeque<String>>,
    next: Option<Instant>,
    /// The pace between batches; None is CLEANUP_EVERY.
    every: Option<Duration>,
}

impl DeadSubjects {
    /// A cleanup with batches `every` apart instead of 15 s, for tests.
    pub fn paced(every: Duration) -> Self {
        DeadSubjects {
            every: Some(every),
            ..Default::default()
        }
    }

    pub async fn run(
        &mut self,
        kv: &kv::Store,
        host: &str,
        subjects: &[String],
    ) -> Result<(), String> {
        let prefix = format!("{}.", crate::token(host));
        let pending = self.pending.get_or_insert_with(|| {
            subjects
                .iter()
                .filter(|k| k.starts_with(&prefix))
                .cloned()
                .collect()
        });
        if self.next.is_some_and(|next| Instant::now() < next) {
            return Ok(());
        }
        self.next = Some(Instant::now() + self.every.unwrap_or(CLEANUP_EVERY));
        for _ in 0..CLEANUP_BATCH {
            let Some(key) = pending.front() else { break };
            if let Some(entry) = kv.entry(key.clone()).await.map_err(|e| e.to_string())? {
                if entry.operation != kv::Operation::Put {
                    purge(kv, key, entry.revision).await?;
                }
            }
            // Pop only after completion: cancellation/reconnect retries this key.
            pending.pop_front();
        }
        Ok(())
    }
}
