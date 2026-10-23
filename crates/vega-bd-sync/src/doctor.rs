//! Local checks of a `.beads` dir. A data dir copied from another machine
//! carries that machine's server config and lock files (rally-ng,
//! 2026-10-11); `vega bd init` therefore always clones fresh from the remote,
//! and `vega bd status` flags what such a copy left behind.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

/// Findings for `beads` (the `.beads` dir), one line each; empty when clean.
pub fn check(beads: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let canon = beads.canonicalize().unwrap_or_else(|_| beads.to_path_buf());
    if let Ok(cfg) = fs::read_to_string(beads.join("dolt-server-config.yaml")) {
        if let Some(dir) = cfg_dir(&cfg) {
            let d = Path::new(&dir);
            let d = d.canonicalize().unwrap_or_else(|_| d.to_path_buf());
            if !d.starts_with(&canon) {
                out.push(format!(
                    "foreign server config: dolt-server-config.yaml points at {dir}, not below {}; copied from another machine? Provision a fresh clone with vega bd init",
                    beads.display()
                ));
            }
        }
    }
    if let Some(pid) = fs::read_to_string(beads.join("dolt-server.pid"))
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
    {
        if !alive(pid) {
            out.push(format!(
                "stale server pid: dolt-server.pid names {pid}, which is not running here (a copy, or a crash); bd dolt stop, then bd dolt start"
            ));
        }
    }
    out
}

fn cfg_dir(yaml: &str) -> Option<String> {
    yaml.lines().find_map(|l| {
        l.trim()
            .strip_prefix("cfg_dir:")
            .map(|v| v.trim().trim_matches('"').trim_matches('\'').to_string())
    })
}

fn alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_a_copied_config_and_a_dead_pid() {
        let t = tempfile::tempdir().unwrap();
        let b = t.path().join(".beads");
        fs::create_dir_all(b.join("dolt/.doltcfg")).unwrap();
        fs::write(
            b.join("dolt-server-config.yaml"),
            format!(
                "listener:\n  port: 1\ncfg_dir: {}\n",
                b.join("dolt/.doltcfg").display()
            ),
        )
        .unwrap();
        fs::write(b.join("dolt-server.pid"), std::process::id().to_string()).unwrap();
        assert!(check(&b).is_empty(), "{:?}", check(&b));

        fs::write(
            b.join("dolt-server-config.yaml"),
            "cfg_dir: /Users/other/src/x/.beads/dolt/.doltcfg\n",
        )
        .unwrap();
        // A pid far above any real one.
        fs::write(b.join("dolt-server.pid"), "4000000000").unwrap();
        let f = check(&b);
        assert_eq!(f.len(), 2, "{f:?}");
        assert!(f[0].starts_with("foreign server config"));
        assert!(f[1].starts_with("stale server pid"));
    }
}
