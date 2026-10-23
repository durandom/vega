//! Where other hosts' files are mounted here, for requests from remote
//! herdr panes, and which host a remote pane's requests go to.
//!
//! Both live in `$XDG_CONFIG_HOME/vega/` (default `~/.config/vega/`):
//!
//! - `open-mounts` on the user's machine, one mount per line:
//!   `<host>:<root on that host> <local mount>`, e.g.
//!   `deimos:/Users/mhild /Volumes/mhild`. `#` starts a comment.
//! - `open-target` on a remote host: the host the user sits at, e.g.
//!   `proteus`. With it, the menu sends its pick there instead of running it.

use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    pub host: String,
    pub remote: PathBuf,
    pub local: PathBuf,
}

pub fn config_dir() -> Option<PathBuf> {
    env::var_os("XDG_CONFIG_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|h| Path::new(&h).join(".config")))
        .map(|c| c.join("vega"))
}

/// The host from `open-target`, if this machine has one.
pub fn open_target() -> Option<String> {
    let text = fs::read_to_string(config_dir()?.join("open-target")).ok()?;
    let host = text.lines().next()?.trim();
    (!host.is_empty()).then(|| host.to_string())
}

pub fn load() -> Result<Vec<Mount>, String> {
    let Some(file) = config_dir().map(|d| d.join("open-mounts")) else {
        return Err("no config dir (HOME unset)".into());
    };
    let text = fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    parse(&text)
}

pub fn parse(text: &str) -> Result<Vec<Mount>, String> {
    let mut v = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let bad = || {
            format!(
                "open-mounts line {}: want `<host>:<root> <local mount>`",
                i + 1
            )
        };
        let (spec, local) = line.split_once(char::is_whitespace).ok_or_else(bad)?;
        let (host, remote) = spec.split_once(':').ok_or_else(bad)?;
        let (remote, local) = (PathBuf::from(remote), PathBuf::from(local.trim()));
        if host.is_empty() || !remote.is_absolute() || !local.is_absolute() {
            return Err(bad());
        }
        v.push(Mount {
            host: host.into(),
            remote,
            local,
        });
    }
    Ok(v)
}

/// Map `path` on `host` to an existing path here that stays inside the
/// mount, symlinks resolved. Paths with `..` are refused outright.
pub fn map(mounts: &[Mount], host: &str, path: &Path) -> Result<PathBuf, String> {
    if path.components().any(|c| c == Component::ParentDir) {
        return Err(format!("{} contains ..", path.display()));
    }
    let m = mounts
        .iter()
        .filter(|m| m.host == host)
        .filter(|m| path.starts_with(&m.remote))
        // The longest root wins when mounts nest.
        .max_by_key(|m| m.remote.components().count())
        .ok_or_else(|| format!("{host}:{} is under no mount", path.display()))?;
    let rest = path
        .strip_prefix(&m.remote)
        .expect("checked by starts_with");
    let local = m.local.join(rest);
    let root = m
        .local
        .canonicalize()
        .map_err(|e| format!("mount {} unavailable: {e}", m.local.display()))?;
    let real = local
        .canonicalize()
        .map_err(|e| format!("{}: {e}", local.display()))?;
    if !real.starts_with(&root) {
        return Err(format!(
            "{} leaves the mount {}",
            local.display(),
            root.display()
        ));
    }
    refuse_executable(&real)?;
    Ok(real)
}

/// Bundle extensions that macOS launches when opened.
const LAUNCHABLE_DIRS: &[&str] = &["app", "bundle", "framework", "plugin", "prefPane", "saver"];

/// A remote request may show files, never start programs: no file with an
/// exec bit, no app bundle.
fn refuse_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let meta = fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let bundle = meta.is_dir()
        && path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| LAUNCHABLE_DIRS.iter().any(|b| b.eq_ignore_ascii_case(e)));
    if bundle || (meta.is_file() && meta.permissions().mode() & 0o111 != 0) {
        return Err(format!("{} is executable", path.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mount(host: &str, remote: &str, local: &Path) -> Mount {
        Mount {
            host: host.into(),
            remote: remote.into(),
            local: local.into(),
        }
    }

    #[test]
    fn parses_lines_and_skips_comments() {
        let m = parse("# mounts\ndeimos:/Users/mhild   /Volumes/mhild  # smb\n\n").unwrap();
        assert_eq!(
            m,
            vec![Mount {
                host: "deimos".into(),
                remote: "/Users/mhild".into(),
                local: "/Volumes/mhild".into(),
            }]
        );
        for bad in [
            "deimos /Volumes/x",
            "deimos:rel /Volumes/x",
            ":/a /b",
            "deimos:/a",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn maps_into_the_mount_by_host_and_longest_root() {
        let dir = tempfile::tempdir().unwrap();
        let share = dir.path().join("share");
        fs::create_dir_all(share.join("src/x")).unwrap();
        fs::write(share.join("src/x/a.md"), "").unwrap();
        let other = dir.path().join("other");
        fs::create_dir_all(&other).unwrap();
        let mounts = [
            mount("deimos", "/Users/mhild", &share),
            mount("deimos", "/Users/mhild/src/x/deep", &other),
            mount("ganymede", "/Users/mhild", &other),
        ];
        let got = map(&mounts, "deimos", Path::new("/Users/mhild/src/x/a.md")).unwrap();
        assert_eq!(got, share.join("src/x/a.md").canonicalize().unwrap());
        assert!(map(
            &mounts,
            "deimos",
            Path::new("/Users/mhild/src/x/missing.md")
        )
        .is_err());
        assert!(map(&mounts, "titan", Path::new("/Users/mhild/src/x/a.md")).is_err());
        assert!(map(&mounts, "deimos", Path::new("/etc/passwd")).is_err());
        assert!(map(
            &mounts,
            "deimos",
            Path::new("/Users/mhild/../../etc/passwd")
        )
        .is_err());
        assert!(map(&mounts, "deimos", Path::new("/Users/mhildx/a")).is_err());
    }

    #[test]
    fn refuses_executables_and_app_bundles() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let share = dir.path().join("share");
        fs::create_dir_all(share.join("Foo.app")).unwrap();
        fs::create_dir_all(share.join("docs")).unwrap();
        fs::write(share.join("run.sh"), "").unwrap();
        fs::set_permissions(share.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        let mounts = [mount("deimos", "/Users/mhild", &share)];
        for p in ["/Users/mhild/run.sh", "/Users/mhild/Foo.app"] {
            let err = map(&mounts, "deimos", Path::new(p)).unwrap_err();
            assert!(err.contains("is executable"), "{err}");
        }
        assert!(map(&mounts, "deimos", Path::new("/Users/mhild/docs")).is_ok());
    }

    #[test]
    fn refuses_symlinks_that_leave_the_mount() {
        let dir = tempfile::tempdir().unwrap();
        let share = dir.path().join("share");
        fs::create_dir_all(&share).unwrap();
        fs::write(dir.path().join("secret"), "").unwrap();
        std::os::unix::fs::symlink(dir.path().join("secret"), share.join("link")).unwrap();
        let mounts = [mount("deimos", "/Users/mhild", &share)];
        let err = map(&mounts, "deimos", Path::new("/Users/mhild/link")).unwrap_err();
        assert!(err.contains("leaves the mount"), "{err}");
    }
}
