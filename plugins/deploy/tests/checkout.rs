//! Execute the real CLI and SSH script with isolated local Git repositories.
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    source: PathBuf,
    checkout: PathBuf,
    bin: PathBuf,
    config: PathBuf,
}

fn executable(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("/usr/bin/git")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
        ])
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().into()
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let source = tmp.path().join("origin");
        let bin = tmp.path().join("bin");
        let config = tmp.path().join("gitconfig");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(home.join(".local/bin")).unwrap();
        fs::create_dir_all(&bin).unwrap();
        git(&source, &["init", "-b", "main"]);
        fs::create_dir_all(source.join("scripts")).unwrap();
        executable(
            &source.join("scripts/install-cli.sh"),
            r#"#!/bin/sh
set -e
[ "$VEGA_INSTALL_BY" = deploy ]
echo install >> "$HOME/actions"
cp "$FIXTURE_BIN/vega" "$HOME/.local/bin/vega"
"#,
        );
        fs::write(source.join("content"), "main").unwrap();
        git(&source, &["add", "."]);
        git(&source, &["commit", "-m", "main"]);
        git(&source, &["checkout", "-b", "feature/test"]);
        fs::write(source.join("content"), "feature").unwrap();
        git(&source, &["commit", "-am", "feature"]);
        git(&source, &["checkout", "main"]);
        // Redirect the production clone URL to this local source; never use the network.
        fs::write(
            &config,
            format!(
                "[url \"{}\"]\n insteadOf = git@github.com:durandom/vega.git\n",
                source.display()
            ),
        )
        .unwrap();
        executable(
            &bin.join("ssh"),
            "#!/bin/sh\nexport XDG_CONFIG_HOME=\"$HOME/.config\"\nexec /bin/sh -s\n",
        );
        executable(&bin.join("hostname"), "#!/bin/sh\necho deploying-host\n");
        executable(
            &bin.join("herdr"),
            "#!/bin/sh\necho '[{\"label\":\"target\",\"target\":\"fixture\",\"enabled\":true}]'\n",
        );
        executable(
            &bin.join("vega"),
            r#"#!/bin/sh
case "$*" in
  --version) echo fixture ;;
  'config apply') echo config >> "$HOME/actions"; echo 'summary: fixture config'; exit "${CONFIG_EXIT:-0}" ;;
  'post relay status')
    if [ -f "$HOME/started" ]; then echo 'vegad: running, binary current'
    else echo 'vegad: stopped'; fi ;;
  'post relay start') echo start >> "$HOME/actions"; touch "$HOME/started" ;;
  *) exit 1 ;;
esac
"#,
        );
        let checkout = home.join("src/durandom/vega");
        Self {
            _tmp: tmp,
            home,
            source,
            checkout,
            bin,
            config,
        }
    }

    fn apply(&self, branch: Option<&str>, config_failure: bool) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_vega-deploy"));
        cmd.args(["apply", "target"]);
        if let Some(branch) = branch {
            cmd.args(["--from-branch", branch]);
        }
        cmd.env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("deployer-config"))
            .env("GIT_CONFIG_GLOBAL", &self.config)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("FIXTURE_BIN", &self.bin)
            .env("CONFIG_EXIT", if config_failure { "1" } else { "0" })
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()));
        cmd.output().unwrap()
    }

    fn success(&self, branch: Option<&str>) -> String {
        let out = self.apply(branch, false);
        assert!(
            out.status.success(),
            "stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into()
    }

    fn actions(&self) -> String {
        fs::read_to_string(self.home.join("actions")).unwrap_or_default()
    }
}

#[test]
fn fresh_clone_of_default_and_feature_branch() {
    for branch in [None, Some("feature/test")] {
        let f = Fixture::new();
        let out = f.success(branch);
        assert_eq!(
            git(&f.checkout, &["branch", "--show-current"]),
            branch.unwrap_or("main")
        );
        assert_eq!(
            git(&f.checkout, &["rev-parse", "HEAD"]),
            git(&f.source, &["rev-parse", branch.unwrap_or("main")])
        );
        assert!(out.contains(",current,0,fixture config,ok"), "{out}");
        assert_eq!(f.actions(), "install\nconfig\nstart\n");
    }
}

#[test]
fn missing_local_branch_existing_branch_and_back_to_default() {
    let f = Fixture::new();
    f.success(None);
    // An ordinary clone has no local feature branch yet.
    f.success(Some("feature/test"));
    git(&f.source, &["checkout", "feature/test"]);
    fs::write(f.source.join("content"), "updated feature").unwrap();
    git(&f.source, &["commit", "-am", "advance"]);
    f.success(Some("feature/test"));
    assert_eq!(
        fs::read_to_string(f.checkout.join("content")).unwrap(),
        "updated feature"
    );
    f.success(None);
    assert_eq!(git(&f.checkout, &["branch", "--show-current"]), "main");
    assert_eq!(
        fs::read_to_string(f.checkout.join("content")).unwrap(),
        "main"
    );
    assert_eq!(f.actions().lines().filter(|s| *s == "start").count(), 1);
}

#[test]
fn divergent_branch_fails_before_install_or_config() {
    let f = Fixture::new();
    f.success(Some("feature/test"));
    fs::write(f.checkout.join("local"), "local commit").unwrap();
    git(&f.checkout, &["add", "local"]);
    git(&f.checkout, &["commit", "-m", "local"]);
    git(&f.checkout, &["checkout", "main"]);
    let before = f.actions();
    let head = git(&f.checkout, &["rev-parse", "HEAD"]);
    // A local branch ahead of the remote must not install unpushed commits either.
    assert_eq!(f.apply(Some("feature/test"), false).status.code(), Some(1));
    assert_eq!(f.actions(), before);
    assert_eq!(git(&f.checkout, &["branch", "--show-current"]), "main");
    git(&f.source, &["checkout", "feature/test"]);
    fs::write(f.source.join("remote"), "remote commit").unwrap();
    git(&f.source, &["add", "remote"]);
    git(&f.source, &["commit", "-m", "remote"]);
    let out = f.apply(Some("feature/test"), false);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(f.actions(), before);
    assert_eq!(git(&f.checkout, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(&f.checkout, &["branch", "--show-current"]), "main");
}

#[test]
fn dirty_tracked_files_prevent_switching_to_existing_or_missing_branch() {
    for existing in [false, true] {
        let f = Fixture::new();
        f.success(None);
        if existing {
            f.success(Some("feature/test"));
            f.success(None);
        }
        // This file is identical on both branches, so Git would carry the edit
        // across checkout and run it during install without the explicit guard.
        let installer = f.checkout.join("scripts/install-cli.sh");
        let dirty = format!(
            "{}\n# local edit\n",
            fs::read_to_string(&installer).unwrap()
        );
        fs::write(&installer, &dirty).unwrap();
        let head = git(&f.checkout, &["rev-parse", "HEAD"]);
        let before = f.actions();
        let out = f.apply(Some("feature/test"), false);
        assert_eq!(out.status.code(), Some(1));
        assert_eq!(git(&f.checkout, &["branch", "--show-current"]), "main");
        assert_eq!(git(&f.checkout, &["rev-parse", "HEAD"]), head);
        assert_eq!(f.actions(), before);
        assert_eq!(fs::read_to_string(installer).unwrap(), dirty);
    }
}

#[test]
fn missing_remote_branch_fails_before_install() {
    let f = Fixture::new();
    assert_eq!(f.apply(Some("missing"), false).status.code(), Some(1));
    assert!(f.actions().is_empty());
    f.success(None);
    let before = f.actions();
    assert_eq!(f.apply(Some("missing"), false).status.code(), Some(1));
    assert_eq!(f.actions(), before);
}

#[test]
fn overrides_replace_target_and_config_failure_is_nonfatal() {
    let f = Fixture::new();
    let cfg = f.home.join(".config/vega/config");
    fs::create_dir_all(&cfg).unwrap();
    fs::write(cfg.join("stale"), "old").unwrap();
    let src = f.home.join("deployer-config/vega/config");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("instructions.md"), "deployer overrides").unwrap();
    let out = f.apply(Some("feature/test"), true);
    assert!(out.status.success());
    assert!(!cfg.join("stale").exists());
    assert_eq!(
        fs::read_to_string(cfg.join("instructions.md")).unwrap(),
        "deployer overrides"
    );
    assert_eq!(f.actions(), "install\nconfig\nstart\n");
    assert!(String::from_utf8_lossy(&out.stdout).contains(",current,1,fixture config,ok"));
}
