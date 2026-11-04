//! Deliberate spawn model selection, backed by lanista's project pins.
use super::*;

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

fn lane(kind: &str) -> &str {
    if kind == "pi" {
        "zai"
    } else {
        kind
    }
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
            .and_then(|cwd| vega_lanista::resolve(lane(kind), tier, &cwd))
            .unwrap_or_else(|e| format!("unavailable ({})", e.message));
        format!("--tier {tier}: {criteria} ({model})")
    })
    .collect::<Vec<_>>()
    .join("; ");
    Failure::new(
        "usage",
        "spawn requires --tier flagship|balanced|budget (no default)",
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
    let model = vega_lanista::resolve(lane(kind), tier, &checkout(cwd)?)?;
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

    #[test]
    fn tiers_select_pins_and_provider_for_every_kind() {
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
                let pin = vega_lanista::seats::pick(&pins, lane(kind), tier).unwrap();
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
            "gpt-6-astra",
            "balanced",
            "normal implementation",
            "gpt-6.1-sol",
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
}
