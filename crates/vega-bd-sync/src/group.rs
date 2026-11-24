//! Flat group manifests and host-local coordination database selection.
use crate::{store, Project};
use std::{collections::BTreeSet, fs, path::Path};
use vega_herdr::Failure;

fn error(message: impl Into<String>) -> Failure {
    Failure::new("group", message, "vega uac group --help")
}
pub fn validate(id: &str) -> Result<(), Failure> {
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
                &vega_herdr::state::post_host(),
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
pub fn members(root: &Path) -> Result<BTreeSet<String>, Failure> {
    let file = root.join(".vega/group.toml");
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
/// A bd workspace, not bd's per-user `~/.beads` (machine-id, shared-server).
fn workspace(dir: &Path) -> bool {
    dir.join(".beads/metadata.json").is_file()
}
/// Select only a fallback. Native workspaces, redirects and explicit selection
/// belong to bd. A non-Git cwd has no canonical member identity.
pub fn coordination(cwd: &Path) -> Result<Option<Project>, Failure> {
    if std::env::var_os("BEADS_DIR").is_some_and(|v| !v.is_empty())
        || cwd.ancestors().any(workspace)
    {
        return Ok(None);
    }
    let Ok(member) = vega_herdr::project::resolve(cwd, &vega_herdr::state::post_host()) else {
        return Ok(None);
    };
    if member.location.common_dir.parent().is_some_and(workspace)
        || store::project(member.id.as_str(), &member.location.common_dir).is_some()
    {
        return Ok(None);
    }
    let mut matching = Vec::new();
    let projects = store::projects();
    // The native sync guard diagnoses a rekeyed registration at this location.
    if projects
        .iter()
        .any(|p| p.common == member.location.common_dir)
    {
        return Ok(None);
    }
    for project in projects {
        let manifest = project.dir.join(".vega/group.toml");
        if !members(&project.dir)
            .map_err(|e| {
                Failure::new(
                    "group_manifest",
                    format!("{}: {}", manifest.display(), e.message),
                    "repair the listed group.toml",
                )
            })?
            .contains(member.id.as_str())
        {
            continue;
        }
        project.validate_location()?;
        let live = vega_herdr::project::resolve(&project.dir, &vega_herdr::state::post_host())?;
        if live.location.common_dir != project.common
            || !project.beads_dir().is_some_and(Path::is_dir)
        {
            return Err(Failure::new(
                "group_registration",
                format!(
                    "Invalid coordination database registration for {}",
                    manifest.display()
                ),
                "repair the coordination checkout and its vega bd registration",
            ));
        }
        matching.push(project);
    }
    let id = &member.id;
    match matching.len() {
        1 => Ok(matching.pop()),
        0 => Err(Failure::new(
            "group_database_missing",
            format!("No local coordination database lists {id}"),
            "add the member ID to the coordination checkout's .vega/group.toml and register that checkout with vega bd init, or set BEADS_DIR",
        )),
        _ => {
            let manifests = matching.iter()
                .map(|p| p.dir.join(".vega/group.toml").display().to_string())
                .collect::<Vec<_>>().join(", ");
            Err(Failure::new(
                "group_database_ambiguous",
                format!("Multiple group manifests list {id}: {manifests}"),
                "set BEADS_DIR explicitly or correct the listed group manifests and registrations",
            ))
        }
    }
}
