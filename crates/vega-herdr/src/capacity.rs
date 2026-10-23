//! Subscription headroom, for `vega capacity` (every window), `vega uac
//! status` and `vega mux spawn` (the binding window per provider).
//!
//! Source-neutral: a `Source` adapter maps a quota monitor's stored sample
//! into `Reading`s (provider, window, scope, percent left, reset, forecast
//! run-out, sample time). token-burn (the Slayer's quota monitor) is the one
//! adapter today (`token_burn`). Window scopes are data
//! (`config/capacity/windows.toml`, `scopes`): a window not listed is
//! account-wide. Per provider the account window with the least left binds;
//! any provider the source reports shows and binds, without code here.
//!
//! No provider call per vega call. Advisory only: vega never picks the kind.
//! A failing source makes the capacity unknown.
//!
//! The Slayer's own quota resets (`resets`) cover a thin window: a planned
//! reset or an override says so instead of warning.

pub mod scopes;
pub mod token_burn;

use std::time::{SystemTime, UNIX_EPOCH};

use crate::resets::Ledger;
use scopes::Scopes;
use token_burn::TokenBurn;

/// A spawn warns below this many percent left in the binding window.
pub const THIN_PERCENT: f64 = 10.0;

/// A spawn warns when the forecast runs the window out within this.
pub const SOON_SECS: i64 = 2 * 3600;

/// The daemon polls every five minutes: an older sample is stale.
pub const STALE_SECS: i64 = 30 * 60;

/// What a window limits.
#[derive(Debug, Clone, PartialEq)]
pub enum Scope {
    /// The whole account: it binds.
    Account,
    /// One model only: it shows, never binds.
    Model(String),
}

impl Scope {
    /// `account` or `model:<name>`.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.strip_prefix("model:") {
            _ if s == "account" => Ok(Scope::Account),
            Some("") => Err("model: needs a name".into()),
            Some(m) => Ok(Scope::Model(m.into())),
            None => Err(format!("{s} is neither account nor model:<name>")),
        }
    }

    pub fn binds(&self) -> bool {
        *self == Scope::Account
    }

    pub fn text(&self) -> String {
        match self {
            Scope::Account => "account".into(),
            Scope::Model(m) => format!("model:{m}"),
        }
    }
}

/// One window as a source reports it, in vega's terms (Unix seconds).
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    pub provider: String,
    pub window: String,
    pub scope: Scope,
    /// Percent left; 0 when the source says the limit is reached.
    pub left: f64,
    pub reset_at: Option<i64>,
    /// When a confident forecast runs the window out.
    pub runs_out_at: Option<i64>,
    /// When the source sampled the window.
    pub observed_at: Option<i64>,
}

/// A source's stored sample: every window, in the source's order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sample {
    pub readings: Vec<Reading>,
}

/// A quota monitor vega reads. `Err` is why the capacity is unknown.
pub trait Source {
    fn read(&self) -> Result<Sample, String>;
}

/// Reads token-burn (`VEGA_TOKEN_BURN`, `VEGA_CAPACITY_FIXTURE`) and sets
/// the scopes from `capacity/windows.toml`.
pub fn read() -> Result<Sample, String> {
    read_from(&TokenBurn::from_env(), &Scopes::load()?)
}

pub fn read_from(source: &dyn Source, scopes: &Scopes) -> Result<Sample, String> {
    let mut sample = source.read()?;
    scopes.apply(&mut sample);
    Ok(sample)
}

/// The providers in the sample, in sample order.
pub fn providers(sample: &Sample) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::new();
    for r in &sample.readings {
        if !out.contains(&r.provider.as_str()) {
            out.push(&r.provider);
        }
    }
    out
}

/// The source's provider for a spawn: `claude`, `codex`, `agy`, and `pi`
/// by its `--provider`, else by the prefix of a `--model provider/model`.
/// A bare `--model` is not guessed. `None`: no provider vega maps the kind to.
pub fn provider_for(kind: &str, args: &[String]) -> Option<&'static str> {
    match kind {
        "claude" => Some("claude"),
        "codex" => Some("codex"),
        "agy" => Some("antigravity"),
        "pi" => {
            let mut it = args.iter();
            let (mut p, mut model) = (None, None);
            while let Some(a) = it.next() {
                if a == "--provider" {
                    p = it.next().map(String::as_str);
                } else if let Some(v) = a.strip_prefix("--provider=") {
                    p = Some(v);
                } else if a == "--model" {
                    model = it.next().map(String::as_str);
                } else if let Some(v) = a.strip_prefix("--model=") {
                    model = Some(v);
                }
            }
            let p = p.or_else(|| model?.split_once('/').map(|(p, _)| p));
            match p? {
                "xai" => Some("xai"),
                "zai" => Some("zai"),
                _ => None,
            }
        }
        _ => None,
    }
}

/// One window's headroom at a time.
#[derive(Debug, Clone, PartialEq)]
pub struct Headroom {
    pub provider: String,
    pub window: String,
    pub scope: Scope,
    /// Percent left; 0 when the source says the limit is reached.
    pub left: f64,
    /// Seconds until the window resets; `None` when unknown.
    pub resets_in: Option<i64>,
    /// Seconds until the forecast runs the window out before its reset
    /// (confident forecasts only).
    pub out_in: Option<i64>,
    /// Age of the sample in seconds.
    pub age: i64,
    /// The Slayer's planned reset or override (`resets::Cover::text`).
    pub cover: Option<String>,
}

impl Headroom {
    /// A reading at `now`: a window whose reset passed since the sample
    /// counts as full; a run-out that is past or after the reset is moot.
    pub fn at(r: &Reading, now: i64) -> Self {
        let passed = r.reset_at.is_some_and(|t| t <= now);
        let out_in = r
            .runs_out_at
            .filter(|&t| t > now && !passed && r.reset_at.is_none_or(|x| t < x))
            .map(|t| t - now);
        Headroom {
            provider: r.provider.clone(),
            window: r.window.clone(),
            scope: r.scope.clone(),
            left: if passed { 100.0 } else { r.left },
            resets_in: r.reset_at.filter(|_| !passed).map(|t| t - now),
            out_in,
            age: r.observed_at.map_or(0, |t| (now - t).max(0)),
            cover: None,
        }
    }

    /// Near empty, or forecast to run out within `SOON_SECS`.
    pub fn thin(&self) -> bool {
        self.left < THIN_PERCENT || self.out_in.is_some_and(|s| s <= SOON_SECS)
    }

    /// Thin and covered by neither a planned reset nor an override.
    pub fn warns(&self) -> bool {
        self.thin() && self.cover.is_none()
    }

    /// Sets the cover from the Slayer's ledger.
    pub fn covered_by(mut self, ledger: &Ledger, now: i64) -> Self {
        self.cover = ledger.cover(&self.provider, now).map(|c| c.text(now));
        self
    }

    pub fn stale(&self) -> bool {
        self.age > STALE_SECS
    }

    /// `claude 6% left (five_hour, resets in 2h; out in ~1h; reset planned
    /// this week)`.
    pub fn summary(&self) -> String {
        let mut s = format!(
            "{} {}% left ({}",
            self.provider,
            percent(self.left),
            self.window
        );
        if let Some(r) = self.resets_in {
            s += &format!(", resets in {}", duration(r));
        }
        if let Some(o) = self.out_in {
            s += &format!("; out in ~{}", duration(o));
        }
        if let Some(c) = &self.cover {
            s += &format!("; {c}");
        }
        if self.stale() {
            s += &format!("; sample {} old", duration(self.age));
        }
        s + ")"
    }
}

/// The binding window of every provider in the sample, in sample order.
/// A provider with model-scoped windows only has none.
pub fn headroom(sample: &Sample, now: i64) -> Vec<Headroom> {
    providers(sample)
        .into_iter()
        .filter_map(|p| for_provider(sample, p, now))
        .collect()
}

/// The binding window of `provider`: its account window with the least
/// left; on a tie the one that runs out first, then the first in the
/// sample. `None` when the sample has no account window of it.
pub fn for_provider(sample: &Sample, provider: &str, now: i64) -> Option<Headroom> {
    sample
        .readings
        .iter()
        .filter(|r| r.provider == provider && r.scope.binds())
        .map(|r| Headroom::at(r, now))
        .enumerate()
        .min_by_key(|(i, h)| ((h.left * 1000.0) as i64, h.out_in.unwrap_or(i64::MAX), *i))
        .map(|(_, h)| h)
}

/// One window of the sample; `binding` marks a provider's binding one.
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub headroom: Headroom,
    pub binding: bool,
}

/// Every window in the sample, grouped by provider in sample order.
pub fn windows(sample: &Sample, now: i64) -> Vec<Window> {
    let mut out = Vec::new();
    for p in providers(sample) {
        let binding = for_provider(sample, p, now).map(|h| h.window);
        for r in sample.readings.iter().filter(|r| r.provider == p) {
            let headroom = Headroom::at(r, now);
            out.push(Window {
                binding: binding.as_deref() == Some(headroom.window.as_str()),
                headroom,
            });
        }
    }
    out
}

/// The `vega uac status` lines: a table of every provider's binding
/// windows, or `capacity: unknown (<reason>)`, then the Slayer's resets
/// that need a look (an unreadable ledger, a plan whose day passed).
pub fn status_lines(
    sample: Result<Sample, String>,
    ledger: &Result<Ledger, String>,
    now: i64,
) -> Vec<String> {
    let mut out = table(sample, ledger.as_ref().ok(), now);
    out.extend(ledger_lines(ledger, now));
    out
}

fn table(sample: Result<Sample, String>, ledger: Option<&Ledger>, now: i64) -> Vec<String> {
    let sample = match sample {
        Ok(s) => s,
        Err(why) => return vec![format!("capacity: unknown ({why})")],
    };
    let empty = Ledger::default();
    let ledger = ledger.unwrap_or(&empty);
    let rows: Vec<Headroom> = headroom(&sample, now)
        .into_iter()
        .map(|h| h.covered_by(ledger, now))
        .collect();
    if rows.is_empty() {
        return vec!["capacity: unknown (no account window in the sample)".into()];
    }
    let mut out = vec![format!(
        "capacity[{}]{{provider,window,left,resets_in,runs_out,resets_owned}}:",
        rows.len()
    )];
    for h in &rows {
        let mut runs_out: Vec<String> = Vec::new();
        match h.out_in {
            Some(s) if h.warns() => runs_out.push(format!("in ~{} (soon)", duration(s))),
            Some(s) => runs_out.push(format!("in ~{}", duration(s))),
            None => {}
        }
        runs_out.extend(h.cover.clone());
        let owned = ledger
            .providers
            .get(&h.provider)
            .map(|e| e.owned.to_string())
            .unwrap_or_default();
        out.push(format!(
            "  {},{},{}%,{},{},{}",
            h.provider,
            h.window,
            percent(h.left),
            h.resets_in.map(duration).unwrap_or_default(),
            runs_out.join("; "),
            owned
        ));
    }
    let oldest = rows.iter().map(|h| h.age).max().unwrap_or(0);
    if oldest > STALE_SECS {
        out.push(format!(
            "capacity_sample: {} old (source down? vega capacity shows its health)",
            duration(oldest)
        ));
    }
    out
}

/// `capacity_resets:` lines for what in the ledger needs the Slayer.
fn ledger_lines(ledger: &Result<Ledger, String>, now: i64) -> Vec<String> {
    let ledger = match ledger {
        Ok(l) => l,
        Err(why) => {
            return vec![format!(
                "capacity_resets: unreadable ({why}); fix or remove it, then vega capacity"
            )]
        }
    };
    ledger
        .providers
        .iter()
        .filter_map(|(p, e)| {
            e.plan_passed(now).map(|d| {
                format!(
                    "capacity_resets: {p} reset planned by {d}, not recorded as used (vega capacity used {p}, or plan {p} --at <date>)"
                )
            })
        })
        .collect()
}

/// What `vega mux spawn` says: the `capacity:` field, and a stderr warning
/// when the binding window is thin.
pub fn spawn_check(
    kind: &str,
    args: &[String],
    sample: Result<Sample, String>,
    ledger: &Ledger,
    now: i64,
) -> (String, Option<String>) {
    let Some(provider) = provider_for(kind, args) else {
        let why = if kind == "pi" {
            "pi without --provider or --model of xai|zai".to_string()
        } else {
            format!("vega maps kind {kind} to no provider")
        };
        return (format!("capacity: unknown ({why})"), None);
    };
    let sample = match sample {
        Ok(s) => s,
        Err(why) => return (format!("capacity: unknown ({why})"), None),
    };
    let Some(h) = for_provider(&sample, provider, now).map(|h| h.covered_by(ledger, now)) else {
        return (
            format!("capacity: unknown (no {provider} account window in the sample)"),
            None,
        );
    };
    let field = format!("capacity: {}", h.summary());
    let owned = ledger.providers.get(provider).map_or(0, |e| e.owned);
    let warning = h.warns().then(|| {
        let reset = if owned > 0 {
            format!("; {owned} {provider} reset(s) owned: vega capacity plan {provider} --this-week")
        } else {
            String::new()
        };
        format!(
            "warning: {} binding window is thin: {}. vega does not pick the kind; spawn another --kind or provider if the run outlasts it (vega capacity lists every window){reset}",
            h.provider,
            h.summary()
        )
    });
    (field, warning)
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `97` or `99.9`: one decimal only below 100 when it matters.
pub fn percent(p: f64) -> String {
    let r = (p * 10.0).round() / 10.0;
    if r.fract() == 0.0 || r >= 99.95 {
        format!("{}", r.round() as i64)
    } else {
        format!("{r:.1}")
    }
}

/// `35m`, `4h12m`, `5d15h`.
pub fn duration(secs: i64) -> String {
    let m = secs.max(0) / 60;
    let (d, h, m) = (m / 1440, m / 60 % 24, m % 60);
    match (d, h) {
        (0, 0) => format!("{m}m"),
        (0, _) if m == 0 => format!("{h}h"),
        (0, _) => format!("{h}h{m}m"),
        _ if h == 0 => format!("{d}d"),
        _ => format!("{d}d{h}h"),
    }
}

/// `YYYY-MM-DDTHH:MM:SS[.frac](Z|±HH:MM)` as Unix seconds.
pub(crate) fn parse_rfc3339(s: &str) -> Option<i64> {
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hh, mm, ss) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let rest = s
        .get(19..)?
        .trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let offset = match rest {
        "Z" | "" => 0,
        o => {
            let sign = if o.starts_with('-') { -1 } else { 1 };
            let h = o.get(1..3)?.parse::<i64>().ok()?;
            let mi = o.get(4..6)?.parse::<i64>().ok()?;
            sign * (h * 3600 + mi * 60)
        }
    };
    // Days from civil (Howard Hinnant).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86400 + hh * 3600 + mm * 60 + ss - offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::path::Path;

    /// Recorded on 2026-10-10 from `token-burn status --json` / `forecast --json`.
    const STATUS: &str = include_str!("../tests/fixtures/token-burn/status.json");
    const FORECAST: &str = include_str!("../tests/fixtures/token-burn/forecast.json");

    /// token-burn rows through the adapter, scoped by the default windows.toml.
    fn scoped(status: &[Value], forecast: &[Value]) -> Sample {
        let mut s = token_burn::sample(status, forecast);
        Scopes::parse(scopes::DEFAULT).unwrap().apply(&mut s);
        s
    }

    fn recorded() -> Sample {
        scoped(
            &token_burn::rows(STATUS).unwrap(),
            &token_burn::rows(FORECAST).unwrap(),
        )
    }

    /// 2026-10-10T07:40:00Z, just after the recording.
    fn at_recording() -> i64 {
        parse_rfc3339("2026-10-10T07:40:00Z").unwrap()
    }

    fn sample(status: Value, forecast: Value) -> Sample {
        scoped(status.as_array().unwrap(), forecast.as_array().unwrap())
    }

    fn row(p: &str, w: &str, left: f64, reset: &str) -> Value {
        json!({"Provider": p, "WindowName": w, "RemainingPercent": left,
               "ResetAt": reset, "LimitReached": false,
               "ObservedAt": "2026-10-10T07:39:45Z"})
    }

    fn strs(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn kinds_map_to_token_burn_providers() {
        assert_eq!(provider_for("claude", &[]), Some("claude"));
        assert_eq!(provider_for("codex", &[]), Some("codex"));
        assert_eq!(provider_for("agy", &[]), Some("antigravity"));
        assert_eq!(
            provider_for("pi", &strs(&["--provider", "xai"])),
            Some("xai")
        );
        assert_eq!(provider_for("pi", &strs(&["--provider=zai"])), Some("zai"));
        assert_eq!(
            provider_for("pi", &strs(&["--provider", "openai-codex"])),
            None
        );
        assert_eq!(provider_for("pi", &[]), None);
        assert_eq!(
            provider_for("pi", &strs(&["--model", "xai/grok-4.7"])),
            Some("xai")
        );
        assert_eq!(
            provider_for("pi", &strs(&["--model=zai/glm-5.3"])),
            Some("zai")
        );
        assert_eq!(
            provider_for("pi", &strs(&["--model=openai-codex/gpt-5.5"])),
            None
        );
        assert_eq!(
            provider_for("pi", &strs(&["--provider", "xai", "--model", "grok-4.7"])),
            Some("xai")
        );
        // An explicit --provider wins over the model's prefix.
        assert_eq!(
            provider_for("pi", &strs(&["--model", "zai/glm-5", "--provider", "xai"])),
            Some("xai")
        );
        // A bare model is ambiguous: no guess.
        assert_eq!(provider_for("pi", &strs(&["--model", "grok-4.7"])), None);
        assert_eq!(
            provider_for("pi", &strs(&["--provider", "github-copilot"])),
            None
        );
        assert_eq!(provider_for("opencode", &[]), None);
    }

    #[test]
    fn recorded_sample_binds_the_account_window_with_least_left_per_provider() {
        let rows = headroom(&recorded(), at_recording());
        let names: Vec<_> = rows
            .iter()
            .map(|h| format!("{}/{}", h.provider, h.window))
            .collect();
        // claude: seven_day (75%) binds, not five_hour (86%); zai 5h reset
        // passed, so weekly binds; antigravity ties at 100%: the first wins.
        assert_eq!(
            names,
            [
                "antigravity/claude_and_gpt",
                "claude/seven_day",
                "codex/primary",
                "copilot/premium_interactions",
                "xai/weekly",
                "zai/weekly",
            ]
        );
        let claude = &rows[1];
        assert_eq!(claude.left, 75.0);
        // seven_day: confident forecast (0.93) runs out 2026-10-13, before
        // its reset on 10-16.
        assert!(claude.out_in.is_some_and(|s| s > 2 * 86400));
        assert!(!claude.thin());
        // codex primary runs out after its reset: no exhaustion.
        assert_eq!(rows[2].out_in, None);
    }

    #[test]
    fn windows_lists_every_window_and_marks_the_binding_one() {
        let w = windows(&recorded(), at_recording());
        let names: Vec<String> = w
            .iter()
            .map(|w| {
                let mark = if w.binding { "*" } else { "" };
                format!("{}/{}{mark}", w.headroom.provider, w.headroom.window)
            })
            .collect();
        assert_eq!(
            names,
            [
                "antigravity/claude_and_gpt*",
                "antigravity/claude_and_gpt_weekly",
                "antigravity/gemini",
                "antigravity/gemini_weekly",
                "claude/five_hour",
                "claude/seven_day*",
                "claude/seven_day_fable",
                "codex/primary*",
                "copilot/chat",
                "copilot/completions",
                "copilot/premium_interactions*",
                "xai/weekly*",
                "zai/5h",
                "zai/weekly*",
            ]
        );
    }

    #[test]
    fn status_table_from_the_recorded_sample() {
        let lines = status_lines(Ok(recorded()), &Ok(Ledger::default()), at_recording());
        assert_eq!(
            lines,
            [
                "capacity[6]{provider,window,left,resets_in,runs_out,resets_owned}:",
                "  antigravity,claude_and_gpt,100%,4h57m,,",
                "  claude,seven_day,75%,5d17h,in ~3d8h,",
                "  codex,primary,80%,3d22h,,",
                "  copilot,premium_interactions,99.7%,21d16h,,",
                "  xai,weekly,97%,4d6h,,",
                "  zai,weekly,100%,2d3h,,",
            ]
        );
    }

    #[test]
    fn stale_sample_says_so() {
        let lines = status_lines(
            Ok(recorded()),
            &Ok(Ledger::default()),
            at_recording() + 3 * 3600,
        );
        assert!(lines.last().unwrap().starts_with("capacity_sample: 3h old"));
    }

    #[test]
    fn unknown_when_the_source_fails_or_has_no_account_window() {
        assert_eq!(
            status_lines(
                Err("token-burn not found".into()),
                &Ok(Ledger::default()),
                0
            ),
            ["capacity: unknown (token-burn not found)"]
        );
        let only_fable = sample(
            json!([row(
                "claude",
                "seven_day_fable",
                5.0,
                "2026-11-01T00:00:00Z"
            )]),
            json!([]),
        );
        assert_eq!(
            status_lines(
                Ok(only_fable.clone()),
                &Ok(Ledger::default()),
                at_recording()
            ),
            ["capacity: unknown (no account window in the sample)"]
        );
        let (f, w) = spawn_check("claude", &[], Ok(only_fable), &Ledger::default(), 0);
        assert_eq!(
            f,
            "capacity: unknown (no claude account window in the sample)"
        );
        assert_eq!(w, None);
    }

    #[test]
    fn spawn_with_headroom_has_a_field_and_no_warning() {
        let (field, warning) = spawn_check(
            "codex",
            &[],
            Ok(recorded()),
            &Ledger::default(),
            at_recording(),
        );
        assert_eq!(field, "capacity: codex 80% left (primary, resets in 3d22h)");
        assert_eq!(warning, None);
    }

    #[test]
    fn spawn_warns_on_a_near_empty_window() {
        let s = sample(
            json!([
                row("claude", "five_hour", 6.0, "2026-10-10T09:40:00Z"),
                row("claude", "seven_day", 70.0, "2026-10-16T00:00:00Z"),
            ]),
            json!([]),
        );
        let (field, warning) =
            spawn_check("claude", &[], Ok(s), &Ledger::default(), at_recording());
        assert_eq!(field, "capacity: claude 6% left (five_hour, resets in 2h)");
        assert!(warning
            .unwrap()
            .starts_with("warning: claude binding window is thin"));
    }

    fn thin_claude() -> Sample {
        sample(
            json!([
                row("claude", "five_hour", 6.0, "2026-10-10T09:40:00Z"),
                row("claude", "seven_day", 70.0, "2026-10-16T00:00:00Z"),
            ]),
            json!([{"provider": "claude", "window_name": "five_hour", "confidence": 0.9,
                    "estimated_100_at": "2026-10-10T08:40:00Z"}]),
        )
    }

    fn owned(n: u32) -> Ledger {
        let mut l = Ledger::default();
        l.set_owned("claude", n);
        l
    }

    #[test]
    fn a_planned_reset_covers_a_thin_window_without_a_nudge() {
        let mut l = owned(1);
        // at_recording is Saturday; the week ends on Sunday 2026-10-11.
        l.plan("claude", "2026-10-11").unwrap();
        let (field, warning) = spawn_check("claude", &[], Ok(thin_claude()), &l, at_recording());
        assert_eq!(
            field,
            "capacity: claude 6% left (five_hour, resets in 2h; out in ~1h; reset planned this week)"
        );
        assert_eq!(warning, None);
        let lines = status_lines(Ok(thin_claude()), &Ok(l), at_recording());
        assert_eq!(
            lines[1],
            "  claude,five_hour,6%,2h,in ~1h; reset planned this week,1"
        );
    }

    #[test]
    fn an_override_silences_the_warning_until_its_day() {
        let mut l = Ledger::default();
        l.override_until("claude", "2026-10-12").unwrap();
        let (field, warning) = spawn_check("claude", &[], Ok(thin_claude()), &l, at_recording());
        assert!(field.ends_with("; override until 2026-10-12)"), "{field}");
        assert_eq!(warning, None);
        // After its day the warning is back.
        let later = at_recording() + 3 * 86400;
        let (_, warning) = spawn_check("claude", &[], Ok(thin_claude()), &l, later);
        assert_eq!(warning, None, "the five_hour reset passed: full again");
        let (_, warning) = spawn_check("claude", &[], Ok(thin_claude()), &owned(0), at_recording());
        assert!(warning.is_some());
    }

    #[test]
    fn owned_resets_without_a_plan_still_warn_and_name_the_plan_command() {
        let (_, warning) = spawn_check("claude", &[], Ok(thin_claude()), &owned(1), at_recording());
        let w = warning.unwrap();
        assert!(
            w.ends_with("; 1 claude reset(s) owned: vega capacity plan claude --this-week"),
            "{w}"
        );
        let lines = status_lines(Ok(thin_claude()), &Ok(owned(1)), at_recording());
        assert_eq!(lines[1], "  claude,five_hour,6%,2h,in ~1h (soon),1");
    }

    #[test]
    fn status_flags_a_passed_plan_and_an_unreadable_ledger() {
        let mut l = owned(1);
        l.plan("claude", "2026-10-09").unwrap();
        let lines = status_lines(Ok(thin_claude()), &Ok(l), at_recording());
        assert_eq!(
            lines.last().unwrap(),
            "capacity_resets: claude reset planned by 2026-10-09, not recorded as used (vega capacity used claude, or plan claude --at <date>)"
        );
        let lines = status_lines(Ok(thin_claude()), &Err("bad json".into()), at_recording());
        assert!(lines[1].contains("(soon)"));
        assert!(lines
            .last()
            .unwrap()
            .starts_with("capacity_resets: unreadable (bad json)"));
    }

    #[test]
    fn spawn_warns_when_the_forecast_runs_out_within_two_hours() {
        let s = sample(
            json!([row("xai", "weekly", 30.0, "2026-10-14T00:00:00Z")]),
            json!([{"provider": "xai", "window_name": "weekly", "confidence": 0.9,
                    "estimated_100_at": "2026-10-10T09:10:00Z"}]),
        );
        let args = strs(&["--provider", "xai"]);
        let (field, warning) = spawn_check("pi", &args, Ok(s), &Ledger::default(), at_recording());
        assert_eq!(
            field,
            "capacity: xai 30% left (weekly, resets in 3d16h; out in ~1h30m)"
        );
        assert!(warning.is_some());
    }

    #[test]
    fn weak_or_post_reset_forecasts_do_not_warn() {
        let s = |confidence: f64, at: &str| {
            sample(
                json!([row("codex", "primary", 30.0, "2026-10-10T09:00:00Z")]),
                json!([{"provider": "codex", "window_name": "primary",
                        "confidence": confidence, "estimated_100_at": at}]),
            )
        };
        let now = at_recording();
        let (_, w) = spawn_check(
            "codex",
            &[],
            Ok(s(0.3, "2026-10-10T08:00:00Z")),
            &Ledger::default(),
            now,
        );
        assert_eq!(w, None, "low confidence");
        let (_, w) = spawn_check(
            "codex",
            &[],
            Ok(s(0.9, "2026-10-10T09:30:00Z")),
            &Ledger::default(),
            now,
        );
        assert_eq!(w, None, "runs out after the reset");
    }

    #[test]
    fn limit_reached_is_empty_and_a_passed_reset_is_full() {
        let mut hit = row("codex", "primary", 40.0, "2026-10-12T00:00:00Z");
        hit["LimitReached"] = json!(true);
        let h = for_provider(&sample(json!([hit]), json!([])), "codex", at_recording()).unwrap();
        assert_eq!(h.left, 0.0);
        assert!(h.thin());
        let old = row("codex", "primary", 2.0, "2026-10-10T07:00:00Z");
        let h = for_provider(&sample(json!([old]), json!([])), "codex", at_recording()).unwrap();
        assert_eq!((h.left, h.resets_in), (100.0, None));
    }

    #[test]
    fn spawn_unknowns() {
        let (f, w) = spawn_check(
            "pi",
            &[],
            Ok(recorded()),
            &Ledger::default(),
            at_recording(),
        );
        assert_eq!(
            f,
            "capacity: unknown (pi without --provider or --model of xai|zai)"
        );
        assert_eq!(w, None);
        let (f, _) = spawn_check(
            "claude",
            &[],
            Err("token-burn not found".into()),
            &Ledger::default(),
            0,
        );
        assert_eq!(f, "capacity: unknown (token-burn not found)");
        let (f, _) = spawn_check("opencode", &[], Ok(recorded()), &Ledger::default(), 0);
        assert_eq!(
            f,
            "capacity: unknown (vega maps kind opencode to no provider)"
        );
    }

    #[test]
    fn a_provider_only_the_fixture_knows_shows_and_binds_and_a_model_window_never_binds() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/unknown-provider");
        let tb = TokenBurn {
            bin: "/nonexistent/token-burn".into(),
            fixture: Some(dir.to_string_lossy().into_owned()),
        };
        let s = read_from(&tb, &Scopes::parse(scopes::DEFAULT).unwrap()).unwrap();
        let w: Vec<String> = windows(&s, at_recording())
            .iter()
            .map(|w| {
                let mark = if w.binding { "*" } else { "" };
                format!("{}/{}{mark}", w.headroom.provider, w.headroom.window)
            })
            .collect();
        // seven_day_fable has the least left, but is model-scoped.
        assert_eq!(
            w,
            [
                "acme/daily",
                "acme/monthly*",
                "claude/five_hour*",
                "claude/seven_day_fable"
            ]
        );
        let (field, warning) =
            spawn_check("claude", &[], Ok(s), &Ledger::default(), at_recording());
        assert!(
            field.starts_with("capacity: claude 60% left (five_hour, "),
            "{field}"
        );
        assert_eq!(warning, None);
    }

    #[test]
    fn durations_and_percents_are_compact() {
        assert_eq!(duration(35 * 60), "35m");
        assert_eq!(duration(4 * 3600), "4h");
        assert_eq!(duration(4 * 3600 + 12 * 60), "4h12m");
        assert_eq!(duration(5 * 86400 + 15 * 3600 + 59), "5d15h");
        assert_eq!(duration(2 * 86400), "2d");
        assert_eq!(percent(99.96833), "100");
        assert_eq!(percent(99.7), "99.7");
        assert_eq!(percent(75.0), "75");
    }
}
