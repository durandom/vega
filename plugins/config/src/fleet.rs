//! Explicit network diagnostic; ordinary infra/onboard probes stay local.
use serde_json::Value;
use std::time::Duration;
use vega_herdr::{cell, Failure};
use vega_registry::View;

pub fn mismatches(machines: &[Value], view: &View) -> Result<Vec<(String, String)>, Failure> {
    let mut out = Vec::new();
    for machine in machines {
        let name = machine["name"]
            .as_str()
            .filter(|n| !n.is_empty())
            .ok_or_else(|| {
                Failure::new(
                    "hc_bad_output",
                    "hc machine entry has no name",
                    "check hc --version",
                )
            })?;
        if let Err(f) = view.check_hc_machine(name) {
            out.push((name.into(), f.hint));
        }
    }
    Ok(out)
}

pub fn check() -> Result<bool, Failure> {
    let inventory = vega_herdr::hc::hc_on(None, &["machine", "ls"], Some(Duration::from_secs(3)))?;
    let machines = inventory["machines"].as_array().ok_or_else(|| {
        Failure::new(
            "hc_bad_output",
            "hc machine list has no machines array",
            "check hc --version",
        )
    })?;
    if machines.is_empty() {
        println!("fleet: 0 hc machines configured");
        return Ok(true);
    }
    let view = vega_registry::read_blocking(Duration::from_secs(3)).map_err(|why| {
        Failure::new(
            "registry_unavailable",
            format!("cannot check hc machine names: {why}"),
            "restore registry access and rerun vega config infra --fleet",
        )
    })?;
    let gaps = mismatches(machines, &view)?;
    println!(
        "fleet: {} hc machines, {} registry host mismatches",
        machines.len(),
        gaps.len()
    );
    if !gaps.is_empty() {
        println!("machine_hosts[{}]{{machine,next}}:", gaps.len());
        for (name, hint) in &gaps {
            println!("  {},{}", cell(name), cell(hint));
        }
    }
    Ok(gaps.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn every_name_needs_its_own_registry_host_not_an_ssh_target() {
        let mut view = View::default();
        view.entries
            .insert(vega_registry::beat_key("io"), json!({"at":1}));
        view.entries
            .insert(vega_registry::beat_key("io_local"), json!({"at":1}));
        let machines = vec![
            json!({"name":"io","target":"io-wsl"}),
            json!({"name":"alias","host":"io","target":"io"}),
            json!({"name":"io.local"}),
        ];
        let gaps = mismatches(&machines, &view).unwrap();
        assert_eq!(gaps.len(), 2);
        assert_eq!(gaps[0].0, "alias");
        assert!(gaps[0].1.contains("hc machine rename alias <host>"));
        assert_eq!(gaps[1].0, "io.local");
        assert!(mismatches(&[json!({})], &view).is_err());
        assert!(mismatches(&[], &view).unwrap().is_empty());
    }
}
