//! `vega bd resume --project` on a temp HOME with temp git repos: every
//! form of a key resolves to the project `vega bd status` lists.

use std::path::Path;
use std::process::{Command, Output};

fn git(dir: &Path, args: &[&str]) {
    let o = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
}

fn register(home: &Path, key: &str, dir: &Path, database: &str) {
    let common = if dir.join(".git").is_dir() {
        dir.join(".git")
    } else {
        dir.parent().unwrap().join(".bare")
    };
    let common = common.canonicalize().unwrap();
    let encode = vega_herdr::project::encode_component;
    let projects = home.join("state/vega/bd/projects/v2").join(encode(key));
    std::fs::create_dir_all(&projects).unwrap();
    let p = serde_json::json!({
        "key": key, "common": common, "dir": dir, "database": database,
        "remote": format!("http://127.0.0.1:1/{database}"), "bd": "/nonexistent/bd",
        "data": dir.join(".beads/dolt"), "since": 1, "resumed": 0,
    });
    let record = serde_json::json!({"schema": 2, "project": key, "common_dir": common, "value": p});
    std::fs::write(
        projects.join(format!("{}.json", encode(common.to_str().unwrap()))),
        record.to_string(),
    )
    .unwrap();
}

fn resume(home: &Path, project: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_vega-bd"))
        .args(["resume", "--project", project])
        .current_dir(home)
        .env("HOME", home)
        .env("XDG_STATE_HOME", home.join("state"))
        .env_remove("XDG_CONFIG_HOME")
        .output()
        .unwrap()
}

#[test]
fn resume_accepts_every_key_form() {
    let tmp = tempfile::tempdir().unwrap();
    // Canonical: macOS temp dirs sit behind the /var symlink.
    let home = std::fs::canonicalize(tmp.path()).unwrap();
    let h = home.display().to_string();
    let vega = home.join("src/vega");
    std::fs::create_dir_all(vega.join("sub")).unwrap();
    git(&vega, &["init", "-q"]);
    let sk = home.join("src/sk");
    std::fs::create_dir_all(&sk).unwrap();
    git(&sk, &["init", "-q", "--bare", ".bare"]);
    git(
        &vega,
        &["remote", "add", "origin", "git@github.com:team/vega.git"],
    );
    git(
        &sk.join(".bare"),
        &["remote", "add", "origin", "https://github.com/team/sk"],
    );
    register(&home, "github.com/team/vega", &vega, "VEGA");
    register(&home, "github.com/team/sk", &sk.join("main"), "SPELLKAVE");

    let misleading = home.join("VEGA");
    std::fs::create_dir_all(&misleading).unwrap();
    git(&misleading, &["init", "-q"]);
    git(
        &misleading,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/team/other.git",
        ],
    );
    register(&home, "github.com/team/other", &misleading, "OTHER");
    let forms = [
        ("github.com/team/vega", "github.com/team/vega"),
        ("~/src/vega/.git", "github.com/team/vega"),
        (&format!("{h}/src/vega/.git"), "github.com/team/vega"),
        (&format!("{h}/src/vega/.git/"), "github.com/team/vega"),
        (&format!("{h}/src/vega"), "github.com/team/vega"),
        ("~/src/vega/", "github.com/team/vega"),
        (&format!("{h}/src/vega/sub"), "github.com/team/vega"),
        ("VEGA", "github.com/team/vega"),
        ("vega", "github.com/team/vega"),
        ("~/src/sk/.bare", "github.com/team/sk"),
        (&format!("{h}/src/sk/.bare"), "github.com/team/sk"),
        (&format!("{h}/src/sk/.bare/"), "github.com/team/sk"),
        (&format!("{h}/src/sk"), "github.com/team/sk"),
        ("SPELLKAVE", "github.com/team/sk"),
    ];
    for (form, key) in forms {
        let o = resume(&home, form);
        let out = String::from_utf8_lossy(&o.stdout);
        assert!(o.status.success(), "{form}: {out}");
        assert!(out.starts_with(&format!("resumed: {key}")), "{form}: {out}");
    }

    let o = resume(&home, &format!("{h}/src/other"));
    let out = String::from_utf8_lossy(&o.stdout);
    assert_eq!(o.status.code(), Some(1), "{out}");
    assert!(out.contains("not_provisioned"), "{out}");
}

#[test]
fn shared_ids_require_a_location_and_resume_only_that_clone() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().canonicalize().unwrap();
    for name in ["a", "b"] {
        let dir = home.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q"]);
        git(
            &dir,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/team/shared.git",
            ],
        );
        register(&home, "github.com/team/shared", &dir, "SHARED");
    }
    for selector in ["github.com/team/shared", "SHARED"] {
        let out = resume(&home, selector);
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("project_location_ambiguous"));
    }
    assert!(resume(&home, home.join("a").to_str().unwrap())
        .status
        .success());
    let records =
        home.join("state/vega/bd/projects/v2")
            .join(vega_herdr::project::encode_component(
                "github.com/team/shared",
            ));
    for name in ["a", "b"] {
        let common = home.join(name).join(".git");
        let file = records.join(format!(
            "{}.json",
            vega_herdr::project::encode_component(common.to_str().unwrap())
        ));
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
        assert_eq!(value["value"]["resumed"].as_i64().unwrap() > 0, name == "a");
    }
}

#[test]
fn status_names_unconfigured_provisioning_without_changing_recorded_remotes() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let repo = home.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    let key = "github.com/team/repo";
    register(home, key, &repo, "TEST");
    let encode = vega_herdr::project::encode_component;
    let common = repo.join(".git").canonicalize().unwrap();
    let record = home
        .join("state/vega/bd/projects/v2")
        .join(encode(key))
        .join(format!("{}.json", encode(common.to_str().unwrap())));
    let original = std::fs::read(&record).unwrap();
    let run = || {
        let output = Command::new(env!("CARGO_BIN_EXE_vega-bd"))
            .arg("status")
            .env_clear()
            .current_dir(home)
            .env("HOME", home)
            .env("PATH", "/usr/bin:/bin")
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("XDG_STATE_HOME", home.join("state"))
            .env("VEGA_POST_HOST", "testhost")
            .output()
            .unwrap();
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(output.status.success(), "{text}");
        assert!(text.contains("projects[1]"), "{text}");
        assert_eq!(std::fs::read(&record).unwrap(), original);
        text
    };
    let text = run();
    assert!(text.contains("provisioning remote: unconfigured"), "{text}");
    assert!(
        text.contains("provisioned projects keep their recorded remotes"),
        "{text}"
    );
    std::fs::create_dir_all(home.join("config/vega")).unwrap();
    std::fs::write(
        home.join("config/vega/bd.remote"),
        "http://dolt.example:8080\n",
    )
    .unwrap();
    assert!(run().contains("provisioning remote: http://dolt.example:8080"));
}
