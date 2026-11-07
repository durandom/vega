//! Execute the real CLI and SSH script with isolated local Git repositories.
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

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

/// Fake `ssh`, `hostname`, `herdr` and `vega`, shared by all fixtures: they keep
/// their state under `$HOME`. Written once and only when changed, because the
/// first exec of a new script costs up to 0.1 s on macOS.
fn fixture_bin() -> &'static Path {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let bin = Path::new(env!("CARGO_TARGET_TMPDIR")).join("deploy-checkout-bin");
        fs::create_dir_all(&bin).unwrap();
        for (name, text) in [
            ("hc", r#"#!/bin/sh
read -r request
printf '{"jsonrpc":"2.0","id":1,"result":'
if [ -f "$HOME/machines.json" ]; then cat "$HOME/machines.json"; else printf '{"machines":[]}'; fi
printf '}'
"#),
            (
                "ssh",
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$HOME/ssh-calls\"\nexport XDG_CONFIG_HOME=\"$HOME/.config\"\nexec /bin/sh -s\n",
            ),
            ("hostname", "#!/bin/sh\necho deploying-host\n"),
            (
                "herdr",
                "#!/bin/sh\necho '[{\"label\":\"target\",\"target\":\"fixture\",\"enabled\":true}]'\n",
            ),
            (
                "vega",
                r#"#!/bin/sh
case "$*" in
  --version) echo fixture ;;
  'config apply') echo config >> "$HOME/actions"; echo 'evidence[1]{unit,event}:'; echo '  claude.settings,"deploy unchanged"'; echo 'summary: fixture config'; exit "${CONFIG_EXIT:-0}" ;;
  'post relay status')
    if [ -f "$HOME/started" ]; then echo 'vegad: running, binary current'
    else echo 'vegad: stopped'; fi ;;
  'post relay start') echo start >> "$HOME/actions"; touch "$HOME/started" ;;
  *) exit 1 ;;
esac
"#,
            ),
        ] {
            let path = bin.join(name);
            if fs::read_to_string(&path).ok().as_deref() != Some(text) {
                executable(&path, text);
            }
        }
        bin
    })
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
        let config = tmp.path().join("gitconfig");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(home.join(".local/bin")).unwrap();
        git(&source, &["init", "-b", "main"]);
        fs::create_dir_all(source.join("scripts")).unwrap();
        executable(
            &source.join("scripts/install-cli.sh"),
            r#"#!/bin/sh
set -e
[ "$VEGA_INSTALL_BY" = deploy ]
echo install >> "$HOME/actions"
echo 'evidence[1]{unit,event}:'
echo '  claude.hooks,"install /hooks/PreToolUse added"'
echo 'summary: fixture hooks'
ln -sf "$FIXTURE_BIN/vega" "$HOME/.local/bin/vega"
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
        // A configured identity keeps Git from guessing one for reflog entries: on
        // macOS that guess resolves the hostname over mDNS, which stalls 5 s when
        // several clones run at once.
        fs::write(
            &config,
            format!(
                "[url \"{}\"]\n insteadOf = git@github.com:durandom/vega.git\n\
                 [user]\n name = Fixture\n email = fixture@example.test\n",
                source.display()
            ),
        )
        .unwrap();
        let checkout = home.join("src/durandom/vega");
        Self {
            _tmp: tmp,
            home,
            source,
            checkout,
            bin: fixture_bin().to_path_buf(),
            config,
        }
    }

    fn apply(&self, branch: Option<&str>, config_failure: bool) -> Output {
        let mut cmd = self.command();
        cmd.args(["apply", "target"]);
        if let Some(branch) = branch {
            cmd.args(["--from-branch", branch]);
        }
        cmd.env("CONFIG_EXIT", if config_failure { "1" } else { "0" });
        cmd.output().unwrap()
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_vega-deploy"));
        // A cwd outside any Git repository keeps history enrichment from probing
        // the checkout the tests run in.
        cmd.current_dir(&self.home)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("deployer-config"))
            .env("XDG_STATE_HOME", self.home.join("state"))
            .env("GIT_CONFIG_GLOBAL", &self.config)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("FIXTURE_BIN", &self.bin)
            .env("CONFIG_EXIT", "0")
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()));
        cmd
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
    // Every deploy, clone or fetch, reports evidence from both config phases.
    assert_config_evidence(&f.success(None));
    // An ordinary clone has no local feature branch yet.
    assert_config_evidence(&f.success(Some("feature/test")));
    git(&f.source, &["checkout", "feature/test"]);
    fs::write(f.source.join("content"), "updated feature").unwrap();
    git(&f.source, &["commit", "-am", "advance"]);
    assert_config_evidence(&f.success(Some("feature/test")));
    assert_eq!(
        fs::read_to_string(f.checkout.join("content")).unwrap(),
        "updated feature"
    );
    assert_config_evidence(&f.success(None));
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
    // The failed checkout is recorded without an invented commit or units.
    let event = history(&f);
    assert_eq!(event["data"]["success"], false);
    let machine = &event["data"]["machines"][0];
    assert_eq!(machine["success"], false);
    assert!(machine["commit"].is_null());
    assert!(machine["install_success"].is_null());
    assert!(machine["error"].as_str().is_some_and(|s| !s.is_empty()));
    assert!(machine["units"].as_array().unwrap().is_empty());
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

    // History records the real remote result, including the nonfatal unit failure.
    let event = history(&f);
    assert_eq!(event["kind"], "deploy");
    assert_eq!(event["data"]["success"], true);
    assert_eq!(event["data"]["branch"], "feature/test");
    let machine = &event["data"]["machines"][0];
    assert_eq!(machine["machine"], "target");
    assert_eq!(
        machine["commit"],
        git(&f.source, &["rev-parse", "feature/test"])
    );
    assert_eq!(machine["success"], true);
    assert_eq!(machine["install_success"], true);
    assert_eq!(machine["config_success"], false);
    assert_eq!(machine["relay"], "current");
    assert_eq!(machine["units"].as_array().unwrap().len(), 2);
    assert!(machine["units"][0]
        .as_str()
        .unwrap()
        .contains("claude.hooks"));
    assert!(machine["duration_ms"].is_number());
    assert!(
        event["data"]["duration_ms"].as_u64().unwrap() >= machine["duration_ms"].as_u64().unwrap()
    );
}

/// Both config phases (install-cli and `vega config apply`) report evidence.
fn assert_config_evidence(out: &str) {
    assert!(
        out.contains("config_evidence[2]{name,unit,event}:"),
        "{out}"
    );
    for expected in [
        "install",
        "deploy",
        "claude.hooks",
        "claude.settings",
        "/hooks/PreToolUse",
        "added",
    ] {
        assert!(out.contains(expected), "{out}");
    }
}

fn history(f: &Fixture) -> serde_json::Value {
    let root = f.home.join("state/vega/history-outbox");
    let mut events = Vec::new();
    for host in fs::read_dir(root).unwrap() {
        for item in fs::read_dir(host.unwrap().path()).unwrap() {
            let path = item.unwrap().path();
            if path.extension().is_some_and(|s| s == "json") {
                events.push(
                    serde_json::from_slice::<serde_json::Value>(&fs::read(path).unwrap()).unwrap(),
                );
            }
        }
    }
    assert_eq!(events.len(), 1);
    events.remove(0)
}

#[test]
fn unavailable_outbox_does_not_change_deploy_success_or_failure() {
    let f = Fixture::new();
    fs::write(f.home.join("state"), "not a directory").unwrap();
    for branch in ["missing", "main"] {
        let out = f.apply(Some(branch), false);
        assert_eq!(out.status.success(), branch == "main");
        assert!(String::from_utf8_lossy(&out.stderr).contains("history: event not queued:"));
    }
}

fn mixed_fleet(f: &Fixture) {
    fs::write(
        f.home.join("machines.json"),
        r#"{"machines":[
        {"name":"target","host":"target","id":"id1","version":"0.1.18","state":"connected","target":"fixture","via":false},
        {"name":"provider","host":"provider","id":"id2","version":"0.1.18","state":"connected","target":"do-not-ssh","via":true}
    ]}"#,
    )
    .unwrap();
}

#[test]
fn via_machine_does_not_block_status_or_selected_ssh_deployment() {
    let f = Fixture::new();
    mixed_fleet(&f);
    let out = f.command().output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("provider,unsupported,-,-,via transport (SSH required)"),
        "{text}"
    );
    assert!(text.contains("target,"));
    f.success(None);
    assert_eq!(f.actions(), "install\nconfig\nstart\n");
    let calls = fs::read_to_string(f.home.join("ssh-calls")).unwrap();
    assert!(calls.contains("fixture"));
    assert!(!calls.contains("do-not-ssh"));
}

#[test]
fn apply_all_skips_via_and_only_via_is_an_explicit_noop() {
    let f = Fixture::new();
    mixed_fleet(&f);
    let out = f.command().args(["apply", "--all"]).output().unwrap();
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("deployed[1]"));
    assert!(String::from_utf8_lossy(&out.stderr).contains("skipping provider"));
    assert_eq!(f.actions(), "install\nconfig\nstart\n");
    let calls = fs::read_to_string(f.home.join("ssh-calls")).unwrap();
    assert!(!calls.contains("do-not-ssh"));
    let out = f.command().args(["apply", "provider"]).output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout)
        .contains("deployed: 0 SSH machines selected (1 skipped)"));
    assert!(String::from_utf8_lossy(&out.stderr).contains("skipping provider"));
    assert_eq!(fs::read_to_string(f.home.join("ssh-calls")).unwrap(), calls);
}

#[test]
fn corp_deploy_refuses_private_ssh_target_before_remote_actions() {
    let f = Fixture::new();
    mixed_fleet(&f);
    let out = f
        .command()
        .env("VEGA_POST_HOST", "deimos")
        .env("VEGA_POST_INBOX_ONLY", "deimos")
        .args(["apply", "target"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("machine_direction"));
    assert!(f.actions().is_empty());
    assert!(!f.home.join("ssh-calls").exists());
}
