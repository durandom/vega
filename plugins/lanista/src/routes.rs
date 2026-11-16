//! Route facts through lanista: who owns what (docs/design/lanista-catalog.md,
//! lanista `docs/routes.md`).
//!
//! vega's seats overlay (`seats.local.json`, [`crate::seats`]) owns only
//! provider + tier → route id. lanista's host-local `routes.local.json`
//! owns route id → canonical id and the route facts (cost class, …).
//! Its deprecated `canonical`/`cost` keys are fallbacks, used only when
//! lanista has no entry for the route id; on conflict vega warns and
//! lanista wins; unknown stays unknown, never zero.
//!
//! Join key: the exact route id, string-equal. lanista is read once per
//! command ([`vega_herdr::routes::read`], `lanista --json routes`).

use crate::seats;
use vega_herdr::routes::{self, Table, Unavailable};

pub use vega_herdr::routes::Facts;

/// Merges the facts for `model` (the exact route id): lanista's entry wins;
/// a seats pin's deprecated `canonical`/`cost` apply only when lanista has
/// no entry (an explicit lanista null masks: no fallback, unknown). Returns
/// the conflict warnings to print; lanista wins regardless.
pub fn resolve(model: &str, pins: &[seats::Pin], table: &Table) -> (Option<Facts>, Vec<String>) {
    if let Some(route) = table.route(model) {
        let mut warnings = Vec::new();
        if let Some(pin) = pins.iter().find(|p| p.model == model) {
            if let (Some(seats_canonical), Some(route_canonical)) =
                (&pin.canonical, &route.canonical)
            {
                if seats_canonical != route_canonical {
                    warnings.push(format!(
                        "route {model}: seats overlay canonical {seats_canonical} conflicts with lanista {route_canonical}; lanista wins"
                    ));
                }
            }
            if pin.cost != "unknown" && route.cost.as_deref().is_some_and(|c| c != pin.cost) {
                warnings.push(format!(
                    "route {model}: seats overlay cost {} conflicts with lanista {}; lanista wins",
                    pin.cost,
                    route.cost.as_deref().unwrap_or("unknown")
                ));
            }
            if pin.cost != "unknown" && route.cost.is_none() {
                warnings.push(format!(
                    "route {model}: seats overlay cost {} is ignored; lanista says the cost is unknown; lanista wins",
                    pin.cost
                ));
            }
        }
        return (
            Some(Facts {
                route_id: model.to_string(),
                canonical: route.canonical.clone(),
                cost: route.cost.clone(),
                origin: origin(&route.cost_origin),
            }),
            warnings,
        );
    }
    // No lanista entry: the seats pin's deprecated canonical/cost are the
    // fallback. Repo pins carry none, so this is normally an overlay pin.
    let pin = pins
        .iter()
        .find(|p| p.model == model && (p.canonical.is_some() || p.cost != "unknown"));
    pin.map(|pin| Facts {
        route_id: model.to_string(),
        canonical: pin.canonical.clone(),
        cost: cost(&pin.cost),
        origin: "seats-overlay",
    })
    .map(|facts| (Some(facts), Vec::new()))
    .unwrap_or((None, Vec::new()))
}

/// A non-`unknown` cost string, else `None` (unknown, never zero).
fn cost(cost: &str) -> Option<String> {
    match cost {
        "" | "unknown" => None,
        c => Some(c.to_string()),
    }
}

/// lanista's origin strings are a closed vocabulary; anything else reads
/// as `unknown`.
fn origin(s: &str) -> &'static str {
    match s {
        "local" => "local",
        "catalog" => "catalog",
        _ => "unknown",
    }
}

/// [`resolve`] plus the read, for spawn paths: one `lanista --json routes`
/// call; a missing/old lanista degrades to the seats fallback with a hint.
pub fn resolve_facts(model: &str, seat: &str) -> Resolved {
    let pins = seats::effective(seat).unwrap_or_default();
    resolve_with(model, &pins, routes::read())
}

/// Pure [`resolve_facts`] over an injected table read (tests).
pub fn resolve_with(
    model: &str,
    pins: &[seats::Pin],
    table: Result<Table, Unavailable>,
) -> Resolved {
    match table {
        Ok(table) => {
            let (facts, warnings) = resolve(model, pins, &table);
            Resolved {
                facts,
                warnings,
                hint: None,
            }
        }
        Err(unavailable) => {
            // Degrade to the fallback: the seats pin's own facts, no
            // conflict check (lanista's side is unknown), hint reported.
            let (facts, _) = resolve(model, pins, &Table::default());
            Resolved {
                facts,
                warnings: Vec::new(),
                hint: Some(unavailable.to_string()),
            }
        }
    }
}

/// The outcome of a facts resolution, with everything to report.
#[derive(Debug, Default)]
pub struct Resolved {
    pub facts: Option<Facts>,
    /// Conflict warnings (lanista won); print with [`Resolved::report`].
    pub warnings: Vec<String>,
    /// Why lanista could not be consulted; the fallback is in use.
    pub hint: Option<String>,
}

impl Resolved {
    /// Prints warnings and the unavailable hint on stderr, vega-style.
    pub fn report(&self) {
        for warning in &self.warnings {
            eprintln!("vega lanista: {warning}");
        }
        if let Some(hint) = &self.hint {
            eprintln!("vega lanista: {hint}; using the seats overlay fallback");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vega_herdr::routes::Route;

    fn pin(model: &str, canonical: Option<&str>, cost: &str) -> seats::Pin {
        seats::Pin {
            provider: "pi".into(),
            model: model.into(),
            engine: "pi".into(),
            role: String::new(),
            tier: "flagship".into(),
            canonical: canonical.map(str::to_string),
            cost: cost.into(),
        }
    }

    fn table_of(routes: Vec<Route>) -> Table {
        Table {
            routes: routes
                .into_iter()
                .map(|r| (r.route_id.clone(), r))
                .collect(),
            ..Default::default()
        }
    }

    fn route(id: &str, canonical: Option<&str>, cost: Option<&str>, origin: &str) -> Route {
        Route {
            route_id: id.into(),
            canonical: canonical.map(str::to_string),
            cost: cost.map(str::to_string),
            cost_origin: origin.into(),
        }
    }

    #[test]
    fn lanista_entry_wins_and_conflicts_warn() {
        let pins = vec![pin("enmaas/gome-3.1", Some("gome-3.1"), "subscription")];
        let table = table_of(vec![route(
            "enmaas/gome-3.1",
            Some("gome-3.2"),
            Some("budget"),
            "local",
        )]);
        let (facts, warnings) = resolve("enmaas/gome-3.1", &pins, &table);
        let facts = facts.unwrap();
        assert_eq!(
            (
                facts.canonical.as_deref(),
                facts.cost.as_deref(),
                facts.origin
            ),
            (Some("gome-3.2"), Some("budget"), "local")
        );
        assert_eq!(
            warnings,
            [
                "route enmaas/gome-3.1: seats overlay canonical gome-3.1 conflicts with lanista gome-3.2; lanista wins".to_string(),
                "route enmaas/gome-3.1: seats overlay cost subscription conflicts with lanista budget; lanista wins".to_string(),
            ]
        );
        // Matching seats values stay quiet.
        let pins = vec![pin("enmaas/gome-3.1", Some("gome-3.1"), "budget")];
        let matching = table_of(vec![route(
            "enmaas/gome-3.1",
            Some("gome-3.1"),
            Some("budget"),
            "local",
        )]);
        assert!(resolve("enmaas/gome-3.1", &pins, &matching).1.is_empty());
    }

    #[test]
    fn an_explicit_lanista_null_masks_the_fallback() {
        let pins = vec![pin("enmaas/gome-3.1", None, "subscription")];
        let table = table_of(vec![route("enmaas/gome-3.1", None, None, "local")]);
        let (facts, warnings) = resolve("enmaas/gome-3.1", &pins, &table);
        let facts = facts.unwrap();
        assert_eq!((facts.cost.as_deref(), facts.origin), (None, "local"));
        assert_eq!(
            warnings,
            ["route enmaas/gome-3.1: seats overlay cost subscription is ignored; lanista says the cost is unknown; lanista wins".to_string()]
        );
        // Without the seats cost there is nothing to warn about.
        assert!(resolve("enmaas/gome-3.1", &[], &table).1.is_empty());
    }

    #[test]
    fn without_a_lanista_entry_the_seats_values_fall_back() {
        let pins = vec![
            pin("enmaas/gome-3.1", Some("gome-3.1"), "subscription"),
            pin("enmaas/gome-1.9", None, "unknown"),
            pin("glm-5.3", None, "unknown"),
        ];
        let (facts, warnings) = resolve("enmaas/gome-3.1", &pins, &table_of(vec![]));
        let facts = facts.unwrap();
        assert_eq!(
            (
                facts.canonical.as_deref(),
                facts.cost.as_deref(),
                facts.origin
            ),
            (Some("gome-3.1"), Some("subscription"), "seats-overlay")
        );
        assert_eq!(facts.summary(), "route subscription, origin seats-overlay");
        assert!(warnings.is_empty());
        // A pin without facts of its own (overlay cost unknown, repo pins)
        // resolves nothing: unknown, never zero.
        assert!(resolve("enmaas/gome-1.9", &pins, &table_of(vec![]))
            .0
            .is_none());
        assert!(resolve("glm-5.3", &pins, &table_of(vec![])).0.is_none());
        assert!(resolve("enmaas/other", &pins, &table_of(vec![]))
            .0
            .is_none());
    }

    #[test]
    fn catalog_origin_passes_through() {
        let table = table_of(vec![route("gw/x", None, Some("budget"), "catalog")]);
        let (facts, _) = resolve("gw/x", &[], &table);
        assert_eq!(facts.unwrap().origin, "catalog");
    }

    #[test]
    fn an_unavailable_lanista_degrades_to_the_fallback_with_a_hint() {
        let pins = vec![pin("enmaas/gome-3.1", Some("gome-3.1"), "subscription")];
        let resolved = resolve_with(
            "enmaas/gome-3.1",
            &pins,
            Err(Unavailable {
                why: "lanista is not installed".into(),
                hint: "upgrade or install it: vega lanista install",
            }),
        );
        let facts = resolved.facts.unwrap();
        assert_eq!(
            (facts.cost.as_deref(), facts.origin),
            (Some("subscription"), "seats-overlay")
        );
        let hint = resolved.hint.unwrap();
        assert!(hint.starts_with("lanista routes unavailable (lanista is not installed)"));
        assert!(hint.ends_with("vega lanista install"));
        // A route without seats facts stays unknown, never zero.
        assert!(resolve_with(
            "enmaas/other",
            &pins,
            Err(Unavailable {
                why: "x".into(),
                hint: "h",
            })
        )
        .facts
        .is_none());
    }
}
