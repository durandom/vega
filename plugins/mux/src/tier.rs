//! Deliberate spawn model selection, backed by lanista's project pins.
use super::*;

/// Task policy belongs to vega; lanista remains the tier-to-model authority.
#[derive(Default)]
pub struct Policy {
    pub tier: Option<String>,
    pub why: Option<String>,
    pub bead: Option<String>,
}

fn bd(args: &[&str], cwd: Option<&str>) -> Result<Vec<u8>, Failure> {
    let mut cmd = Command::new("bd");
    let dir = checkout(cwd)?;
    if let Some(project) = vega_bd_sync::group::coordination(&dir)? {
        cmd.env("BEADS_DIR", project.beads_dir().expect("validated registration"));
    }
    cmd.args(args).current_dir(dir);
    let hint = "ensure bd is installed and the bead exists in the local checkout";
    let out = vega_herdr::output_within(&mut cmd, std::time::Duration::from_secs(10))
        .map_err(|e| Failure::new("bead_read_failed", format!("cannot run bd: {e}"), hint))?
        .ok_or_else(|| Failure::new("bead_timeout", "bd exceeded 10 seconds", hint))?;
    if !out.status.success() {
        return Err(Failure::new(
            "bead_command_failed",
            format!("bd {} failed for bead {}", args[0], args[1]),
            hint,
        ));
    }
    Ok(out.stdout)
}

fn bead_tier(bead: &Value) -> Result<(&str, String, bool), Failure> {
    let labels: Vec<_> = bead["labels"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|s| s.strip_prefix("tier:"))
        .collect();
    if !labels.is_empty() {
        if labels.len() != 1 || !["flagship", "balanced", "budget"].contains(&labels[0]) {
            return Err(Failure::new(
                "usage",
                "bead needs exactly one valid tier label",
                "set one tier:flagship|balanced|budget label",
            ));
        }
        return Ok((labels[0], format!("bead label tier:{}", labels[0]), true));
    }
    let kind = bead["issue_type"].as_str().unwrap_or("");
    let tier = match kind {
        "decision" | "research" => "flagship",
        "task" | "bug" | "feature" | "epic" => "balanced",
        "chore" => "budget",
        _ => {
            return Err(Failure::new(
                "usage",
                format!("no tier policy for bead type {kind:?}"),
                "set a tier:flagship|balanced|budget label on the bead",
            ))
        }
    };
    Ok((tier, format!("bead type {kind}"), false))
}

pub fn policy(
    tier: Option<&str>,
    bead: Option<&str>,
    why: Option<&str>,
    cwd: Option<&str>,
) -> Result<Policy, Failure> {
    if bead.is_some_and(|s| s.trim().is_empty() || s.starts_with('-'))
        || why.is_some_and(|s| s.trim().is_empty())
    {
        return Err(Failure::new(
            "usage",
            "--bead and --why must be nonempty",
            "pass --bead <id> and --why <reason>",
        ));
    }
    let mut selected = tier.map(String::from);
    let mut reason = why.map(String::from);
    let mut authorized_flagship = false;
    if let Some(id) = bead {
        let bytes = bd(&["show", id, "--json"], cwd)?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| {
            Failure::new(
                "bead_read_failed",
                "bd returned invalid JSON",
                "run bd show <id> --json",
            )
        })?;
        let item = value
            .as_array()
            .filter(|a| a.len() == 1)
            .and_then(|a| a.first())
            .filter(|v| v["id"].as_str() == Some(id))
            .ok_or_else(|| {
                Failure::new(
                    "bead_not_found",
                    format!("bd did not return bead {id}"),
                    "run bd show <id> --json in the local checkout",
                )
            })?;
        let (derived, source, labelled) = bead_tier(item)?;
        if tier.is_some_and(|t| t != derived) && why.is_none() {
            return Err(Failure::new(
                "usage",
                format!("--tier differs from bead tier {derived}"),
                "pass --why <reason> to override the bead tier",
            ));
        }
        selected = Some(tier.unwrap_or(derived).into());
        authorized_flagship =
            labelled && derived == "flagship" && selected.as_deref() == Some("flagship");
        reason = reason.or(Some(source));
    }
    if selected.as_deref() == Some("flagship") && !authorized_flagship && why.is_none() {
        return Err(Failure::new(
            "usage",
            "flagship requires a recorded reason",
            "pass --why <reason> (or use a bead labelled tier:flagship)",
        ));
    }
    Ok(Policy {
        tier: selected,
        why: reason,
        bead: bead.map(String::from),
    })
}

impl Policy {
    pub fn record(&self, model: Option<&str>, cwd: Option<&str>) -> Result<(), Failure> {
        if let Some(id) = &self.bead {
            let note = format!(
                "mux spawn selection: tier={}, model={}, reason={}",
                self.tier.as_deref().unwrap_or("-"),
                model.unwrap_or("-"),
                self.why.as_deref().unwrap_or("-")
            );
            bd(&["update", id, "--append-notes", &note], cwd)?;
        }
        Ok(())
    }
}

pub struct Selection {
    pub args: Vec<String>,
    pub model: Option<String>,
}

/// Last explicit model wins, as it does for the harness. Do not interpret
/// text inside other arguments (notably Codex config values) as a model flag.
pub fn explicit_model(args: &[String]) -> Option<String> {
    let mut model = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            break;
        }
        if arg == "-m" || arg == "--model" {
            if let Some(value) = args.next().filter(|v| !v.is_empty() && !v.starts_with('-')) {
                model = Some(value.clone());
            }
        } else if let Some(value) = arg
            .strip_prefix("--model=")
            .or_else(|| arg.strip_prefix("-m=").or_else(|| arg.strip_prefix("-m")))
        {
            if !value.is_empty() {
                model = Some(value.into());
            }
        }
    }
    model
}

fn checkout(cwd: Option<&str>) -> Result<PathBuf, Failure> {
    match cwd.filter(|_| machine().is_none()) {
        Some(cwd) => Ok(PathBuf::from(expand_home(
            cwd,
            &env::var("HOME").unwrap_or_default(),
        ))),
        None => env::current_dir()
            .map_err(|e| Failure::new("no_cwd", e.to_string(), "pass --cwd <dir>")),
    }
}

pub fn required(kind: &str, cwd: Option<&str>) -> Failure {
    let choices = [
        ("flagship", "design/root-cause"),
        ("balanced", "normal implementation"),
        ("budget", "mechanical"),
    ]
    .into_iter()
    .map(|(tier, criteria)| {
        let model = checkout(cwd)
            .and_then(|cwd| vega_lanista::resolve(kind, tier, &cwd))
            .unwrap_or_else(|e| format!("unavailable ({})", e.message));
        format!("--tier {tier}: {criteria} ({model})")
    })
    .collect::<Vec<_>>()
    .join("; ");
    Failure::new(
        "usage",
        "spawn requires --bead <id> or --tier flagship|balanced|budget (no default)",
        &choices,
    )
}

pub fn select(
    kind: &str,
    tier: Option<&str>,
    extra: &[String],
    cwd: Option<&str>,
) -> Result<Selection, Failure> {
    if let Some(model) = explicit_model(extra) {
        return Ok(Selection {
            args: extra.to_vec(),
            model: Some(model),
        });
    }
    let Some(tier) = tier else {
        return Ok(Selection {
            args: extra.to_vec(),
            model: None,
        });
    };
    let model = vega_lanista::resolve(kind, tier, &checkout(cwd)?)?;
    // Pi accepts provider/model and otherwise may find an ambiguous model id.
    let model = if kind == "pi" && !model.contains('/') {
        format!("zai/{model}")
    } else {
        model
    };
    let mut args = vec!["--model".into(), model.clone()];
    args.extend_from_slice(extra);
    Ok(Selection {
        args,
        model: Some(model),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    /// Re-run one test on a fake host (puck) with hermetic config/cache
    /// homes (`home`), so no host-local seats overlay leaks into tier
    /// resolution. Seed overlay and cache files under `home` first.
    fn fake_host_rerun(marker: &str, test: &str, home: &std::path::Path) -> std::process::Output {
        std::process::Command::new(env::current_exe().unwrap())
            .args(["--exact", test, "--nocapture"])
            .env(marker, "1")
            .env("XDG_CONFIG_HOME", home)
            .env("XDG_CACHE_HOME", home)
            .env("VEGA_POST_HOST", "puck")
            .env("VEGA_NATS_URL", "nats://127.0.0.1:1")
            .env("VEGA_NATS_NKEY", home.join("missing"))
            .env_remove("VEGA_LANISTA_PIN_HOSTS")
            .output()
            .unwrap()
    }
    fn assert_ok(out: &std::process::Output) {
        assert!(
            out.status.success(),
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn pi_host_override_reaches_spawn_model() {
        const MARKER: &str = "VEGA_PI_HOST_TIER_TEST";
        if env::var_os(MARKER).is_none() {
            let dir = tempfile::tempdir().unwrap();
            let cache = dir.path().join("vega/lanista");
            std::fs::create_dir_all(&cache).unwrap();
            std::fs::write(cache.join("puck.json"), r#"{"schema":1,"seats":{"herdr-agents":{"pi":{"budget":{"model":"google/gemini-3.8-flash","until":null}}}}}"#).unwrap();
            let out = fake_host_rerun(
                MARKER,
                "tier::tests::pi_host_override_reaches_spawn_model",
                dir.path(),
            );
            assert_ok(&out);
            assert!(String::from_utf8_lossy(&out.stderr).contains("lane pi (host puck)"));
            return;
        }
        let selected = select("pi", Some("budget"), &[], None).unwrap();
        assert_eq!(selected.model.as_deref(), Some("google/gemini-3.8-flash"));
        assert_eq!(
            selected.args,
            strings(&["--model", "google/gemini-3.8-flash"])
        );
    }

    #[test]
    fn tiers_select_pins_and_provider_for_every_kind() {
        const MARKER: &str = "VEGA_TIERS_SELECT_TEST";
        if env::var_os(MARKER).is_none() {
            let home = tempfile::tempdir().unwrap();
            let out = fake_host_rerun(
                MARKER,
                "tier::tests::tiers_select_pins_and_provider_for_every_kind",
                home.path(),
            );
            assert_ok(&out);
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_str();
        for kind in ["codex", "claude", "pi", "agy"] {
            for tier in ["flagship", "balanced", "budget"] {
                let selected = select(kind, Some(tier), &[], cwd).unwrap();
                let pins = vega_lanista::seats::pins(
                    vega_lanista::seats::SEATS,
                    vega_lanista::seats::DEFAULT_SEAT,
                )
                .unwrap();
                let pin = vega_lanista::seats::global(&pins, kind, tier).unwrap();
                let expected = if kind == "pi" {
                    format!("zai/{}", pin.model)
                } else {
                    pin.model.clone()
                };
                assert_eq!(selected.model.as_deref(), Some(expected.as_str()));
                let agent_args = spawn_args(kind, &[], &[], &selected.args);
                assert!(agent_args.windows(2).any(|a| a == ["--model", &expected]));
            }
        }
    }

    #[test]
    fn model_flags_override_tier_and_skip_pin_lookup() {
        for args in [
            strings(&["-m", "custom"]),
            strings(&["--model", "custom"]),
            strings(&["--model=custom"]),
            strings(&["-m=custom"]),
            strings(&["-mcustom"]),
        ] {
            for tier in [None, Some("balanced")] {
                let selected = select("codex", tier, &args, Some("/nonexistent/checkout")).unwrap();
                assert_eq!(selected.args, args);
                assert_eq!(selected.model.as_deref(), Some("custom"));
            }
        }
        assert_eq!(
            explicit_model(&strings(&["-m", "first", "--model=last"])).as_deref(),
            Some("last")
        );
        for args in [
            strings(&["--model"]),
            strings(&["--model="]),
            strings(&["--model", "--other"]),
            strings(&["-c", "model=foo"]),
        ] {
            assert_eq!(explicit_model(&args), None);
        }
    }

    #[test]
    fn missing_tier_is_usage_with_resolved_choices_and_exceptions() {
        const MARKER: &str = "VEGA_MISSING_TIER_TEST";
        if env::var_os(MARKER).is_none() {
            let home = tempfile::tempdir().unwrap();
            let out = fake_host_rerun(
                MARKER,
                "tier::tests::missing_tier_is_usage_with_resolved_choices_and_exceptions",
                home.path(),
            );
            assert_ok(&out);
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let base = [
            "mux",
            "spawn",
            "fix",
            "--kind",
            "codex",
            "--cwd",
            dir.path().to_str().unwrap(),
        ];
        let cli = Cli::try_parse_from(base).unwrap();
        let failure = validate(cli.cmd.as_ref().unwrap()).unwrap();
        assert_eq!(failure.code, "usage");
        for text in [
            "flagship",
            "design/root-cause",
            "gpt-6.2-astra",
            "balanced",
            "normal implementation",
            "gpt-6.2-sol",
            "budget",
            "mechanical",
            "gpt-6-luna",
        ] {
            assert!(failure.hint.contains(text), "{} lacks {text}", failure.hint);
        }
        for extra in [
            vec!["--no-parent"],
            vec!["--tier", "budget"],
            vec!["--", "-m", "custom"],
        ] {
            let cli = Cli::try_parse_from(base.into_iter().chain(extra)).unwrap();
            assert!(validate(cli.cmd.as_ref().unwrap()).is_none());
        }
        assert!(Cli::try_parse_from(base.into_iter().chain(["--tier", "unknown"])).is_err());
        assert!(
            Cli::try_parse_from(["mux", "spawn", "--resume", "w1:p1", "--tier", "budget"]).is_err()
        );
    }

    /// The puck story: an inbox-only host pins its pi tiers to its EnMaaS
    /// gateway via ~/.config/vega/lanista/seats.local.json, and spawn tiers
    /// resolve enmaas/<id> without any repo seats edit.
    #[test]
    fn overlay_pins_pi_tiers_to_the_enmaas_gateway() {
        const MARKER: &str = "VEGA_PI_OVERLAY_TIER_TEST";
        if env::var_os(MARKER).is_none() {
            let home = tempfile::tempdir().unwrap();
            let config = home.path().join("vega/lanista");
            std::fs::create_dir_all(&config).unwrap();
            std::fs::write(
                config.join("seats.local.json"),
                r#"{"schema":1,"seats":{"herdr-agents":[
                    {"provider":"pi","pin":"enmaas/gome-3.1","tier":"flagship","canonical":"gome-3.1","cost":"subscription"},
                    {"provider":"pi","pin":"enmaas/gome-2.4","tier":"balanced","cost":"budget"},
                    {"provider":"pi","pin":"enmaas/gome-1.9","tier":"budget","cost":"unknown"}
                ]}}"#,
            )
            .unwrap();
            let out = fake_host_rerun(
                MARKER,
                "tier::tests::overlay_pins_pi_tiers_to_the_enmaas_gateway",
                home.path(),
            );
            assert_ok(&out);
            assert!(String::from_utf8_lossy(&out.stderr).contains("lane pi (overlay)"));
            return;
        }
        for (tier, model) in [
            ("flagship", "enmaas/gome-3.1"),
            ("balanced", "enmaas/gome-2.4"),
            ("budget", "enmaas/gome-1.9"),
        ] {
            let selected = select("pi", Some(tier), &[], None).unwrap();
            assert_eq!(selected.model.as_deref(), Some(model));
            assert_eq!(selected.args, strings(&["--model", model]));
        }
        // Non-pi kinds keep their repo pins.
        assert_eq!(
            select("codex", Some("budget"), &[], None).unwrap().model,
            Some("gpt-6-luna".into())
        );
        // An explicit model flag still wins over the overlay.
        let selected = select(
            "pi",
            Some("flagship"),
            &strings(&["-m", "enmaas/other"]),
            None,
        )
        .unwrap();
        assert_eq!(selected.model.as_deref(), Some("enmaas/other"));
    }
}
