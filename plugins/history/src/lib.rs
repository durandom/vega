//! Consumer-free snapshot scan, using only the viewer's INFO and MSG.GET grants.
use serde_json::Value;
use vega_herdr::Failure;
pub const STREAM: &str = "VEGA_HISTORY";

#[derive(Default, Debug, clap::Args)]
pub struct Filter {
    #[arg(long)]
    pub project: Option<String>,
    #[arg(long)]
    pub bead: Option<String>,
    #[arg(long)]
    pub host: Option<String>,
    /// Inclusive event timestamp, Unix milliseconds (not arrival order).
    #[arg(long)]
    pub since: Option<u64>,
    /// Skip stream sequences through this cursor.
    #[arg(long, default_value_t = 0)]
    pub after: u64,
    /// Maximum returned matches (read default 100; export default unlimited).
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    pub limit: Option<u64>,
}
impl Filter {
    pub fn matches(&self, e: &Value) -> bool {
        [&self.project, &self.bead, &self.host]
            .iter()
            .zip(["project", "bead", "host"])
            .all(|(v, key)| v.as_deref().is_none_or(|v| e[key].as_str() == Some(v)))
            && self
                .since
                .is_none_or(|since| e["ts"].as_u64().is_some_and(|ts| ts >= since))
    }
}

pub fn failure(code: &str, message: impl ToString) -> Failure {
    Failure::new(code, message.to_string(), "use the viewer seed with vega history read/export --viewer-key <path>; check broker and docs/design/history.md")
}

/// No implicit host/admin seed fallback, including when the viewer seed is missing.
pub async fn connect(url: &str, key: &std::path::Path) -> Result<async_nats::Client, Failure> {
    let seed = std::fs::read_to_string(key)
        .map_err(|_| failure("viewer_key", "cannot read viewer seed file"))?;
    async_nats::ConnectOptions::with_nkey(seed.trim().to_owned())
        .custom_inbox_prefix("_INBOX_viewer")
        .connection_timeout(std::time::Duration::from_secs(5))
        .request_timeout(Some(std::time::Duration::from_secs(5)))
        .connect(url)
        .await
        .map_err(|_| failure("history_broker", "viewer connection failed"))
}

/// Streams matches to the caller without retaining event bodies. A fixed end
/// sequence prevents publishers from extending a running export indefinitely.
/// Errors are never treated as empty or successful partial results.
pub async fn scan(
    client: async_nats::Client,
    filter: &Filter,
    mut visit: impl FnMut(u64, Value) -> Result<(), Failure>,
) -> Result<(u64, u64), Failure> {
    let stream = async_nats::jetstream::new(client)
        .get_stream(STREAM)
        .await
        .map_err(|_| failure("history_read", "cannot read history stream info"))?;
    let state = &stream.cached_info().state;
    let end = state.last_sequence;
    if filter.after >= end {
        return Ok((0, end));
    }
    let mut count = 0;
    for seq in state.first_sequence.max(filter.after.saturating_add(1))..=end {
        let message = stream.get_raw_message(seq).await.map_err(|_| {
            failure(
                "history_read",
                format!("cannot read history sequence {seq}; export may be partial"),
            )
        })?;
        let event: Value = serde_json::from_slice(&message.payload)
            .map_err(|_| failure("history_schema", format!("invalid JSON at sequence {seq}")))?;
        if event["schema"] != 1
            || !event["id"].is_string()
            || !event["kind"].is_string()
            || !event["host"].is_string()
            || !event["ts"].is_u64()
        {
            return Err(failure(
                "history_schema",
                format!("unsupported event at sequence {seq}"),
            ));
        }
        if filter.matches(&event) {
            count += 1;
            visit(seq, event)?;
        }
    }
    Ok((count, end))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filters_are_conjunctive_and_since_inclusive() {
        let f = Filter {
            project: Some("p".into()),
            bead: Some("b".into()),
            host: Some("h".into()),
            since: Some(10),
            ..Default::default()
        };
        let mut e = serde_json::json!({"project":"p","bead":"b","host":"h","ts":10});
        assert!(f.matches(&e));
        for (key, value) in [
            ("project", serde_json::json!(null)),
            ("bead", serde_json::json!("other")),
            ("host", serde_json::json!("other")),
            ("ts", serde_json::json!(9)),
        ] {
            let old = e[key].take();
            e[key] = value;
            assert!(!f.matches(&e));
            e[key] = old;
        }
    }
}
