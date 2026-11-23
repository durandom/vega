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
            r#"test "$1" = --version && echo "agent-browser $(cat "$HOME/version")""#,
        );
        script(
            &f.p("bin/curl"),
            r#"
for arg do
  case "$arg" in
    https://*) printf '%s\n' "$arg" >> "$HOME/chrome.log";;
  esac
done
test ! -f "$HOME/fail-chrome"
"#,
        );
        script(
            &f.p("bin/unzip"),
            r#"
test ! -f "$HOME/incomplete-chrome" || exit 0
case "$(uname -s):$(uname -m)" in
  Darwin:arm64) platform=mac-arm64;;
  Darwin:x86_64) platform=mac-x64;;
  Linux:aarch64) platform=linux-arm64;;
  Linux:x86_64) platform=linux64;;
  *) exit 9;;
esac
case "$platform" in
  mac-*) executable='Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing';;
  *) executable=chrome;;
esac
binary="$4/chrome-$platform/$executable"
mkdir -p "$(dirname "$binary")"
printf '#!/bin/sh\nexit 9\n' > "$binary"
chmod +x "$binary"
"#,
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
fn assert_pinned_download(f: &Fix) {
    let downloads = fs::read_to_string(f.p("chrome.log")).unwrap();
    assert_eq!(downloads.lines().count(), 1);
    assert!(downloads.starts_with(&format!(
        "https://storage.googleapis.com/chrome-for-testing-public/{}/",
        vega_browser::chrome::VERSION
    )));
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
        assert_pinned_download(&f);
    }
}

#[test]
fn mise_current_converges_chrome_and_mismatch_suggests_mise_command() {
    let pin = vega_browser::install::VERSION.trim();
    for (version, through_shim) in [(pin, true), ("0.31.1", true), ("0.31.1", false)] {
        let f = Fix::new(None);
        let root = f.p("mise/installs/npm-agent-browser/test");
        let target = root.join("node_modules/.bin/agent-browser");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::copy(f.p("template"), &target).unwrap();
        fs::write(f.p("version"), version).unwrap();
        let source = f.p("config with spaces/agent's tools.toml");
        let listing = serde_json::json!([{
            "active": true,
            "install_path": root,
            "source": {"path": source}
        }]);
        fs::write(f.p("mise-list.json"), listing.to_string()).unwrap();
        script(
            &f.p("bin/mise"),
            r#"
case "${0##*/}:$*" in
  agent-browser:--version) exec "$HOME/mise/installs/npm-agent-browser/test/node_modules/.bin/agent-browser" "$@";;
  'mise:which agent-browser') echo "$HOME/mise/installs/npm-agent-browser/test/node_modules/.bin/agent-browser";;
  'mise:ls --current --json npm:agent-browser') cat "$HOME/mise-list.json";;
  *) echo "$*" >> "$HOME/unexpected-mise.log"; exit 9;;
esac"#,
        );
        let browser_dir = if through_shim {
            let shims = f.p("mise/shims");
            fs::create_dir_all(&shims).unwrap();
            symlink(f.p("bin/mise"), shims.join("agent-browser")).unwrap();
            shims
        } else {
            target.parent().unwrap().to_owned()
        };
        let o = f
            .cmd()
            .env(
                "PATH",
                format!(
                    "{}:{}:/usr/bin:/bin",
                    browser_dir.display(),
                    f.p("bin").display()
                ),
            )
            .arg("install")
            .output()
            .unwrap();
        if version == pin {
            assert!(String::from_utf8_lossy(&o.stdout).contains("result: current"));
            ok(o);
            assert_pinned_download(&f);
            assert!(vega_browser::chrome::installed(f.tmp.path()));
        } else {
            assert_eq!(o.status.code(), Some(1));
            let command = format!(
                "mise use --path '{}/config with spaces/agent'\\''s tools.toml' --pin npm:agent-browser@{pin}",
                f.tmp.path().display()
            );
            assert!(String::from_utf8_lossy(&o.stdout).contains(&command));
            assert!(!f.p("chrome.log").exists());
        }
        assert!(!f.p("npm.log").exists());
        assert!(!f.p("unexpected-mise.log").exists());
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
    let pinned = vega_browser::chrome::binary(f.tmp.path());
    script(&pinned, "exit 9");
    script(&f.p("vega/shims/bin/agent-browser"), "exit 99");
    let path = format!(
        "{}:{}:{}:/usr/bin:/bin",
        f.p("vega/shims/bin").display(),
        f.p("bin").display(),
        f.p("prefix/bin").display()
    );
    ok(f.cmd().env("PATH", path).arg("install").output().unwrap());
    assert!(!f.p("chrome.log").exists());
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

#[test]
fn old_chrome_survives_install_and_an_incomplete_download_can_be_retried() {
    let f = Fix::new(Some("0.43.0"));
    let legacy = f.p(".agent-browser/browsers/chrome-146.0.0.0");
    let legacy = if cfg!(target_os = "macos") {
        legacy.join("Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing")
    } else {
        legacy.join("chrome")
    };
    script(&legacy, "exit 9");
    let status = || -> serde_json::Value {
        serde_json::from_slice(&f.run(&["status", "--json"]).stdout).unwrap()
    };
    assert_eq!(status()["chrome"], "missing");
    fs::write(f.p("incomplete-chrome"), "").unwrap();
    assert!(!f.run(&["install"]).status.success());
    assert_eq!(status()["chrome"], "missing");
    fs::remove_file(f.p("incomplete-chrome")).unwrap();
    ok(f.run(&["install"]));
    assert_eq!(status()["chrome"], "current");
    assert!(vega_browser::chrome::installed(f.tmp.path()));
    assert_eq!(fs::read_to_string(legacy).unwrap(), "#!/bin/sh\nexit 9\n");
    assert!(fs::read_dir(f.p(".agent-browser/browsers"))
        .unwrap()
        .all(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("chrome-")
        }));
}
