//! Effective reset inventory: manual provider entries override source accounts.
use serde::Serialize;
use vega_herdr::capacity::{self, Sample};
use vega_herdr::resets::Ledger;

#[derive(Serialize)]
pub struct Inventory {
    provider: String,
    account_id: Option<String>,
    observed_at: Option<i64>,
    owned: u32,
    source: &'static str,
    next_expiry: Option<i64>,
    last_used: Option<i64>,
}

pub fn rows(sample: Option<&Sample>, ledger: &Ledger, now: i64) -> Vec<Inventory> {
    let mut rows: Vec<_> = ledger
        .providers
        .iter()
        .map(|(provider, entry)| Inventory {
            provider: provider.clone(),
            account_id: None,
            owned: entry.owned,
            source: "manual",
            observed_at: None,
            next_expiry: None,
            last_used: entry.used.last().copied(),
        })
        .collect();
    if let Some(sample) = sample {
        for h in capacity::headroom(sample, now) {
            if ledger.providers.contains_key(&h.provider) {
                continue;
            }
            if let Some(r) = h.resets {
                rows.push(Inventory {
                    provider: h.provider,
                    account_id: Some(h.account_id),
                    owned: r.owned,
                    source: "token-burn",
                    observed_at: Some(r.observed_at),
                    next_expiry: r.next_expiry,
                    last_used: r.last_used,
                });
            }
        }
    }
    rows
}

pub fn table(sample: Option<&Sample>, ledger: &Ledger, now: i64) -> Vec<String> {
    let rows = rows(sample, ledger, now);
    // Keep the manual plan/override table and add source inventory only when available.
    let live: Vec<_> = rows.iter().filter(|r| r.source == "token-burn").collect();
    let mut out = super::ledger::table(ledger, now);
    if live.is_empty() {
        return out;
    }
    if ledger.providers.is_empty() {
        out.clear();
    }
    out.push(format!(
        "source_resets[{}]{{provider,account_id,owned,next_expiry,last_used}}:",
        live.len()
    ));
    for r in live {
        let expiry = r
            .next_expiry
            .map(|t| {
                let date = vega_herdr::resets::day_of(t);
                if t <= now {
                    format!("{date} (passed; refresh inventory)")
                } else if t <= now + 7 * 86400 {
                    format!("{date} (expires this week)")
                } else {
                    date
                }
            })
            .unwrap_or_else(|| "unknown".into());
        out.push(format!(
            "  {},{},{},{},{}",
            vega_herdr::cell(&r.provider),
            vega_herdr::cell(r.account_id.as_deref().unwrap_or_default()),
            r.owned,
            vega_herdr::cell(&expiry),
            r.last_used
                .map(vega_herdr::resets::day_of)
                .unwrap_or_default()
        ));
    }
    out
}
