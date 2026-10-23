use std::process::Command;

#[test]
fn skill_stub_has_valid_frontmatter() {
    let stub = include_str!("../../skills/vega/SKILL.md");
    let fm = vega_plugin::parse_frontmatter(stub).unwrap();
    assert_eq!(fm.name, "vega");
    let findings = vega_plugin::lint_guide(stub);
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn skill_flag_is_removed_with_a_hint() {
    let out = Command::new(env!("CARGO_BIN_EXE_vega"))
        .arg("--skill")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("removed,"), "{stdout}");
    assert!(stdout.contains("`vega skills`"), "{stdout}");
}

fn plugin(dir: &std::path::Path, name: &str, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(format!("vega-{name}"));
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn vega(path: &std::path::Path, args: &[&str]) -> (Option<i32>, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_vega"))
        .args(args)
        .env("PATH", path)
        .output()
        .unwrap();
    (out.status.code(), String::from_utf8(out.stdout).unwrap())
}

#[test]
fn skill_alias_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    plugin(dir.path(), "skills", "echo \"${0##*/} ARGS $*\"; exit 3");
    let (code, out) = vega(dir.path(), &["skills", "lib", "list", "-g"]);
    assert_eq!(code, Some(3));
    assert_eq!(out, "vega-skills ARGS lib list -g\n");
    let (code, out) = vega(dir.path(), &["skill"]);
    assert_eq!(code, Some(2));
    assert!(
        out.contains("unknown_plugin,no vega-skill on PATH"),
        "{out}"
    );
}
