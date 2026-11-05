//! Flat, portable repo groups; routing continues to use ordinary v2 claims.
use super::*;
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

const HELP: &str = include_str!("../help/group.md");
fn error(message: impl Into<String>) -> Failure {
    Failure::new("group", message, "vega uac group --help")
}
fn root(path: &Path) -> Result<PathBuf, Failure> {
    Ok(vega_herdr::project::resolve(path, &st::post_host())?
        .location
        .checkout)
}
fn validate(id: &str) -> Result<(), Failure> {
    if id.starts_with("./") || id.starts_with("../") {
        return Err(error(
            "local member path does not exist; pass an existing checkout",
        ));
    }
    // Local/file IDs are already owner-qualified and cannot be normalized on
    // a different host. Network IDs round-trip through the shared resolver.
    if let Some(rest) = id
        .strip_prefix("local:")
        .or_else(|| id.strip_prefix("file:"))
    {
        if let Some((host, path)) = rest.split_once(':') {
            if !host.is_empty()
                && Path::new(path).is_absolute()
                && !id.chars().any(char::is_control)
            {
                return Ok(());
            }
        }
    } else if !id.contains("://") {
        // IDs forget transport. A port that is default for HTTPS can still
        // be a canonical nondefault SSH port (and conversely for port 22).
        for scheme in ["https", "ssh"] {
            if vega_herdr::project::normalize_remote(
                &format!("{scheme}://{id}"),
                &st::post_host(),
                Path::new("/"),
            )
            .is_ok_and(|normalized| normalized.as_str() == id)
            {
                return Ok(());
            }
        }
    }
    Err(error(
        "members must be canonical project IDs; use group add <path>",
    ))
}
pub(super) fn members(path: &Path) -> Result<BTreeSet<String>, Failure> {
    let file = root(path)?.join(".vega/group.toml");
    let text = match fs::read_to_string(file) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(e) => return Err(error(e.to_string())),
    };
    let doc = text
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| error(e.to_string()))?;
    if doc.iter().any(|(key, _)| key != "members") {
        return Err(error("unknown group key; only members is supported"));
    }
    let array = doc
        .get("members")
        .and_then(|v| v.as_array())
        .ok_or_else(|| error("group members must be an array of project IDs"))?;
    let mut members = BTreeSet::new();
    for item in array {
        let id = item
            .as_str()
            .ok_or_else(|| error("member must be a project ID string"))?;
        validate(id)?;
        members.insert(id.to_owned());
    }
    Ok(members)
}
fn add_input(input: &str) -> Result<String, Failure> {
    let path = expand_home(input);
    if path.exists() {
        return Ok(vega_herdr::project::resolve(&path, &st::post_host())?
            .id
            .to_string());
    }
    validate(input)?;
    Ok(input.into())
}
fn save(root: &Path, members: &BTreeSet<String>) -> Result<(), Failure> {
    let mut doc = toml_edit::DocumentMut::new();
    let mut array = toml_edit::Array::new();
    for member in members {
        array.push(member.as_str());
    }
    doc["members"] = toml_edit::value(array);
    let file = root.join(".vega/group.toml");
    fs::create_dir_all(file.parent().unwrap()).map_err(|e| error(e.to_string()))?;
    fs::write(file, doc.to_string()).map_err(|e| error(e.to_string()))
}
pub(super) fn run(args: &[String]) -> Result<(), Failure> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{HELP}");
        return Ok(());
    }
    if !matches!(args, [] | [_, _])
        || args
            .first()
            .is_some_and(|a| !matches!(a.as_str(), "add" | "discover"))
        || args.get(1).is_some_and(|a| a.starts_with('-'))
    {
        return Err(Failure::new(
            "usage",
            "use group, group add <path|project-id>, or group discover <dir>",
            HELP,
        ));
    }
    let root = root(&env::current_dir().map_err(|e| error(e.to_string()))?)?;
    let mut all = members(&root)?;
    match args {
        [cmd, input] if cmd == "add" => {
            all.insert(add_input(input)?);
        }
        [_, dir] => {
            for entry in fs::read_dir(expand_home(dir)).map_err(|e| error(e.to_string()))? {
                let path = entry.map_err(|e| error(e.to_string()))?.path();
                // Only direct repositories, not arbitrary subdirectories within one.
                if path.is_dir() && (path.join(".git").exists() || path.join("HEAD").is_file()) {
                    all.insert(
                        vega_herdr::project::resolve(&path, &st::post_host())?
                            .id
                            .to_string(),
                    );
                }
            }
        }
        _ => {}
    }
    if !args.is_empty() {
        save(&root, &all)?;
    }
    println!("members[{}]{{project}}:", all.len());
    for id in all {
        println!("  {}", cell(&id));
    }
    Ok(())
}
fn record(key: &str) -> Result<Value, Failure> {
    let file = st::project_file("uac", key).ok_or_else(|| error("state directory unavailable"))?;
    match fs::read_to_string(file) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| error(e.to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(serde_json::json!({})),
        Err(e) => Err(error(e.to_string())),
    }
}
pub(super) fn claim_all(
    key: &str,
    me: &str,
    agents: &[Value],
    view: Option<&vega_registry::View>,
    path: &Path,
) -> Result<(), Failure> {
    let mut members = members(path)?;
    members.insert(key.into());
    let here = st::post_host();
    // Preflight the entire set before even claiming the coordination repo.
    for member in &members {
        if let Some((pane, host)) = remote_uac(member, view).or_else(|| {
            uac_record(member).filter(|(p, h)| h != &here || p != me && alive(agents, p))
        }) {
            return Err(Failure::new(
                "uac_exists",
                format!("{pane} on {host} is the UAC of {member}"),
                "release that live UAC before claiming this repo or its group",
            ));
        }
        record(member)?;
    }
    claim(key, me, agents, view, path)?;
    for member in members.iter().filter(|m| m.as_str() != key) {
        let prior = record(member)?;
        let next = serde_json::json!({"schema":2,"project":member,"pane":me,"host":here,"at":unix_now(),"group":key});
        let mut next = claim_record(prior.clone(), next, None);
        if prior["pane"] == me && prior["host"] == here {
            next = prior;
            next["group"] = key.into();
        }
        let file =
            st::project_file("uac", member).ok_or_else(|| error("state directory unavailable"))?;
        fs::write(file, next.to_string()).map_err(|e| error(e.to_string()))?;
        println!("member_uac: {}", cell(member));
    }
    Ok(())
}
pub(super) fn release_all(key: &str, me: &str) -> Result<(), Failure> {
    // Check authority before touching member records.
    if uac_record(key).is_some_and(|(p, h)| p != me || h != st::post_host()) {
        return release(key, me);
    }
    for (member, pane, _) in st::project_uac_records() {
        let r = record(&member)?;
        if pane == me && r["host"] == st::post_host() && r["group"] == key {
            fs::remove_file(
                st::project_file("uac", &member)
                    .ok_or_else(|| error("state directory unavailable"))?,
            )
            .map_err(|e| error(e.to_string()))?;
        }
    }
    release(key, me)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn canonical_network_ids_keep_transport_independent_ports() {
        for id in [
            "github.com/team/service",
            "github.com:443/team/service",
            "git.example:22/Team/Service",
            "git.example/Team/Service",
        ] {
            assert!(validate(id).is_ok(), "{id}");
        }
        for id in [
            "https://github.com/team/service",
            "github.com/Team/Service",
            "github.com/team/../service",
            "../service",
        ] {
            assert!(validate(id).is_err(), "{id}");
        }
    }
    fn git(path: &Path, args: &[&str]) {
        assert!(std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .status()
            .unwrap()
            .success());
    }
    fn repo(path: &Path, name: &str) {
        fs::create_dir_all(path).unwrap();
        git(path, &["init", "-q"]);
        git(
            path,
            &[
                "remote",
                "add",
                "origin",
                &format!("https://github.com/team/{name}.git"),
            ],
        );
    }
    #[test]
    fn flat_groups_claim_preflight_release_and_publication_without_locations() {
        const CHILD: &str = "VEGA_GROUP_FIXTURE";
        if env::var_os(CHILD).is_none() {
            let tmp = tempfile::tempdir().unwrap();
            let out = std::process::Command::new(env::current_exe().unwrap())
                .args(["--exact", "group::tests::flat_groups_claim_preflight_release_and_publication_without_locations"])
                .env(CHILD,"1").env("HOME",tmp.path()).env("XDG_STATE_HOME",tmp.path().join("state"))
                .env("VEGA_POST_HOST","fixture").env("GIT_CONFIG_GLOBAL","/dev/null").env("GIT_CONFIG_NOSYSTEM","1")
                .output().unwrap();
            assert!(
                out.status.success(),
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            return;
        }
        let home = PathBuf::from(env::var_os("HOME").unwrap());
        let coord = home.join("coord");
        repo(&coord, "coord");
        env::set_current_dir(&coord).unwrap();
        let dir = home.join("members");
        repo(&dir.join("service"), "service");
        repo(&dir.join("web"), "web");
        repo(&dir.join("service/nested"), "nested");
        let bare = dir.join("bare-service.git");
        fs::create_dir_all(&bare).unwrap();
        git(&bare, &["init", "-q", "--bare"]);
        git(
            &bare,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/team/bare-service.git",
            ],
        );
        run(&["discover".into(), dir.to_string_lossy().into()]).unwrap();
        run(&["add".into(), "github.com/team/service".into()]).unwrap();
        run(&["add".into(), dir.join("web").to_string_lossy().into()]).unwrap();
        run(&[]).unwrap();
        assert_eq!(
            members(&coord).unwrap(),
            BTreeSet::from([
                "github.com/team/bare-service".into(),
                "github.com/team/service".into(),
                "github.com/team/web".into()
            ])
        );
        let before = fs::read(coord.join(".vega/group.toml")).unwrap();
        assert!(run(&["add".into(), "--unknown".into()]).is_err());
        assert_eq!(fs::read(coord.join(".vega/group.toml")).unwrap(), before);
        let key = "github.com/team/coord";
        let service = "github.com/team/service";
        let file = st::project_file("uac", service).unwrap();
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(
            &file,
            json!({"schema":2,"project":service,"pane":"busy","host":"fixture","at":1}).to_string(),
        )
        .unwrap();
        let agents = [json!({"pane_id":"busy"}), json!({"pane_id":"uac"})];
        assert_eq!(
            claim_all(key, "uac", &agents, None, &coord)
                .unwrap_err()
                .code,
            "uac_exists"
        );
        assert!(uac_record(key).is_none());
        // A gone member owner can be replaced, as with ordinary claims.
        claim_all(key, "uac", &agents[1..], None, &coord).unwrap();
        let first = record(service).unwrap();
        assert!(first.get("common_dir").is_none());
        assert_eq!(first["group"], key);
        claim_all(key, "uac", &agents[1..], None, &coord).unwrap();
        assert_eq!(record(service).unwrap(), first);
        for id in [key, service, "github.com/team/web"] {
            assert_eq!(
                st::project_uac_of(id),
                Some(("uac".into(), "fixture".into()))
            );
            let entry = vega_registry::uac_entry("fixture", &record(id).unwrap()).unwrap();
            assert_eq!(entry["project"], id);
            assert_eq!(entry["pane"], "uac");
        }
        assert!(release_all(key, "intruder").is_err());
        assert!(uac_record(service).is_some());
        fs::write(coord.join(".vega/group.toml"), "members = []\n").unwrap();
        release_all(key, "uac").unwrap();
        assert!(st::project_uac_records().is_empty());
        release_all(key, "uac").unwrap();
        fs::write(
            coord.join(".vega/group.toml"),
            "members = ['https://github.com/team/web']\n",
        )
        .unwrap();
        assert!(members(&coord).is_err());
    }
}
