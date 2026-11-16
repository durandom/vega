use std::{fs, os::unix::fs::PermissionsExt, process::Command};

#[test]
fn fake_ssh_checks_configured_hosts_and_never_leaks_output() {
    let home = tempfile::tempdir().unwrap();
    let ssh_dir = home.path().join(".ssh");
    let bin = home.path().join("bin");
    fs::create_dir(&ssh_dir).unwrap();
    fs::create_dir(&bin).unwrap();
    fs::write(
        ssh_dir.join("config"),
        "Host *\n  UseKeychain yes\nInclude hosts\n",
    )
    .unwrap();
    fs::write(
        ssh_dir.join("hosts"),
        "Host github.com\nHost corp\n  HostName=gitlab.cee.redhat.com\n",
    )
    .unwrap();
    let ssh = bin.join("ssh");
    fs::write(&ssh, r#"#!/bin/sh
printf '%s\n' "$@" >> "$HOME/argv"
case "$*" in
  *BatchMode=yes*StrictHostKeyChecking=yes*UpdateHostKeys=no*ControlPath=none*PermitLocalCommand=no*RemoteCommand=none*) ;;
  *) exit 99;;
esac
case "$*" in
  *github.com) echo 'Hi private-user! You have successfully authenticated, but GitHub does not provide shell access.' >&2; exit "${GITHUB_EXIT:-1}";;
  *corp) echo 'Welcome to GitLab, private-user!'; exit "${GITLAB_EXIT:-0}";;
esac
"#).unwrap();
    fs::set_permissions(&ssh, fs::Permissions::from_mode(0o700)).unwrap();
    let run = |github: &str, gitlab: &str| {
        Command::new("python3")
            .env_clear()
            .env("HOME", home.path())
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .env("GITHUB_EXIT", github)
            .env("GITLAB_EXIT", gitlab)
            .args(["-c", vega_config::ssh_auth::SCRIPT])
            .output()
            .unwrap()
    };
    let out = run("1", "0");
    assert!(out.status.success());
    let rows = vega_config::ssh_auth::parse(&out.stdout).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.ok));
    assert!(!String::from_utf8_lossy(&out.stdout).contains("private-user"));
    let rows = vega_config::ssh_auth::parse(&run("255", "255").stdout).unwrap();
    assert!(
        rows.iter().all(|r| !r.ok),
        "greeting cannot override transport failure"
    );
    fs::write(
        ssh_dir.join("config"),
        "Host *\n IdentityAgent ~/.ssh/agent.sock\n",
    )
    .unwrap();
    assert!(vega_config::ssh_auth::parse(&run("1", "0").stdout)
        .unwrap()
        .is_empty());
    fs::write(ssh_dir.join("config"), "Include config\nHost github.com\n").unwrap();
    fs::write(&ssh, "#!/bin/sh\nexec /bin/sleep 10\n").unwrap();
    let timed = vega_config::ssh_auth::parse(&run("1", "0").stdout).unwrap();
    assert_eq!(timed[0].state, "fail: SSH timeout");
    fs::remove_file(&ssh).unwrap();
    let missing = Command::new("python3")
        .env_clear()
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
        .args([
            "-c",
            &format!(
                "import os; os.environ['PATH']=os.environ['HOME']+'/bin'\n{}",
                vega_config::ssh_auth::SCRIPT
            ),
        ])
        .output()
        .unwrap();
    let rows = vega_config::ssh_auth::parse(&missing.stdout).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].state, "fail: ssh unavailable");
}

#[test]
fn infra_exits_one_for_auth_failure_with_a_fix_hint() {
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join("bin");
    fs::create_dir(&bin).unwrap();
    fs::create_dir(home.path().join(".ssh")).unwrap();
    fs::write(home.path().join(".ssh/config"), "Host github.com\n").unwrap();
    fs::write(bin.join("ssh"), "#!/bin/sh\nexit 255\n").unwrap();
    fs::set_permissions(bin.join("ssh"), fs::Permissions::from_mode(0o700)).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_vega-config"))
        .env_clear()
        .env("HOME", home.path())
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .arg("infra")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("github.com") && text.contains("Terminal.app") && text.contains("Background"),
        "{text}"
    );
}
