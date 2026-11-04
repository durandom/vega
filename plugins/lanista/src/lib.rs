//! Model pins shared by lanista's CLI and agent spawning.
pub mod seats;

use std::path::Path;
use vega_herdr::Failure;

/// Resolve a tier in the target checkout, using the same project overrides as
/// `vega lanista pin`. An unavailable registry falls back to the shipped pins.
pub fn resolve(provider: &str, tier: &str, cwd: &Path) -> Result<String, Failure> {
    let pins = seats::pins(seats::SEATS, seats::DEFAULT_SEAT).expect("embedded seats are tested");
    let pin = seats::pick(&pins, provider, tier).ok_or_else(|| {
        Failure::new(
            "usage",
            format!("no {tier} pin for kind {provider}"),
            "use --kind claude|codex|pi|agy, or pass an explicit --model after --",
        )
    })?;
    let project = vega_herdr::project::resolve_optional(cwd, &vega_herdr::state::post_host())?;
    let Some(project) = project else {
        return Ok(pin.model.clone());
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
        Ok(view) => Ok(effective_model(
            &view,
            project.id.as_str(),
            seats::DEFAULT_SEAT,
            provider,
            tier,
        )
        .map_or_else(|| pin.model.clone(), |(_, model, _)| model)),
        Err(e) => {
            eprintln!("vega lanista: registry unavailable: {e}; using global seats");
            Ok(pin.model.clone())
        }
    }
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
