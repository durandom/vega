//! Runtime dependency unit: vega-browser owns the pin and install policy.
use crate::{Ctx, Outcome, Plan};
use serde_json::Value;
use std::process::Command;

fn run(ctx: &Ctx, args: &[&str]) -> Result<String, String> {
    let out = Command::new(super::installed(&ctx.env.home, "vega-browser"))
        .args(args)
        .env("HOME", &ctx.env.home)
        .output()
        .map_err(|e| format!("vega browser: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "vega browser {}: {}{}",
            args.join(" "),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

pub fn plan(ctx: &Ctx) -> Result<Plan, String> {
    let s: Value =
        serde_json::from_str(&run(ctx, &["status", "--json"])?).map_err(|e| e.to_string())?;
    let state = s["state"]
        .as_str()
        .filter(|s| matches!(*s, "current" | "differs" | "missing"))
        .ok_or("invalid browser status")?;
    let chrome = s["chrome"]
        .as_str()
        .filter(|s| matches!(*s, "current" | "missing"))
        .ok_or("invalid Chrome status")?;
    if state == "missing" && s["npm"] == Value::Bool(false) {
        return Ok(Plan::skip(
            "no agent-browser and no npm on PATH: this host does not use browsers",
        ));
    }
    Ok(Plan {
        browser_pending: state != "current" || chrome != "current",
        notes: vec![format!("agent-browser: {state}, pinned {}; Chrome for Testing: {chrome}; apply runs vega browser install", s["pinned"].as_str().unwrap_or("unknown"))],
        ..Plan::default()
    })
}

pub fn apply(ctx: &Ctx, plan: &Plan) -> Outcome {
    if !plan.browser_pending {
        return Outcome::Unchanged;
    }
    match run(ctx, &["install"]) {
        Ok(_) => match self::plan(ctx) {
            Ok(p) if !p.browser_pending => {
                Outcome::Written(vec!["agent-browser and Chrome for Testing converged".into()])
            }
            Ok(_) => Outcome::Failed("vega browser install did not converge".into()),
            Err(e) => Outcome::Failed(e),
        },
        Err(e) => Outcome::Failed(e),
    }
}
