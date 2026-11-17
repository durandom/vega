use std::{
    fs,
    os::unix::fs::{symlink, PermissionsExt},
    path::Path,
    process::{Command, Output},
};
const BIN: &str = env!("CARGO_BIN_EXE_vega-browser");
fn script(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}
struct Fix {
    tmp: tempfile::TempDir,
}
impl Fix {
    fn new(version: Option<&str>) -> Self {
        let f = Self {
            tmp: tempfile::tempdir().unwrap(),
        };
        fs::create_dir_all(f.p("prefix/bin")).unwrap();
        script(
            &f.p("bin/npm"),
            r#"
echo "$*" >> "$HOME/npm.log"
case "$1" in
root) echo "$HOME/prefix/lib/node_modules";;
prefix) echo "$HOME/prefix";;
install)
  test ! -f "$HOME/fail-npm" || exit 5
  mkdir -p "$HOME/prefix/lib/node_modules/agent-browser/bin"
  cp "$HOME/template" "$HOME/prefix/lib/node_modules/agent-browser/bin/agent-browser.js"
  ln -sf ../lib/node_modules/agent-browser/bin/agent-browser.js "$HOME/prefix/bin/agent-browser"
  echo 0.43.0 > "$HOME/version";;
esac"#,
        );
        script(
            &f.p("template"),
            r#"
case "$1" in
--version) echo "agent-browser $(cat "$HOME/version")";;
install)
 echo install >> "$HOME/chrome.log"
 test ! -f "$HOME/fail-chrome" || exit 6
 mkdir -p "$HOME/.agent-browser/browsers/chrome-test/Google Chrome for Testing.app/Contents/MacOS"
 touch "$HOME/.agent-browser/browsers/chrome-test/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing"
 touch "$HOME/.agent-browser/browsers/chrome-test/chrome";;
*) exit 9;;
esac"#,
        );
        if let Some(v) = version {
            let real = f.p("prefix/lib/node_modules/agent-browser/bin/agent-browser.js");
            fs::create_dir_all(real.parent().unwrap()).unwrap();
            fs::copy(f.p("template"), &real).unwrap();
            symlink(
                "../lib/node_modules/agent-browser/bin/agent-browser.js",
                f.p("prefix/bin/agent-browser"),
            )
            .unwrap();
            fs::write(f.p("version"), v).unwrap();
        }
        f
    }
    fn p(&self, p: &str) -> std::path::PathBuf {
        self.tmp.path().join(p)
    }
    fn cmd(&self) -> Command {
        let mut c = Command::new(BIN);
        c.env_clear().env("HOME", self.tmp.path()).env(
            "PATH",
            format!(
                "{}:{}:/usr/bin:/bin",
                self.p("bin").display(),
                self.p("prefix/bin").display()
            ),
        );
        c
    }
    fn run(&self, args: &[&str]) -> Output {
        self.cmd().args(args).output().unwrap()
    }
}
fn ok(o: Output) {
    assert!(
        o.status.success(),
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
}
#[test]
fn missing_upgrade_and_current_converge_without_reinstalling() {
    for initial in [None, Some("0.31.1"), Some("0.43.0")] {
        let f = Fix::new(initial);
        let s = f.run(&["status", "--json"]);
        let v: serde_json::Value = serde_json::from_slice(&s.stdout).unwrap();
        assert_eq!(
            v["state"],
            match initial {
                None => "missing",
                Some("0.43.0") => "current",
                _ => "differs",
            }
        );
        assert!(!f.p("npm.log").exists());
        ok(f.run(&["install"]));
        let npm = fs::read_to_string(f.p("npm.log")).unwrap_or_default();
        if initial != Some("0.43.0") {
            assert!(npm.contains("agent-browser@0.43.0"));
        }
        ok(f.run(&["install"]));
        assert_eq!(fs::read_to_string(f.p("npm.log")).unwrap_or_default(), npm);
        assert_eq!(fs::read_to_string(f.p("chrome.log")).unwrap(), "install\n");
    }
}
#[test]
fn foreign_override_and_wrong_prefix_refuse_before_mutating() {
    let f = Fix::new(Some("0.31.1"));
    script(&f.p("foreign"), "echo 'agent-browser 0.31.1'");
    let o = f
        .cmd()
        .env("VEGA_AGENT_BROWSER_BIN", f.p("foreign"))
        .arg("install")
        .output()
        .unwrap();
    assert!(!o.status.success());
    assert!(!fs::read_to_string(f.p("npm.log"))
        .unwrap()
        .contains("install"));
    assert!(!f.p("chrome.log").exists());
}
#[test]
fn dependency_failure_propagates_and_unknown_args_do_not_mutate() {
    for fail in ["fail-npm", "fail-chrome"] {
        let f = Fix::new(None);
        fs::write(f.p(fail), "").unwrap();
        assert!(!f.run(&["install"]).status.success());
    }
    let f = Fix::new(None);
    assert_eq!(f.run(&["install", "--force"]).status.code(), Some(2));
    assert!(!f.p("npm.log").exists());
}

#[test]
fn shim_is_bypassed_and_existing_chrome_is_not_reinstalled_on_upgrade() {
    let f = Fix::new(Some("0.31.1"));
    ok(f.cmd()
        .env("VEGA_AGENT_BROWSER_BIN", f.p("prefix/bin/agent-browser"))
        .args(["status"])
        .output()
        .unwrap());
    // Seed only Chrome, with the fake binary's installer.
    ok(Command::new(f.p("prefix/bin/agent-browser"))
        .env("HOME", f.tmp.path())
        .arg("install")
        .output()
        .unwrap());
    script(&f.p("vega/shims/bin/agent-browser"), "exit 99");
    let path = format!(
        "{}:{}:{}:/usr/bin:/bin",
        f.p("vega/shims/bin").display(),
        f.p("bin").display(),
        f.p("prefix/bin").display()
    );
    ok(f.cmd().env("PATH", path).arg("install").output().unwrap());
    assert_eq!(fs::read_to_string(f.p("chrome.log")).unwrap(), "install\n");
}

#[test]
fn false_success_does_not_claim_convergence() {
    let f = Fix::new(Some("0.31.1"));
    script(
        &f.p("bin/npm"),
        r#"
case "$1" in
root) echo "$HOME/prefix/lib/node_modules";;
prefix) echo "$HOME/prefix";;
install) exit 0;;
esac"#,
    );
    assert!(!f.run(&["install"]).status.success());
    assert!(!f.p("chrome.log").exists());
}
