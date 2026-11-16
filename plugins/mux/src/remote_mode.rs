//! Mode belongs to the pane's host, not a caller-side machine-prefixed cache.
use super::*;
use vega_herdr::pane::{parse, Backend};

pub fn run(target: &str, requested: Option<ModeArg>) -> Result<Option<Value>, Failure> {
    let address = parse(target);
    let qualified = address.as_ref().and_then(|p| p.machine.as_deref());
    if let (Some(scope), Some(host)) = (machine(), qualified) {
        if scope != host {
            return Err(Failure::new(
                "usage",
                "mode target and --machine name different hosts",
                "use one host in both the target address and --machine",
            ));
        }
    }
    let Some(host) = qualified.or(machine()) else {
        return Ok(None);
    };
    vega_herdr::hc::check_direction(host)?;
    // Strip remote qualification and keep the backend explicit. Names resolve
    // on the target; do not consult stale local spawn records for their identity.
    let local = address.as_ref().map_or_else(
        || target.to_string(),
        |p| {
            format!(
                "{}:{}",
                if p.backend == Backend::Hc {
                    "hc"
                } else {
                    "herdr"
                },
                p.pane
            )
        },
    );
    let wanted = requested.map(|m| match m {
        ModeArg::Headed => "headed",
        ModeArg::Headless => "headless",
    });
    let mut args = vec![
        "exec",
        "--timeout",
        "15",
        "--",
        "vega-mux",
        "mode",
        &local,
        "--json",
    ];
    if let Some(mode) = wanted {
        args.push(mode);
    }
    let fail = |message: String| {
        Failure::new(
        "remote_mode_failed",
        format!("cannot confirm mode on {host}: {message}"),
        "check the host connection and matching vega versions; run `vega mux mode <pane>` on the pane host before retrying",
    )
    };
    let result = execute(host, &args, &fail)?;
    if result["schema"] != 1
        || result["output"].as_str().is_none_or(str::is_empty)
        || result["mode"].as_str().and_then(Mode::parse).is_none()
        || wanted.is_some_and(|m| result["mode"] != m)
    {
        return Err(fail("incompatible or unconfirmed mode reply".into()));
    }
    Ok(Some(result))
}

pub use vega_herdr::remote_mode::execute;
