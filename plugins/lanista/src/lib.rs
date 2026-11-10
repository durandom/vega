//! Model pins shared by lanista's CLI and agent spawning.
pub mod hosts;
pub mod seats;

use std::path::Path;
use vega_herdr::Failure;

/// Resolve a tier in the target checkout, using the same project overrides as
/// `vega lanista pin`. An unavailable registry falls back to the shipped pins.
pub fn resolve(provider: &str, tier: &str, cwd: &Path) -> Result<String, Failure> {
    let pins = seats::pins(seats::SEATS, seats::DEFAULT_SEAT).expect("embedded seats are tested");
    let pin = seats::global(&pins, provider, tier).ok_or_else(|| {
        Failure::new(
            "usage",
            format!("no {tier} pin for kind {provider}"),
            "use --kind claude|codex|pi|agy, or pass an explicit --model after --",
        )
    })?;
    let local = vega_herdr::state::post_host();
    let host_config = hosts::read(&local).unwrap_or_default();
    let defaults = hosts::read_default(&local).unwrap_or_default();
    let fallback_pin = hosts::resolve_cell(
        &vega_registry::View::default(),
        &host_config,
        &defaults,
        None,
        seats::DEFAULT_SEAT,
        provider,
        tier,
    );
    let fallback = fallback_pin
        .as_ref()
        .map_or_else(|| pin.model.clone(), |p| p.model.clone());
    let report = |p: Option<&hosts::Resolution>| {
        if provider == "pi" {
            if let Some(p) = p {
                eprintln!(
                    "vega lanista: lane {} ({}{})",
                    p.lane,
                    p.source,
                    if p.source == "host" {
                        format!(" {local}")
                    } else {
                        String::new()
                    }
                );
            }
        }
    };
    if vega_herdr::state::is_inbox_only(&local)
        && std::env::var_os("VEGA_LANISTA_PIN_HOSTS").is_none()
    {
        report(fallback_pin.as_ref());
        return Ok(fallback);
    }
    let project = vega_herdr::project::resolve_optional(cwd, &vega_herdr::state::post_host())?;
    let Some(project) = project else {
        report(fallback_pin.as_ref());
        return Ok(fallback);
    };
    let record = vega_registry::pins::Pin {
        schema: 2,
        project: project.id.clone(),
        seat: seats::DEFAULT_SEAT.into(),
        lane: provider.into(),
        tier: tier.into(),
        model: None,
        at: 0,
        expires: None,
    };
    match vega_registry::pins::access(record, None) {
        Ok(view) => {
            let resolved = hosts::resolve_cell(
                &view,
                &host_config,
                &defaults,
                Some(project.id.as_str()),
                seats::DEFAULT_SEAT,
                provider,
                tier,
            );
            report(resolved.as_ref());
            Ok(resolved.map_or(fallback, |p| p.model))
        }
        Err(e) => {
            eprintln!("vega lanista: registry unavailable: {e}; using host/global fallback");
            report(fallback_pin.as_ref());
            Ok(fallback)
        }
    }
}

/// Pure precedence shared by callers and tests: project, host, global.
pub fn select(
    view: &vega_registry::View,
    config: &hosts::Config,
    project: &str,
    seat: &str,
    lane: &str,
    tier: &str,
) -> Option<String> {
    select_layers(
        view,
        config,
        &hosts::Config::default(),
        project,
        seat,
        lane,
        tier,
    )
}
#[allow(clippy::too_many_arguments)]
pub fn select_layers(
    view: &vega_registry::View,
    config: &hosts::Config,
    defaults: &hosts::Config,
    project: &str,
    seat: &str,
    lane: &str,
    tier: &str,
) -> Option<String> {
    hosts::resolve_cell(view, config, defaults, Some(project), seat, lane, tier).map(|p| p.model)
}

/// Active project override (host, model, expiry), shared with the pin CLI.
pub fn effective_model(
    view: &vega_registry::View,
    project: &str,
    seat: &str,
    provider: &str,
    tier: &str,
) -> Option<(String, String, Option<i64>)> {
    let (host, pin) = vega_registry::pins::latest(view, project, seat, provider, tier)?;
    let model = pin.active(vega_herdr::capacity::now())?;
    Some((host.to_string(), model.to_string(), pin.expires))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pi_overrides_google_but_defaults_to_zai() {
        let mut config = hosts::Config::default();
        let view = vega_registry::View::default();
        assert_eq!(
            select(&view, &config, "p", seats::DEFAULT_SEAT, "pi", "budget").unwrap(),
            "glm-4.7"
        );
        config.apply(&hosts::Change {
            seat: seats::DEFAULT_SEAT.into(),
            lane: "pi".into(),
            tier: "budget".into(),
            model: Some("google/gemini-3.8-flash".into()),
            clear: false,
            until: None,
        });
        assert_eq!(
            select(&view, &config, "p", seats::DEFAULT_SEAT, "pi", "budget").unwrap(),
            "google/gemini-3.8-flash"
        );
        let pins = seats::pins(seats::SEATS, seats::DEFAULT_SEAT).unwrap();
        assert!(pins
            .iter()
            .filter(|p| p.provider == "pi")
            .all(|p| p.tier.is_empty()));
    }

    #[test]
    fn precedence_is_project_then_host_then_global() {
        let mut config = hosts::Config::default();
        let mut view = vega_registry::View::default();
        let seat = seats::DEFAULT_SEAT;
        let get = |v: &vega_registry::View, c: &hosts::Config| {
            select(v, c, "github.com/team/project", seat, "codex", "budget").unwrap()
        };
        assert_eq!(get(&view, &config), "gpt-6-luna");
        config.apply(&hosts::Change {
            seat: seat.into(),
            lane: "codex".into(),
            tier: "budget".into(),
            model: Some("gpt-6.1-sol".into()),
            clear: false,
            until: None,
        });
        assert_eq!(get(&view, &config), "gpt-6.1-sol");
        let key = vega_registry::pins::key(
            "proteus",
            "github.com/team/project",
            seat,
            "codex",
            "budget",
        );
        let mut pin = json!({"schema":2,"project":"github.com/team/project","seat":seat,"lane":"codex","tier":"budget","model":"gpt-6-astra","at":1,"expires":null});
        view.entries.insert(key.clone(), pin.clone());
        assert_eq!(get(&view, &config), "gpt-6-astra");
        pin["expires"] = json!(1);
        view.entries.insert(key, pin);
        assert_eq!(get(&view, &config), "gpt-6.1-sol");
    }

    #[test]
    fn resolver_uses_active_override_and_respects_clear_and_expiry() {
        let project = "github.com/team/project";
        let mut view = vega_registry::View::default();
        let key = vega_registry::pins::key("host", project, seats::DEFAULT_SEAT, "codex", "budget");
        let mut pin = json!({"schema": 2, "project": project, "seat": seats::DEFAULT_SEAT,
            "lane": "codex", "tier": "budget", "model": "custom", "at": 1, "expires": null});
        view.entries.insert(key.clone(), pin.clone());
        assert_eq!(
            effective_model(&view, project, seats::DEFAULT_SEAT, "codex", "budget")
                .unwrap()
                .1,
            "custom"
        );
        assert!(
            effective_model(&view, project, seats::DEFAULT_SEAT, "codex", "balanced").is_none()
        );
        pin["expires"] = json!(1);
        view.entries.insert(key.clone(), pin.clone());
        assert!(effective_model(&view, project, seats::DEFAULT_SEAT, "codex", "budget").is_none());
        pin["expires"] = json!(null);
        pin["model"] = json!(null);
        view.entries.insert(key, pin);
        assert!(effective_model(&view, project, seats::DEFAULT_SEAT, "codex", "budget").is_none());
    }
}
