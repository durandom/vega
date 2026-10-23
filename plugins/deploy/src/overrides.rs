//! Local config overrides (`$XDG_CONFIG_HOME/vega/config/`, docs/design/config.md)
//! travel with the deploy: the deploying host's directory replaces the
//! target's, so a file deleted here is deleted there. The files go inline in
//! the remote script, base64-encoded; no second ssh connection, no tar.

use std::fs;
use std::path::{Path, PathBuf};

/// This host's override directory.
pub fn local_dir() -> PathBuf {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        });
    config.join("vega/config")
}

/// Every regular file under `dir` (links followed), relative, sorted.
fn files(dir: &Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) -> Result<(), String> {
        let rd = fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for e in rd {
            let p = e.map_err(|e| e.to_string())?.path();
            if p.is_dir() {
                walk(base, &p, out)?;
            } else if p.is_file() {
                let rel = p
                    .strip_prefix(base)
                    .expect("under base")
                    .to_string_lossy()
                    .into_owned();
                let bytes = fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
                out.push((rel, bytes));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    if dir.is_dir() {
        walk(dir, dir, &mut out)?;
    }
    out.sort();
    Ok(out)
}

/// The shell lines that replace the target's override directory with the
/// files of `dir` (none when `dir` does not exist) and print
/// `vega-deploy:overrides=<count>`.
pub fn script(dir: &Path) -> Result<String, String> {
    let files = files(dir)?;
    let mut s = String::from(
        "cfg=\"${XDG_CONFIG_HOME:-$HOME/.config}/vega/config\"\nnew=\"$cfg.vega-new\"\nrm -rf \"$new\"\nmkdir -p \"$new\"\n",
    );
    for (rel, bytes) in &files {
        let q = quote(rel);
        if let Some((d, _)) = rel.rsplit_once('/') {
            s.push_str(&format!("mkdir -p \"$new\"/{}\n", quote(d)));
        }
        s.push_str(&format!(
            "printf '%s' '{}' | base64 -d > \"$new\"/{q}\n",
            base64(bytes)
        ));
    }
    s.push_str("rm -rf \"$cfg\"\nmv \"$new\" \"$cfg\"\n");
    s.push_str(&format!("echo \"vega-deploy:overrides={}\"", files.len()));
    Ok(s)
}

/// A single-quoted shell word.
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn base64(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn base64_matches_rfc4648() {
        for (i, o) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(i.as_bytes()), o);
        }
    }

    /// Runs the script against a temp HOME: the target's dir ends up equal to
    /// the source's, stale files gone.
    #[test]
    fn script_replaces_the_target_dir() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("src");
        fs::create_dir_all(src.join("claude/output-styles")).unwrap();
        fs::write(src.join("instructions.md"), "it's \"mine\"\n$HOME `x`\n").unwrap();
        fs::write(src.join("claude/output-styles/my style.md"), "ü\n").unwrap();
        let home = d.path().join("home");
        let cfg = home.join(".config/vega/config");
        fs::create_dir_all(cfg.join("herdr")).unwrap();
        fs::write(cfg.join("herdr/config.toml"), "stale\n").unwrap();
        let run = |s: String| {
            let o = Command::new("sh")
                .arg("-ec")
                .arg(s)
                .env("HOME", &home)
                .env_remove("XDG_CONFIG_HOME")
                .output()
                .unwrap();
            assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
            String::from_utf8_lossy(&o.stdout).into_owned()
        };
        assert_eq!(run(script(&src).unwrap()), "vega-deploy:overrides=2\n");
        assert_eq!(
            fs::read_to_string(cfg.join("instructions.md")).unwrap(),
            "it's \"mine\"\n$HOME `x`\n"
        );
        assert_eq!(
            fs::read_to_string(cfg.join("claude/output-styles/my style.md")).unwrap(),
            "ü\n"
        );
        assert!(!cfg.join("herdr").exists());
        assert!(!home.join(".config/vega/config.vega-new").exists());
        // No source dir: the target's is emptied too.
        assert_eq!(
            run(script(&d.path().join("none")).unwrap()),
            "vega-deploy:overrides=0\n"
        );
        assert_eq!(fs::read_dir(&cfg).unwrap().count(), 0);
    }
}
