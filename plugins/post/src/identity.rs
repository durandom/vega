//! Durable, owner-local flag-day migration. Deployment coordination belongs to
//! the UAC; no distributed lock, remote filesystem access or broker provisioning.
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};

use clap::Subcommand;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use vega_herdr::cutover::{Mapping, Snapshot};
use vega_herdr::project::{ProjectId, RepoLocation};
use vega_herdr::Failure;
use vega_registry::cutover::{Plan as RegistryPlan, Rekey as RegistryRekey};

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

const JOURNAL: &str = "identity-cutover/journal.json";
const BUNDLE: &str = "identity-cutover/approved.json";
const MARKER: &str = "identity-cutover/complete.json";
/// Explicit v2-to-v2 rekeys: one immutable generation `<n>.json` per rekey.
const REKEY: &str = "identity-cutover/rekey";

/// State the flag-day cutover snapshots and journals.
const KINDS: [&str; 10] = [
    "uac",
    "uac-status",
    "lanes",
    "badges",
    "notes",
    "hygiene.json",
    "pagerduty/state.json",
    "bd/projects",
    "bd/state",
    "bd/git",
];
/// A rekey also moves project-keyed state created after the flag day.
const REKEY_KINDS: [&str; 11] = [
    "uac",
    "uac-status",
    "lanes",
    "badges",
    "notes",
    "hygiene.json",
    "pagerduty/state.json",
    "bd/projects",
    "bd/state",
    "bd/git",
    "schedule",
];

#[derive(Subcommand)]
pub enum Action {
    /// Resolve this host's legacy keys and export the JSON array consumed by preview
    Mappings {
        #[arg(long)]
        output: PathBuf,
    },
    /// Snapshot this stopped host and export a preview (never mutates source records)
    Preview {
        #[arg(long)]
        mappings: PathBuf,
        #[arg(long)]
        output: PathBuf,
        /// Exact keys this inbox-only host is authorized to read
        #[arg(long)]
        registry_read: Vec<String>,
    },
    /// Validate the complete host inventory and export the deployment bundle
    Audit {
        #[arg(long, required = true)]
        plan: Vec<PathBuf>,
        #[arg(long, required = true)]
        host: Vec<String>,
        /// Orrery export: JSON object mapping each inbox host to its effective publish/subscribe allowlists
        #[arg(long)]
        grants: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Check that every planned owner is v2 and no legacy keys remain
    Verify {
        /// Check only these hosts' planned v2 projection (a late-host rejoin);
        /// completed owners' UACs and pins drift once writers resume
        #[arg(long)]
        host: Vec<String>,
    },
    /// Restore this host's journal; writers must stay stopped until old deploy
    Rollback,
    /// Move a project's records to its new ID after a remote change (v2 to v2)
    #[command(after_help = include_str!("../help/identity-rekey.md"))]
    Rekey {
        #[command(subcommand)]
        action: RekeyAction,
    },
}

#[derive(Subcommand)]
pub enum RekeyAction {
    /// Plan the rekey of this stopped host's records (changes nothing)
    Preview {
        /// The registered (old) project ID the identity error names
        #[arg(long)]
        from: String,
        /// The project's checkout (default: the current directory)
        #[arg(long)]
        checkout: Option<PathBuf>,
        #[arg(long)]
        output: PathBuf,
    },
    /// Journal the plan as the next generation and apply it; replays an interrupted one
    Apply {
        #[arg(long)]
        plan: PathBuf,
    },
    /// Undo the newest rekey generation not yet rolled back
    Rollback,
}

/// One rekey generation, journaled before any change. `original` holds the
/// prior bytes of every path the rekey writes or removes (absent: new file).
#[derive(Serialize, Deserialize, PartialEq)]
pub struct RekeyPlan {
    schema: u32,
    host: String,
    inbox_only: bool,
    from: String,
    to: ProjectId,
    location: RepoLocation,
    original: Snapshot,
    writes: Snapshot,
    remove: BTreeSet<PathBuf>,
    registry: RegistryRekey,
}

#[derive(Serialize, Deserialize)]
pub struct HostPlan {
    schema: u32,
    host: String,
    inbox_only: bool,
    mappings: Vec<Mapping>,
    original: Snapshot,
    writes: Snapshot,
    remove: BTreeSet<PathBuf>,
    registry: RegistryPlan,
    /// Every owned ephemeral payload, including agents and notes republished at startup.
    owned: BTreeMap<String, Value>,
    registry_reads: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct Grants {
    publish: Vec<String>,
    subscribe: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct Bundle {
    schema: u32,
    hosts: Vec<HostPlan>,
    grants: BTreeMap<String, Grants>,
}

pub(super) fn failure(message: impl ToString) -> Failure {
    Failure::new("identity_cutover", message.to_string(), "keep writers stopped; see docs/design/project-registry-cutover.md; run `vega post identity --help`")
}
fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    serde_json::from_slice(&fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?)
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Rename + fsync file and parent: the journal is durable before any mutation.
fn durable_dir(path: &Path) -> Result<(), String> {
    if path.is_dir() {
        return Ok(());
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    durable_dir(parent)?;
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && path.is_dir() => {}
        Err(e) => return Err(e.to_string()),
    }
    fs::File::open(parent)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())
}

fn atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    durable_dir(parent)?;
    let mut tmp = path.with_extension("cutover-tmp");
    if tmp == path {
        tmp.set_extension("cutover-tmp.next");
    }
    // Interrupted writes may leave a plain temporary file. Unlink it rather
    // than truncating it: it could be hard-linked to a source record. Reject
    // symlinks intact, and create exclusively to close the check/open race.
    match fs::symlink_metadata(&tmp) {
        Ok(meta) if meta.file_type().is_file() => {
            fs::remove_file(&tmp).map_err(|e| e.to_string())?;
        }
        Ok(_) => return Err(format!("unsafe cutover temporary file: {}", tmp.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.to_string()),
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(libc::O_NOFOLLOW)
        .mode(0o600)
        .open(&tmp)
        .map_err(|e| e.to_string())?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    fs::rename(tmp, path).map_err(|e| e.to_string())?;
    fs::File::open(parent)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())
}
fn save(path: &Path, v: &impl Serialize) -> Result<(), String> {
    atomic(
        path,
        &serde_json::to_vec_pretty(v).map_err(|e| e.to_string())?,
    )
}
fn delete(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => fs::File::open(path.parent().ok_or("no parent")?)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}
fn safe(path: &Path) -> bool {
    !path.as_os_str().is_empty() && path.components().all(|p| matches!(p, Component::Normal(_)))
}

fn snapshot(root: &Path) -> Result<Snapshot, String> {
    snapshot_of(root, &KINDS)
}
fn snapshot_of(root: &Path, kinds: &[&str]) -> Result<Snapshot, String> {
    fn walk(root: &Path, path: &Path, out: &mut Snapshot) -> Result<(), String> {
        if !path.exists() {
            return Ok(());
        }
        let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        if meta.file_type().is_symlink() {
            return Err(format!("symlink in cutover state: {}", path.display()));
        }
        if meta.is_dir() {
            for entry in fs::read_dir(path).map_err(|e| e.to_string())? {
                walk(root, &entry.map_err(|e| e.to_string())?.path(), out)?;
            }
        } else if meta.is_file() {
            out.insert(
                path.strip_prefix(root).map_err(|e| e.to_string())?.into(),
                fs::read(path).map_err(|e| e.to_string())?,
            );
        }
        Ok(())
    }
    let mut out = Snapshot::new();
    for kind in kinds {
        walk(root, &root.join(kind), &mut out)?;
    }
    Ok(out)
}

pub(super) fn local_plans(
    original: &Snapshot,
    mappings: &[Mapping],
    host: &str,
) -> Result<(Snapshot, BTreeSet<PathBuf>), String> {
    let local: Snapshot = original
        .iter()
        .filter(|(p, _)| !p.starts_with("bd"))
        .map(|(p, b)| (p.clone(), b.clone()))
        .collect();
    let plan = vega_herdr::cutover::prepare_with_aliases(
        &local,
        mappings,
        host,
        &[&vega_herdr::local_host()],
    )?;
    let bd: Snapshot = original
        .iter()
        .filter_map(|(p, b)| p.strip_prefix("bd").ok().map(|p| (p.into(), b.clone())))
        .collect();
    let mappings = mappings
        .iter()
        .map(|m| vega_bd_sync::cutover::Mapping {
            old_key: m.old_key.clone(),
            project: m.project.clone(),
            location: m.location.clone(),
        })
        .collect::<Vec<_>>();
    let bd = vega_bd_sync::cutover::prepare(&bd, &mappings)?;
    let mut writes = plan.writes;
    let mut remove = plan.remove;
    writes.extend(
        bd.writes
            .into_iter()
            .map(|(p, b)| (Path::new("bd").join(p), b)),
    );
    remove.extend(bd.remove.into_iter().map(|p| Path::new("bd").join(p)));
    Ok((writes, remove))
}

/// Compose the pure local and registry plans before journaling. Registry-only
/// aliases and the earliest published claim time must survive reconciliation.
fn retain_registry_claims(writes: &mut Snapshot, registry: &RegistryPlan) -> Result<(), String> {
    for (key, claim) in &registry.projection {
        if key.split('.').nth(1) != Some("uac") {
            continue;
        }
        let project = claim["project"].as_str().ok_or("claim missing project")?;
        let path = Path::new("uac/v2").join(format!(
            "{}.json",
            vega_herdr::project::encode_component(project)
        ));
        let bytes = writes.get(&path).ok_or(
            "owned registry claim has no migrated local claim; reconcile ownership before preview",
        )?;
        let mut local: Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if local["pane"] != claim["pane"] || local["host"] != claim["host"] {
            return Err("registry/local ownership mismatch".into());
        }
        for field in ["aliases", "origin_name", "folder_name", "at"] {
            if !claim[field].is_null() {
                local[field] = claim[field].clone();
            }
        }
        if vega_registry::uac_entry(registry.host(), &local)? != *claim {
            return Err("registry claim cannot be reproduced from migrated local state".into());
        }
        writes.insert(
            path,
            serde_json::to_vec_pretty(&local).map_err(|e| e.to_string())?,
        );
    }
    Ok(())
}

fn validate_bundle(bundle: &Bundle, expected: &[String]) -> Result<(), String> {
    if bundle.schema != 2 {
        return Err("mixed cutover schema versions".into());
    }
    let mut hosts = BTreeSet::new();
    let mut plans = Vec::new();
    for p in &bundle.hosts {
        if p.registry.host() != p.host
            || p.schema != 2
            || p.host != vega_registry::token(&p.host)
            || !hosts.insert(p.host.clone())
        {
            return Err("mixed versions, invalid or duplicate host plan".into());
        }
        for path in p
            .original
            .keys()
            .chain(p.writes.keys())
            .chain(p.remove.iter())
        {
            if !safe(path) || !KINDS.iter().any(|kind| path.starts_with(kind)) {
                return Err("unsafe journal path".into());
            }
        }
        if p.inbox_only
            && (!p.owned.is_empty()
                || !p.registry.backup.is_empty()
                || !p.registry.projection.is_empty())
        {
            return Err("inbox-only host has registry writer state".into());
        }
        if p.owned
            .keys()
            .any(|key| !key.starts_with(&format!("{}.", p.host)))
        {
            return Err("foreign payload in owner backup".into());
        }
        for key in &p.registry_reads {
            crate::auth::parse_registry_read(&format!("{}={key}", p.host))
                .map_err(|e| e.message)?;
            if !p.inbox_only {
                return Err("registry read checks belong to inbox-only hosts".into());
            }
        }
        plans.push(p.registry.clone());
    }
    let expected: BTreeSet<_> = expected.iter().cloned().collect();
    if hosts != expected {
        return Err("bundle must contain exactly the UAC's full deployment host inventory (including offline/inbox-only hosts)".into());
    }
    validate_grants(bundle)?;
    vega_registry::cutover::validate(&plans)?;
    for p in &bundle.hosts {
        for key in &p.registry_reads {
            if !bundle
                .hosts
                .iter()
                .any(|h| h.registry.projection.contains_key(key))
            {
                return Err(format!("exact read {key} has no planned publication"));
            }
        }
    }
    Ok(())
}

fn verify_locations(p: &HostPlan) -> Result<(), String> {
    for m in &p.mappings {
        let resolved =
            vega_herdr::project::resolve(&m.location.checkout, &p.host).map_err(|e| e.message)?;
        if resolved.id != m.project || resolved.location != m.location {
            return Err(format!(
                "identity/location changed since preview: {}",
                m.old_key
            ));
        }
    }
    Ok(())
}

fn apply_files(root: &Path, p: &HostPlan) -> Result<(), String> {
    write_files(root, &p.writes, &p.remove)
}
fn write_files(root: &Path, writes: &Snapshot, remove: &BTreeSet<PathBuf>) -> Result<(), String> {
    for (path, bytes) in writes {
        atomic(&root.join(path), bytes)?;
    }
    for path in remove {
        delete(&root.join(path))?;
    }
    Ok(())
}
fn restore_files(root: &Path, p: &HostPlan) -> Result<(), String> {
    restore(root, &p.original, &p.writes)
}
fn restore(root: &Path, original: &Snapshot, writes: &Snapshot) -> Result<(), String> {
    for path in writes.keys() {
        if !original.contains_key(path) {
            delete(&root.join(path))?;
        }
    }
    for (path, bytes) in original {
        atomic(&root.join(path), bytes)?;
    }
    Ok(())
}

/// Fresh old daemons must be stopped before deployment; an unplanned host or
/// old offline host returning later is a hard failure, never a protocol bridge.
fn check_live(entries: &BTreeMap<String, Value>, bundle: &Bundle, now: i64) -> Result<(), String> {
    for (key, v) in entries {
        let owner = key.split('.').next().unwrap_or_default();
        if key.ends_with(".beat")
            && v["at"]
                .as_i64()
                .is_some_and(|at| now - at <= vega_registry::FRESH.as_secs() as i64)
            && (v["schema"] != 2 || !bundle.hosts.iter().any(|p| p.host == owner))
        {
            return Err(format!(
                "mixed version or unplanned live host: {owner}; stop it before cutover/rejoin"
            ));
        }
        if vega_registry::is_legacy_project_key(key)
            && !bundle
                .hosts
                .iter()
                .any(|p| p.registry.backup.get(key) == Some(v))
        {
            return Err(format!(
                "unplanned legacy key {key}; host must upgrade and preview before rejoin"
            ));
        }
    }
    Ok(())
}

fn export_mappings(output: &Path) -> Result<(), Failure> {
    let root = crate::service::state_dir()?;
    // Resolve existing ancestors too: an output symlink must never write into
    // source state. Check before taking the relay lock or creating any file.
    fn physical(path: &Path) -> std::io::Result<PathBuf> {
        if path.exists() {
            return path.canonicalize();
        }
        let parent = path
            .parent()
            .ok_or_else(|| std::io::Error::other("invalid export path"))?;
        Ok(physical(parent)?.join(
            path.file_name()
                .ok_or_else(|| std::io::Error::other("invalid export filename"))?,
        ))
    }
    let absolute = std::path::absolute(output).map_err(failure)?;
    let target = physical(&absolute).map_err(failure)?;
    // rename() replaces the final component itself. Check its location as
    // well as the symlink target: either may lie in the source state root.
    let logical = physical(
        absolute
            .parent()
            .ok_or_else(|| failure("invalid export path"))?,
    )
    .map_err(failure)?
    .join(
        absolute
            .file_name()
            .ok_or_else(|| failure("invalid export filename"))?,
    );
    let source = physical(&std::path::absolute(&root).map_err(failure)?).map_err(failure)?;
    if target.starts_with(&source) || logical.starts_with(&source) {
        return Err(failure(
            "mappings output must be outside the live vega state root",
        ));
    }
    let _lock = crate::service::lock(false)?;
    if root.join(JOURNAL).exists() {
        return Err(failure("journal already exists; retain it for restart/rollback, never resnapshot partial state"));
    }
    let original = snapshot(&root).map_err(failure)?;
    let host = crate::host();
    let registry = if vega_herdr::state::is_inbox_only(&host) {
        BTreeMap::new()
    } else {
        crate::runtime()?
            .block_on(async {
                let c = vega_registry::connect("identity mappings").await?;
                let kv = vega_registry::bucket(&c).await?;
                raw_entries(&kv).await
            })
            .map_err(failure)?
    };
    let mappings = crate::identity_mappings::generate(
        &original,
        &registry,
        &host,
        &PathBuf::from(std::env::var_os("HOME").ok_or_else(|| failure("HOME is not set"))?),
    )?;
    save(output, &mappings).map_err(failure)?;
    println!("identity: mappings saved\nmappings: {}\noutput: {}\nhelp: vega post identity preview --mappings {} --output host-plan.json", mappings.len(), vega_herdr::cell(&output.display().to_string()), vega_herdr::cell(&output.display().to_string()));
    Ok(())
}

pub fn run(action: Action) -> Result<(), Failure> {
    // Same host token and relay lock as vegad: `hostname -s` can differ from
    // the registry host (io-wsl vs io) in a shell without VEGA_POST_HOST.
    crate::service::load_env_file();
    if let Action::Mappings { output } = action {
        return export_mappings(&output);
    }
    run_inner(action).map_err(failure)
}
fn run_inner(action: Action) -> Result<(), String> {
    match action {
        Action::Mappings { .. } => unreachable!("handled at structured-error boundary"),
        Action::Rekey { action } => rekey(action),
        Action::Audit {
            plan,
            host,
            grants,
            output,
        } => {
            let bundle = Bundle {
                schema: 2,
                hosts: plan.iter().map(|p| read(p)).collect::<Result<_, _>>()?,
                grants: read(&grants)?,
            };
            validate_bundle(&bundle, &host)?;
            save(&output, &bundle)?;
            println!(
                "identity: audited {} hosts\nbundle: {}",
                bundle.hosts.len(),
                output.display()
            );
            for p in &bundle.hosts {
                for subject in p.registry.direct_get_subjects() {
                    println!("grant_candidate: {subject}");
                }
            }
            Ok(())
        }
        Action::Preview {
            mappings,
            output,
            registry_read,
        } => {
            let _lock = crate::service::lock(false).map_err(|e| e.message)?;
            let root = crate::service::state_dir().map_err(|e| e.message)?;
            if root.join(JOURNAL).exists() {
                return Err("journal already exists; retain it for restart/rollback, never resnapshot partial state".into());
            }
            let target = std::path::absolute(&output).map_err(|e| e.to_string())?;
            if target.starts_with(&root) {
                return Err("preview output must be outside the live vega state root".into());
            }
            let mappings: Vec<Mapping> = read(&mappings)?;
            let original = snapshot(&root)?;
            let host = crate::host();
            let (mut writes, remove) = local_plans(&original, &mappings, &host)?;
            let inbox_only = vega_herdr::state::is_inbox_only(&host);
            let (registry, owned) = crate::runtime().map_err(|e| e.message)?.block_on(async {
                if inbox_only {
                    return RegistryPlan::prepare(
                        &host,
                        &BTreeMap::new(),
                        &BTreeMap::new(),
                        BTreeMap::new(),
                    )
                    .map(|p| (p, BTreeMap::new()));
                }
                let c = vega_registry::connect("identity preview").await?;
                let kv = vega_registry::bucket(&c).await?;
                let entries = raw_entries(&kv).await?;
                let ids = mappings
                    .iter()
                    .map(|m| (m.old_key.clone(), m.project.clone()))
                    .collect();
                let mut publication = BTreeMap::new();
                for (path, bytes) in &writes {
                    if path.starts_with("uac/v2") {
                        let v: Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
                        let id = v["project"].as_str().ok_or("missing project")?;
                        publication.insert(
                            vega_registry::uac_key(&host, id),
                            vega_registry::uac_entry(&host, &v)?,
                        );
                    }
                }
                let plan = RegistryPlan::prepare(&host, &entries, &ids, publication)?;
                let owned = entries
                    .into_iter()
                    .filter(|(key, _)| key.starts_with(&format!("{host}.")))
                    .collect();
                Ok::<_, String>((plan, owned))
            })?;
            retain_registry_claims(&mut writes, &registry)?;
            let p = HostPlan {
                schema: 2,
                host,
                inbox_only,
                mappings,
                original,
                writes,
                remove,
                registry,
                owned,
                registry_reads: registry_read,
            };
            verify_locations(&p)?;
            save(&output, &p)?;
            println!(
                "identity: preview saved\nplan: {}\nwrites: {}\nremovals: {}",
                output.display(),
                p.writes.len(),
                p.remove.len()
            );
            Ok(())
        }
        Action::Verify { host } => {
            let root = crate::service::state_dir().map_err(|e| e.message)?;
            let bundle: Bundle = read(&root.join(BUNDLE))?;
            let plans = projection_hosts(&bundle, &host)?;
            crate::runtime().map_err(|e| e.message)?.block_on(async {
                let c = vega_registry::connect("identity verify").await?;
                let kv = vega_registry::bucket(&c).await?;
                let entries = raw_entries(&kv).await?;
                verify_deployment(&entries, &bundle, now())?;
                for p in &plans {
                    p.registry.verify(&c, &kv).await?;
                }
                Ok::<_, String>(())
            })?;
            println!("identity: all planned owners publish v2; no legacy keys remain");
            if !host.is_empty() {
                println!("projection: verified for {}", host.join(", "));
            }
            Ok(())
        }
        Action::Rollback => {
            let _lock = crate::service::lock(false).map_err(|e| e.message)?;
            let root = crate::service::state_dir().map_err(|e| e.message)?;
            // Generations unwind newest first; the flag-day journal is the oldest.
            if let Some(n) = generations(&root)?
                .into_iter()
                .find(|&n| !generation(&root, n, "rolled-back").exists())
            {
                return Err(format!("rekey generation {n} is not rolled back; run `vega post identity rekey rollback` first"));
            }
            let p: HostPlan = read(&root.join(JOURNAL))?;
            if p.host != crate::host() {
                return Err("foreign host journal".into());
            }
            // Persist rollback intent first: an interrupted rollback can never
            // accidentally resume forward migration at daemon startup.
            atomic(&root.join("identity-cutover/rollback"), b"rollback\n")?;
            crate::runtime().map_err(|e| e.message)?.block_on(async {
                if !p.inbox_only {
                    let c = vega_registry::connect("identity rollback").await?;
                    let kv = vega_registry::bucket(&c).await?;
                    p.registry.rollback(&kv).await?;
                    // Ephemeral pane-keyed publications also changed at startup.
                    // Restore them from the same durable owner snapshot.
                    for key in raw_entries(&kv)
                        .await?
                        .keys()
                        .filter(|key| key.starts_with(&format!("{}.", p.host)))
                    {
                        if !p.owned.contains_key(key) {
                            kv.delete(key).await.map_err(|e| e.to_string())?;
                        }
                    }
                    for (key, value) in &p.owned {
                        kv.put(key, value.to_string().into())
                            .await
                            .map_err(|e| e.to_string())?;
                    }
                }
                restore_files(&root, &p)
            })?;
            delete(&root.join(MARKER))?;
            println!("identity: rolled back; retain journal and keep new binaries stopped");
            Ok(())
        }
    }
}

fn generation(root: &Path, n: u32, ext: &str) -> PathBuf {
    root.join(REKEY).join(format!("{n}.{ext}"))
}

/// Journaled rekey generations, oldest first.
fn generations(root: &Path) -> Result<Vec<u32>, String> {
    let dir = root.join(REKEY);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|e| e.to_string())? {
        let name = entry.map_err(|e| e.to_string())?.file_name();
        if let Some(n) = name
            .to_str()
            .and_then(|n| n.strip_suffix(".json"))
            .and_then(|n| n.parse().ok())
        {
            out.push(n);
        }
    }
    out.sort_unstable();
    Ok(out)
}

/// A generation whose apply or rollback has not finished: vegad stays
/// quiesced and only that generation's apply or rollback may continue.
fn pending(root: &Path) -> Result<Option<u32>, String> {
    Ok(generations(root)?.into_iter().find(|&n| {
        !generation(root, n, "rolled-back").exists()
            && (!generation(root, n, "done").exists() || generation(root, n, "rollback").exists())
    }))
}

/// Rekeys follow a completed flag day and never touch its journal or bundle.
fn rekey_ready(root: &Path) -> Result<(), String> {
    if root.join("identity-cutover/rollback").exists() || !root.join(MARKER).exists() {
        return Err(
            "rekey needs a completed identity cutover on this host (complete.json, no rollback)"
                .into(),
        );
    }
    Ok(())
}

/// The pure file plans of both state trees plus every checkout registered
/// under `from`, which must now resolve to `to`.
fn rekey_files(
    snap: &Snapshot,
    from: &str,
    to: &ProjectId,
) -> Result<(Snapshot, BTreeSet<PathBuf>, BTreeSet<PathBuf>), String> {
    let local: Snapshot = snap
        .iter()
        .filter(|(p, _)| !p.starts_with("bd"))
        .map(|(p, b)| (p.clone(), b.clone()))
        .collect();
    let plan = vega_herdr::cutover::rekey(&local, from, to)?;
    let bd: Snapshot = snap
        .iter()
        .filter_map(|(p, b)| p.strip_prefix("bd").ok().map(|p| (p.into(), b.clone())))
        .collect();
    let (bd, mut checkouts) = vega_bd_sync::cutover::rekey(&bd, from, to)?;
    let mut writes = plan.writes;
    let mut remove = plan.remove;
    writes.extend(
        bd.writes
            .into_iter()
            .map(|(p, b)| (Path::new("bd").join(p), b)),
    );
    remove.extend(bd.remove.into_iter().map(|p| Path::new("bd").join(p)));
    let claim = Path::new("uac/v2").join(format!(
        "{}.json",
        vega_herdr::project::encode_component(from)
    ));
    if let Some(bytes) = snap.get(&claim) {
        let v: Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        let locations = v["locations"].as_array().cloned().unwrap_or_default();
        checkouts.extend(
            std::iter::once(&v["checkout"])
                .chain(locations.iter().map(|l| &l["checkout"]))
                .filter_map(|c| c.as_str().map(PathBuf::from)),
        );
    }
    Ok((writes, remove, checkouts))
}

fn rekey(action: RekeyAction) -> Result<(), String> {
    let _lock = crate::service::lock(false).map_err(|e| e.message)?;
    let root = crate::service::state_dir().map_err(|e| e.message)?;
    let host = crate::host();
    rekey_ready(&root)?;
    match action {
        RekeyAction::Preview {
            from,
            checkout,
            output,
        } => {
            if std::path::absolute(&output)
                .map_err(|e| e.to_string())?
                .starts_with(&root)
            {
                return Err("rekey plan must be outside the live vega state root".into());
            }
            if let Some(n) = pending(&root)? {
                return Err(format!(
                    "rekey generation {n} is incomplete; apply or roll it back first"
                ));
            }
            let checkout = match checkout {
                Some(c) => c,
                None => std::env::current_dir().map_err(|e| e.to_string())?,
            };
            let current = vega_herdr::project::resolve(&checkout, &host)
                .map_err(|e| format!("{}: {}", e.code, e.message))?;
            let to = current.id;
            let snap = snapshot_of(&root, &REKEY_KINDS)?;
            let (writes, remove, checkouts) = rekey_files(&snap, &from, &to)?;
            for c in &checkouts {
                let id = vega_herdr::project::resolve(c, &host)
                    .map_err(|e| format!("{}: {}: {}", c.display(), e.code, e.message))?
                    .id;
                if id != to {
                    return Err(format!("{} still resolves to {id}: the old ID has another live location; rekey it to {to} first", c.display()));
                }
            }
            let bundle: Bundle = read(&root.join(BUNDLE))?;
            let hex = vega_herdr::project::encode_component(&from);
            if bundle
                .hosts
                .iter()
                .flat_map(|p| &p.registry_reads)
                .chain(bundle.grants.values().flat_map(|g| &g.publish))
                .any(|key| key.contains(&format!(".v2.{hex}")))
            {
                return Err("an inbox-only host has an exact registry read or grant for the old ID: the new key needs an orrery grant change with the Slayer's approval (docs/design/project-identity.md, Rekey after a remote change)".into());
            }
            let inbox_only = vega_herdr::state::is_inbox_only(&host);
            let registry = if inbox_only {
                RegistryRekey::default()
            } else {
                let path = Path::new("uac/v2").join(format!("{}.json", to.encoded()));
                let claim = writes
                    .get(&path)
                    .or(snap.get(&path))
                    .map(|b| serde_json::from_slice::<Value>(b))
                    .transpose()
                    .map_err(|e| e.to_string())?;
                crate::runtime().map_err(|e| e.message)?.block_on(async {
                    let c = vega_registry::connect("identity rekey").await?;
                    let kv = vega_registry::bucket(&c).await?;
                    let entries = raw_entries(&kv).await?;
                    RegistryRekey::prepare(&host, &entries, &from, &to, claim.as_ref())
                })?
            };
            if writes.is_empty() && remove.is_empty() && registry.backup.is_empty() {
                return Err(format!("no records of {from} on this host"));
            }
            let original = writes
                .keys()
                .chain(remove.iter())
                .filter_map(|p| snap.get(p).map(|b| (p.clone(), b.clone())))
                .collect();
            let plan = RekeyPlan {
                schema: 2,
                host,
                inbox_only,
                from,
                to,
                location: current.location,
                original,
                writes,
                remove,
                registry,
            };
            save(&output, &plan)?;
            println!(
                "identity: rekey preview saved\nfrom: {}\nto: {}\nplan: {}\nwrites: {}\nremovals: {}\nregistry: {} replaced, {} published\nhelp: vega post identity rekey apply --plan {}",
                vega_herdr::cell(&plan.from),
                vega_herdr::cell(&plan.to),
                output.display(),
                plan.writes.len(),
                plan.remove.len(),
                plan.registry.backup.len(),
                plan.registry.projection.len(),
                output.display()
            );
            Ok(())
        }
        RekeyAction::Apply { plan } => {
            let plan: RekeyPlan = read(&plan)?;
            if plan.schema != 2 || plan.host != host {
                return Err("foreign or invalid rekey plan".into());
            }
            if plan
                .original
                .keys()
                .chain(plan.writes.keys())
                .chain(plan.remove.iter())
                .any(|p| !safe(p) || !REKEY_KINDS.iter().any(|kind| p.starts_with(kind)))
            {
                return Err("unsafe rekey path".into());
            }
            let resume = pending(&root)?;
            if let Some(n) = resume {
                if generation(&root, n, "rollback").exists() {
                    return Err(format!("rollback of rekey generation {n} is in progress; finish it with `vega post identity rekey rollback`"));
                }
                if read::<RekeyPlan>(&generation(&root, n, "json"))? != plan {
                    return Err(format!("rekey generation {n} is incomplete with another plan; apply that plan or roll it back"));
                }
            } else {
                let current = vega_herdr::project::resolve(&plan.location.checkout, &host)
                    .map_err(|e| format!("{}: {}", e.code, e.message))?;
                if current.id != plan.to || current.location != plan.location {
                    return Err(
                        "identity/location changed since preview; repeat the preview".into(),
                    );
                }
                let live = snapshot_of(&root, &REKEY_KINDS)?;
                if let Some(p) = plan
                    .writes
                    .keys()
                    .chain(plan.remove.iter())
                    .find(|p| live.get(*p) != plan.original.get(*p))
                {
                    return Err(format!(
                        "{} changed since preview; stop writers and repeat the preview",
                        p.display()
                    ));
                }
            }
            crate::runtime().map_err(|e| e.message)?.block_on(async {
                let broker = if plan.inbox_only {
                    None
                } else {
                    let c = vega_registry::connect("identity rekey").await?;
                    let kv = vega_registry::bucket(&c).await?;
                    if resume.is_none() {
                        plan.registry.matches(&raw_entries(&kv).await?, false)?;
                    }
                    Some((c, kv))
                };
                let n = match resume {
                    Some(n) => n,
                    None => {
                        let n = generations(&root)?.last().map_or(1, |n| n + 1);
                        // The generation is durable before any state or KV change.
                        save(&generation(&root, n, "json"), &plan)?;
                        n
                    }
                };
                write_files(&root, &plan.writes, &plan.remove)?;
                if let Some((c, kv)) = &broker {
                    plan.registry.apply(kv).await?;
                    plan.registry.verify(c, kv).await?;
                }
                atomic(&generation(&root, n, "done"), b"done\n")?;
                println!(
                    "identity: rekeyed {} to {}\ngeneration: {n}\nhelp: vega post relay start",
                    vega_herdr::cell(&plan.from),
                    vega_herdr::cell(&plan.to)
                );
                Ok::<_, String>(())
            })
        }
        RekeyAction::Rollback => {
            let n = generations(&root)?
                .into_iter()
                .rev()
                .find(|&n| !generation(&root, n, "rolled-back").exists())
                .ok_or("no rekey generation to roll back")?;
            let p: RekeyPlan = read(&generation(&root, n, "json"))?;
            if p.host != host {
                return Err("foreign host rekey generation".into());
            }
            crate::runtime().map_err(|e| e.message)?.block_on(async {
                let broker = if p.inbox_only {
                    None
                } else {
                    let c = vega_registry::connect("identity rekey rollback").await?;
                    Some(vega_registry::bucket(&c).await?)
                };
                let intent = generation(&root, n, "rollback");
                // An applied generation rolls back only while nothing has
                // changed since: a snapshot restore would drop newer work.
                if !intent.exists() && generation(&root, n, "done").exists() {
                    let live = snapshot_of(&root, &REKEY_KINDS)?;
                    if let Some(path) = p
                        .writes
                        .keys()
                        .chain(p.remove.iter())
                        .find(|path| live.get(*path) != p.writes.get(*path))
                    {
                        return Err(format!("{} changed since rekey generation {n}; rollback would drop newer work", path.display()));
                    }
                    if let Some(kv) = &broker {
                        p.registry.matches(&raw_entries(kv).await?, true)?;
                    }
                }
                atomic(&intent, b"rollback\n")?;
                if let Some(kv) = &broker {
                    p.registry.rollback(kv).await?;
                }
                restore(&root, &p.original, &p.writes)?;
                atomic(&generation(&root, n, "rolled-back"), b"rolled back\n")?;
                println!(
                    "identity: rekey generation {n} rolled back ({} restored)\nhelp: vega post relay start",
                    vega_herdr::cell(&p.from)
                );
                Ok::<_, String>(())
            })
        }
    }
}

async fn raw_entries(
    kv: &async_nats::jetstream::kv::Store,
) -> Result<BTreeMap<String, Value>, String> {
    use futures_util::StreamExt;
    // Subject listing plus concurrent gets, as vega_registry::read_kinds: a
    // `kv.keys()` consumer and sequential gets time out on the live bucket.
    let prefix = format!("$KV.{}.", vega_registry::BUCKET);
    let stream = kv.stream.clone();
    let mut subjects = stream
        .info_with_subjects(format!("{prefix}>"))
        .await
        .map_err(|e| e.to_string())?;
    let mut keys = Vec::new();
    while let Some(s) = subjects.next().await {
        let (subject, _) = s.map_err(|e| e.to_string())?;
        if let Some(k) = subject.strip_prefix(&prefix) {
            keys.push(k.to_string());
        }
    }
    let gets = keys.into_iter().map(|key| async move {
        let got = kv.get(&key).await.map_err(|e| e.to_string());
        (key, got)
    });
    let mut got = futures_util::stream::iter(gets).buffer_unordered(64);
    let mut entries = BTreeMap::new();
    while let Some((key, bytes)) = got.next().await {
        if let Some(bytes) = bytes? {
            entries.insert(
                key,
                serde_json::from_slice(&bytes).map_err(|e| e.to_string())?,
            );
        }
    }
    Ok(entries)
}

/// Runs under the relay lock, before even local-only workers are spawned.
/// Missing approval is also the offline-host rejoin guard. No network fallback
/// is allowed until this host has completed its first migration.
pub async fn startup() -> Result<(), Failure> {
    startup_inner().await.map_err(failure)
}
/// Shared durable file preparation; deliberately has no broker access.
fn prepare_startup_files(root: &Path, approved: &HostPlan, host: &str) -> Result<HostPlan, String> {
    if root.join("identity-cutover/rollback").exists() {
        return Err("rollback in progress/completed; new binaries must remain stopped".into());
    }
    if !root.join(JOURNAL).exists() {
        verify_locations(approved)?;
        let (mut writes, remove) = local_plans(&approved.original, &approved.mappings, host)?;
        retain_registry_claims(&mut writes, &approved.registry)?;
        if writes != approved.writes || remove != approved.remove {
            return Err("local plan does not match pure projection".into());
        }
        if snapshot(root)? != approved.original {
            return Err(
                "local records changed since preview; stop writers and repeat preview/audit".into(),
            );
        }
        // Backup + exact plan are persisted together, before any state or KV mutation.
        save(&root.join(JOURNAL), approved)?;
    }
    let p: HostPlan = read(&root.join(JOURNAL))?;
    if serde_json::to_value(&p).map_err(|e| e.to_string())?
        != serde_json::to_value(approved).map_err(|e| e.to_string())?
    {
        return Err("approved bundle does not match durable owner journal".into());
    }
    Ok(p)
}

async fn startup_inner() -> Result<(), String> {
    let root = crate::service::state_dir().map_err(|e| e.message)?;
    if root.join("identity-cutover/rollback").exists() {
        return Err("rollback in progress/completed; new binaries must remain stopped".into());
    }
    if let Some(n) = pending(&root)? {
        return Err(format!("rekey generation {n} is incomplete; writers stay stopped: repeat `vega post identity rekey apply` with its plan, or `vega post identity rekey rollback`"));
    }
    let bundle: Bundle = read(&root.join(BUNDLE))?;
    validate_bundle(
        &bundle,
        &bundle
            .hosts
            .iter()
            .map(|p| p.host.clone())
            .collect::<Vec<_>>(),
    )?;
    let host = crate::host();
    let approved = bundle
        .hosts
        .iter()
        .find(|p| p.host == host)
        .ok_or("offline/unplanned host: obtain a new audited bundle before rejoin")?;
    if approved.inbox_only != vega_herdr::state::is_inbox_only(&host) {
        return Err("host role changed since audit".into());
    }
    let p = prepare_startup_files(&root, approved, &host)?;
    if root.join(MARKER).exists() {
        let marker: Value = read(&root.join(MARKER))?;
        if marker != serde_json::json!({"schema":2,"host":host}) {
            return Err("invalid completion marker".into());
        }
        // Check an online rejoin before workers start, while retaining local
        // mode for completed hosts whose broker is unreachable. The recurring
        // registry guard also checks a later reconnect before publication.
        match crate::connect("identity rejoin", None).await {
            Ok(c) if !p.inbox_only => {
                let kv = vega_registry::bucket(&c).await?;
                let entries = raw_entries(&kv).await?;
                check_live(&entries, &bundle, now())?;
                reject_legacy(&entries)?;
            }
            Ok(_) => {}
            Err(e) if e.code == "broker_unreachable" => {}
            Err(e) => return Err(e.message),
        }
        return Ok(());
    }
    let c = vega_registry::connect("identity startup").await?;
    if p.inbox_only {
        // Inbox-only restricts broker publication, not local project records.
        apply_files(&root, &p)?;
        for key in &p.registry_reads {
            let expected = bundle
                .hosts
                .iter()
                .find_map(|p| p.registry.projection.get(key))
                .ok_or("unplanned read")?;
            if vega_registry::get_one(&c, key).await?.as_ref() != Some(expected) {
                return Err(format!("exact v2 direct get failed: {key}"));
            }
        }
        // A timeout is not proof of a denied publish. Require the broker's
        // explicit, subject-specific permission event on an observed connection.
        let (tx, mut errors) = tokio::sync::mpsc::unbounded_channel();
        let probe = vega_registry::connect_with("identity grant probes", |options| {
            options.event_callback(move |event| {
                let tx = tx.clone();
                async move {
                    let _ = tx.send(event);
                }
            })
        })
        .await?;
        for subject in [
            format!("$JS.API.STREAM.INFO.KV_{}", vega_registry::BUCKET),
            vega_registry::direct_get_subject("identity-cutover.ungranted"),
        ] {
            require_denial(&probe, &mut errors, &subject).await?;
        }
    } else {
        let kv = vega_registry::bucket(&c).await?;
        let live = raw_entries(&kv).await?;
        check_live(&live, &bundle, now())?;
        // An original key may already be absent after an interruption. Existing
        // owned values must still equal either original or planned payloads.
        for (key, value) in &live {
            if key.starts_with(&format!("{host}."))
                && (vega_registry::is_legacy_project_key(key)
                    || matches!(key.split('.').nth(1), Some("uac" | "lanista")))
                && p.registry.backup.get(key) != Some(value)
                && p.registry.projection.get(key) != Some(value)
            {
                return Err(format!("owned registry changed since preview: {key}"));
            }
        }
        apply_files(&root, &p)?;
        save(
            &root.join("identity-cutover/progress.json"),
            &serde_json::json!({"schema":2,"phase":"files_applied"}),
        )?;
        p.registry.apply(&kv).await?;
        p.registry.verify(&c, &kv).await?;
        save(
            &root.join("identity-cutover/progress.json"),
            &serde_json::json!({"schema":2,"phase":"registry_verified"}),
        )?;
        // Agents/notes have stable pane/source keys and reconcile from migrated
        // local state. Publish and verify them before releasing project workers.
        let agents = vega_herdr::herdr(&["agent", "list"]).map_err(|e| e.message)?;
        let workspaces = vega_herdr::herdr(&["workspace", "list"]).map_err(|e| e.message)?;
        let mut desired = vega_registry::local_entries(
            &host,
            agents["result"]["agents"]
                .as_array()
                .ok_or("invalid agent list")?,
            workspaces["result"]["workspaces"]
                .as_array()
                .ok_or("invalid workspace list")?,
        );
        // Advertise this owner's verified v2 projection even while ordinary
        // reconciliation is held by other hosts' remaining legacy entries.
        desired.insert(
            vega_registry::beat_key(&host),
            serde_json::json!({"schema":2,"at":now()}),
        );
        for (key, value) in &desired {
            kv.put(key, value.to_string().into())
                .await
                .map_err(|e| e.to_string())?;
            if vega_registry::get_one(&c, key).await?.as_ref() != Some(value) {
                return Err(format!("v2 republish failed: {key}"));
            }
        }
    }
    save(
        &root.join(MARKER),
        &serde_json::json!({"schema":2,"host":host}),
    )?;
    Ok(())
}

/// NATS reports denied publishes asynchronously. Only an explicit denial of
/// this exact subject proves the negative check; disconnects/timeouts fail.
async fn require_denial(
    client: &async_nats::Client,
    events: &mut tokio::sync::mpsc::UnboundedReceiver<async_nats::Event>,
    subject: &str,
) -> Result<(), String> {
    while events.try_recv().is_ok() {}
    let reply = client.request(subject.to_owned(), "{}".into());
    tokio::pin!(reply);
    let result = tokio::time::timeout(std::time::Duration::from_secs(4), async {
        loop {
            tokio::select! {
                result = &mut reply => return Err(format!("no explicit permission denial for {subject}: {}",if result.is_ok(){"request was allowed"}else{"request failed or timed out"})),
                event = events.recv() => match event {
                    Some(async_nats::Event::ServerError(async_nats::ServerError::Other(message)))
                        if message.to_lowercase().contains("permissions violation") && message.contains(&format!("Publish to \"{subject}\"")) => return Ok(()),
                    Some(async_nats::Event::Disconnected | async_nats::Event::Closed | async_nats::Event::ClientError(_)) | None => return Err(format!("connection failed while verifying denial of {subject}")),
                    Some(_) => {},
                }
            }
        }
    }).await;
    result.map_err(|_| format!("timed out verifying permission denial of {subject}"))?
}

/// Refuse publication when an old or unplanned host comes back online.
pub fn guard(entries: &BTreeMap<String, Value>) -> Result<(), String> {
    let root = crate::service::state_dir().map_err(|e| e.message)?;
    let bundle: Bundle = read(&root.join(BUNDLE))?;
    check_live(entries, &bundle, now())?;
    reject_legacy(entries)
}

fn reject_legacy(entries: &BTreeMap<String, Value>) -> Result<(), String> {
    if let Some(key) = entries
        .keys()
        .find(|key| vega_registry::is_legacy_project_key(key))
    {
        return Err(format!("legacy project key active after completion: {key}"));
    }
    Ok(())
}

// Do two NATS subject patterns overlap? Used to reject broad grants, including
// patterns that do not literally spell VEGA_REGISTRY (e.g. $JS.API.>).
fn overlaps(a: &str, b: &str) -> bool {
    let a: Vec<_> = a.split('.').collect();
    let b: Vec<_> = b.split('.').collect();
    for i in 0..a.len().min(b.len()) {
        if a[i] == ">" || b[i] == ">" {
            return true;
        }
        if a[i] != "*" && b[i] != "*" && a[i] != b[i] {
            return false;
        }
    }
    a.len() == b.len()
}

fn validate_grants(bundle: &Bundle) -> Result<(), String> {
    let sensitive = [
        "$KV.VEGA_REGISTRY.>",
        "$JS.API.DIRECT.GET.KV_VEGA_REGISTRY",
        "$JS.API.DIRECT.GET.KV_VEGA_REGISTRY.>",
        "$JS.API.STREAM.*.KV_VEGA_REGISTRY",
        "$JS.API.STREAM.*.*.KV_VEGA_REGISTRY",
        "$JS.API.CONSUMER.*.KV_VEGA_REGISTRY",
        "$JS.API.CONSUMER.*.KV_VEGA_REGISTRY.>",
        "$JS.API.CONSUMER.DURABLE.CREATE.KV_VEGA_REGISTRY.>",
        "$JS.API.CONSUMER.MSG.NEXT.KV_VEGA_REGISTRY.>",
        "$JS.API.CONSUMER.LEADER.STEPDOWN.KV_VEGA_REGISTRY.>",
        "$JS.API.STREAM.LIST",
        "$JS.API.STREAM.NAMES",
    ];
    for p in bundle.hosts.iter().filter(|p| p.inbox_only) {
        let allow = bundle
            .grants
            .get(&p.host)
            .ok_or_else(|| format!("missing orrery effective publish allowlist for {}", p.host))?;
        let expected: BTreeSet<_> = p
            .registry_reads
            .iter()
            .map(|key| vega_registry::direct_get_subject(key))
            .collect();
        let mut actual = BTreeSet::new();
        for subject in &allow.subscribe {
            if overlaps(subject, "$KV.VEGA_REGISTRY.>") {
                return Err(format!("inbox-only registry watch subscription: {subject}"));
            }
        }
        for subject in &allow.publish {
            if sensitive.iter().any(|pattern| overlaps(subject, pattern)) {
                // Exact theme reads are unchanged by the project cutover.
                let prefix = "$JS.API.DIRECT.GET.KV_VEGA_REGISTRY.$KV.VEGA_REGISTRY.";
                if let Some(key) = subject.strip_prefix(prefix) {
                    if key.split('.').count() == 2
                        && key.ends_with(".theme")
                        && crate::auth::parse_registry_read(&format!("{}={key}", p.host)).is_ok()
                    {
                        continue;
                    }
                }
                if !expected.contains(subject) {
                    return Err(format!(
                        "obsolete, wildcard or excessive inbox-only registry grant: {subject}"
                    ));
                }
                actual.insert(subject.clone());
            }
        }
        if actual != expected {
            return Err(format!("missing exact v2 registry grants for {}", p.host));
        }
    }
    Ok(())
}

/// Plans whose flag-day projection `verify` compares: all, or only the named
/// (late) hosts. Every named host must be in the audited bundle.
fn projection_hosts<'a>(bundle: &'a Bundle, hosts: &[String]) -> Result<Vec<&'a HostPlan>, String> {
    if let Some(h) = hosts
        .iter()
        .find(|h| !bundle.hosts.iter().any(|p| &p.host == *h))
    {
        return Err(format!("host {h} is not in the audited bundle"));
    }
    Ok(bundle
        .hosts
        .iter()
        .filter(|p| hosts.is_empty() || hosts.contains(&p.host))
        .collect())
}

fn verify_deployment(
    entries: &BTreeMap<String, Value>,
    bundle: &Bundle,
    now: i64,
) -> Result<(), String> {
    validate_bundle(
        bundle,
        &bundle
            .hosts
            .iter()
            .map(|p| p.host.clone())
            .collect::<Vec<_>>(),
    )?;
    check_live(entries, bundle, now)?;
    reject_legacy(entries)?;
    for p in bundle.hosts.iter().filter(|p| !p.inbox_only) {
        let beat = entries
            .get(&vega_registry::beat_key(&p.host))
            .ok_or_else(|| format!("{} has not republished", p.host))?;
        if beat["schema"] != 2
            || beat["at"]
                .as_i64()
                .is_none_or(|at| now - at > vega_registry::FRESH.as_secs() as i64)
        {
            return Err(format!("{} is offline or not v2", p.host));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture(host: &str, pane: &str) -> HostPlan {
        let m:Mapping = serde_json::from_value(json!({"old_key":"~/repo/.git", "project":"github.com/team/repo", "location":{"common_dir":"/repo/.git","checkout":"/repo"}})).unwrap();
        let original = Snapshot::from([
            ("uac/local_~_repo_.git".into(),serde_json::to_vec(&json!({"project":m.old_key,"host":host,"pane":pane,"at":7,"tracker":"task"})).unwrap()),
            ("notes/9.json".into(),serde_json::to_vec(&json!({"id":9,"host":host,"project":m.old_key,"cause":"bd-sync:DB","resolved":null,"title":"stopped","body":"conflict"})).unwrap()),
        ]);
        let mappings = vec![m.clone()];
        let (writes, remove) = local_plans(&original, &mappings, host).unwrap();
        let ids = BTreeMap::from([(m.old_key.clone(), m.project.clone())]);
        let registry = RegistryPlan::prepare(host,&BTreeMap::from([
            (format!("{host}.uac.old"),json!({"project":m.old_key,"host":host,"pane":pane,"at":7})),
            (format!("{host}.lanista.old"),json!({"project":m.old_key,"seat":"s","lane":"l","tier":"t","model":null,"at":3,"expires":1})),
        ]),&ids,BTreeMap::new()).unwrap();
        HostPlan {
            schema: 2,
            host: host.into(),
            inbox_only: false,
            mappings,
            original,
            writes,
            remove,
            registry,
            owned: BTreeMap::new(),
            registry_reads: vec![],
        }
    }
    fn bundle(p: HostPlan) -> Bundle {
        Bundle {
            schema: 2,
            hosts: vec![p],
            grants: BTreeMap::new(),
        }
    }

    #[test]
    fn interrupted_apply_replays_durable_original_then_rolls_back_exact_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let p = fixture("a", "p1");
        for (path, bytes) in &p.original {
            atomic(&root.join(path), bytes).unwrap();
        }
        assert_eq!(snapshot(root).unwrap(), p.original);
        save(&root.join(JOURNAL), &p).unwrap();
        // Power loss after the first target write, before legacy deletions.
        let (path, bytes) = p.writes.first_key_value().unwrap();
        atomic(&root.join(path), bytes).unwrap();
        let recovered: HostPlan = read(&root.join(JOURNAL)).unwrap();
        assert_eq!(recovered.original, p.original);
        apply_files(root, &recovered).unwrap();
        let migrated = snapshot(root).unwrap();
        apply_files(root, &recovered).unwrap();
        assert_eq!(snapshot(root).unwrap(), migrated);
        assert_eq!(
            read::<Value>(&root.join("notes/9.json")).unwrap()["cause"],
            "bd-sync:DB"
        );
        // Rollback itself is also replayable after a partial restoration.
        atomic(&root.join("identity-cutover/rollback"), b"rollback").unwrap();
        restore_files(root, &recovered).unwrap();
        restore_files(root, &recovered).unwrap();
        assert_eq!(snapshot(root).unwrap(), p.original);
        assert!(root.join(JOURNAL).exists());
        assert!(!root.join(MARKER).exists());
    }

    #[test]
    fn inbox_only_local_records_migrate_without_registry_publication() {
        let mut p = fixture("deimos", "p1");
        p.inbox_only = true;
        p.registry = RegistryPlan::prepare(
            "deimos",
            &BTreeMap::new(),
            &BTreeMap::new(),
            BTreeMap::new(),
        )
        .unwrap();
        // Real inbox host also has local badge data; its role forbids only KV writes.
        p.original.insert("badges/__repo_.git.json".into(), serde_json::to_vec(&json!({"project":"~/repo/.git","at":7,"notes":0,"decisions":0,"waiting":0,"waiting_panes":[]})).unwrap());
        (p.writes, p.remove) = local_plans(&p.original, &p.mappings, "deimos").unwrap();
        let mut b = bundle(p);
        b.grants.insert(
            "deimos".into(),
            Grants {
                publish: vec![],
                subscribe: vec![],
            },
        );
        validate_bundle(&b, &["deimos".into()]).unwrap();
        let root = tempfile::tempdir().unwrap();
        let p = &b.hosts[0];
        for (path, bytes) in &p.original {
            atomic(&root.path().join(path), bytes).unwrap();
        }
        apply_files(root.path(), p).unwrap();
        assert!(root.path().join("uac/v2").is_dir());
        assert!(root.path().join("badges/v2").is_dir());
        restore_files(root.path(), p).unwrap();
        assert_eq!(snapshot(root.path()).unwrap(), p.original);
        b.hosts[0]
            .owned
            .insert("deimos.beat".into(), json!({"at":7}));
        assert!(validate_bundle(&b, &["deimos".into()])
            .unwrap_err()
            .contains("registry writer"));
    }

    /// Opt-in rehearsal on owner-local state copies and an audited real bundle.
    /// Only file migration runs: all registry operations are explicitly skipped.
    #[test]
    #[ignore = "requires VEGA_IDENTITY_DRYRUN_ROOT and VEGA_IDENTITY_DRYRUN_BUNDLE"]
    fn real_copy_startup_and_rollback_without_broker() {
        let root = PathBuf::from(std::env::var_os("VEGA_IDENTITY_DRYRUN_ROOT").expect("copy root"));
        assert!(root.is_absolute());
        assert_ne!(
            root.canonicalize().unwrap(),
            PathBuf::from(std::env::var_os("HOME").unwrap()).join(".local/state/vega")
        );
        let b: Bundle = read(&PathBuf::from(
            std::env::var_os("VEGA_IDENTITY_DRYRUN_BUNDLE").expect("audited bundle"),
        ))
        .unwrap();
        validate_bundle(
            &b,
            &b.hosts.iter().map(|p| p.host.clone()).collect::<Vec<_>>(),
        )
        .unwrap();
        let host = crate::host();
        let approved = b.hosts.iter().find(|p| p.host == host).unwrap();
        let original = snapshot(&root).unwrap();
        let p = prepare_startup_files(&root, approved, &host).unwrap();
        // Interrupt after the first target, then replay from the same journal.
        if let Some((path, bytes)) = p.writes.first_key_value() {
            atomic(&root.join(path), bytes).unwrap();
        }
        let recovered = prepare_startup_files(&root, approved, &host).unwrap();
        apply_files(&root, &recovered).unwrap();
        let migrated = snapshot(&root).unwrap();
        apply_files(&root, &recovered).unwrap();
        assert_eq!(snapshot(&root).unwrap(), migrated);
        save(&root.join(MARKER), &json!({"schema":2,"host":host})).unwrap();
        prepare_startup_files(&root, approved, &host).unwrap();
        atomic(&root.join("identity-cutover/rollback"), b"rollback\n").unwrap();
        restore_files(&root, &recovered).unwrap();
        restore_files(&root, &recovered).unwrap();
        delete(&root.join(MARKER)).unwrap();
        assert_eq!(snapshot(&root).unwrap(), original);
        assert!(root.join(JOURNAL).exists());
        assert!(!root.join(MARKER).exists());
        assert!(prepare_startup_files(&root, approved, &host)
            .err()
            .unwrap()
            .contains("rollback"));
    }

    #[test]
    fn audit_blocks_missing_hosts_conflicts_mixed_versions_and_unsafe_paths() {
        let mut b = bundle(fixture("a", "p1"));
        validate_bundle(&b, &["a".into()]).unwrap();
        assert!(validate_bundle(&b, &["a".into(), "offline".into()]).is_err());
        b.hosts.push(fixture("b", "p2"));
        assert!(validate_bundle(&b, &["a".into(), "b".into()])
            .unwrap_err()
            .contains("duplicate claim"));
        b.hosts.pop();
        b.hosts[0].schema = 1;
        assert!(validate_bundle(&b, &["a".into()]).is_err());
        b.hosts[0].schema = 2;
        b.hosts[0].writes.insert("../outside".into(), vec![]);
        assert!(validate_bundle(&b, &["a".into()]).is_err());
    }

    #[test]
    fn old_or_unplanned_offline_host_cannot_rejoin_and_completion_needs_every_owner() {
        let b = bundle(fixture("a", "p1"));
        let mut entries = BTreeMap::from([("a.beat".into(), json!({"schema":2,"at":100}))]);
        verify_deployment(&entries, &b, 100).unwrap();
        let (legacy, value) = b.hosts[0].registry.backup.first_key_value().unwrap();
        entries.insert(legacy.clone(), value.clone());
        check_live(&entries, &b, 100).unwrap(); // migration may see planned legacy
        assert!(reject_legacy(&entries).is_err()); // completed publication may not
        entries.remove(legacy);
        entries.insert("old.beat".into(), json!({"schema":2,"at":100}));
        assert!(check_live(&entries, &b, 100).is_err());
        entries.remove("old.beat");
        entries.insert("a.beat".into(), json!({"at":100}));
        assert!(check_live(&entries, &b, 100).is_err());
        entries.insert("a.beat".into(), json!({"schema":2,"at":1}));
        assert!(verify_deployment(&entries, &b, 1000).is_err());
        entries.insert("unplanned.uac.old".into(), json!({"project":"/old"}));
        assert!(check_live(&entries, &b, 1000).is_err());
    }

    #[test]
    fn verify_host_limits_projection_check_to_named_audited_hosts() {
        let mut b = bundle(fixture("a", "p1"));
        b.hosts.push(fixture("late", "p2"));
        let all: Vec<_> = projection_hosts(&b, &[])
            .unwrap()
            .iter()
            .map(|p| p.host.clone())
            .collect();
        assert_eq!(all, ["a", "late"]);
        let late = projection_hosts(&b, &["late".into()]).unwrap();
        assert_eq!(late.len(), 1);
        assert_eq!(late[0].host, "late");
        assert!(projection_hosts(&b, &["io-wsl".into()])
            .err()
            .is_some_and(|e| e.contains("not in the audited bundle")));
    }

    #[test]
    fn exact_grants_audit_rejects_old_keys_wildcards_writes_and_enumeration() {
        let mut b = bundle(fixture("a", "p1"));
        let key = b.hosts[0]
            .registry
            .projection
            .keys()
            .find(|k| k.contains(".uac."))
            .unwrap()
            .clone();
        let subject = vega_registry::direct_get_subject(&key);
        let mut inbox = fixture("reader", "p2");
        inbox.original.clear();
        inbox.writes.clear();
        inbox.remove.clear();
        inbox.mappings.clear();
        inbox.registry = RegistryPlan::prepare(
            "reader",
            &BTreeMap::new(),
            &BTreeMap::new(),
            BTreeMap::new(),
        )
        .unwrap();
        inbox.inbox_only = true;
        inbox.registry_reads = vec![key];
        b.hosts.push(inbox);
        b.grants.insert(
            "reader".into(),
            Grants {
                publish: vec![
                    subject.clone(),
                    "$JS.API.STREAM.INFO.VEGA_INBOX_reader".into(),
                    // Real orrery grant shape: the stream is the token after
                    // the operation, not an arbitrary later token.
                    "$JS.API.CONSUMER.*.VEGA_INBOX_reader.>".into(),
                    "$JS.API.CONSUMER.MSG.NEXT.VEGA_INBOX_reader.*".into(),
                ],
                subscribe: vec!["_INBOX_reader.>".into()],
            },
        );
        validate_bundle(&b, &["a".into(), "reader".into()]).unwrap();
        // An authorized subset may be empty: explicit live denial events
        // prove connectivity without provisioning a new project read grant.
        let saved = b.hosts[1].registry_reads.clone();
        let saved_publish = b.grants["reader"].publish.clone();
        b.hosts[1].registry_reads.clear();
        b.grants.get_mut("reader").unwrap().publish.clear();
        validate_bundle(&b, &["a".into(), "reader".into()]).unwrap();
        b.hosts[1].registry_reads = saved;
        b.grants.get_mut("reader").unwrap().publish = saved_publish;
        for bad in [
            ">",
            "$JS.API.>",
            "$KV.VEGA_REGISTRY.reader.>",
            "$JS.API.STREAM.INFO.KV_VEGA_REGISTRY",
            "$JS.API.DIRECT.GET.KV_VEGA_REGISTRY.>",
            "$JS.API.DIRECT.GET.KV_VEGA_REGISTRY.$KV.VEGA_REGISTRY.a.uac.old",
            "$JS.API.CONSUMER.CREATE.KV_VEGA_REGISTRY.watch",
            "$JS.API.CONSUMER.CREATE.KV_VEGA_REGISTRY",
            "$JS.API.CONSUMER.DURABLE.CREATE.KV_VEGA_REGISTRY.watch",
            "$JS.API.CONSUMER.MSG.NEXT.KV_VEGA_REGISTRY.watch",
            "$JS.API.CONSUMER.LEADER.STEPDOWN.KV_VEGA_REGISTRY.watch",
        ] {
            b.grants.get_mut("reader").unwrap().publish.push(bad.into());
            assert!(validate_grants(&b).is_err(), "accepted {bad}");
            b.grants.get_mut("reader").unwrap().publish.pop();
        }
        b.grants.get_mut("reader").unwrap().publish.remove(0);
        assert!(validate_grants(&b).is_err());
    }

    #[test]
    fn denied_probe_requires_explicit_subject_permission_error_not_timeout_or_reply() {
        let dir = tempfile::tempdir().unwrap();
        let auth = r#"authorization {
            users: [
                {user: "reader", password: "test", permissions: {publish: {allow: ["safe"]}, subscribe: {allow: ["_INBOX.>"]}}},
                {user: "owner", password: "test"}
            ]
        }"#;
        let (_server, url) = crate::registry_read_test::start(dir.path(), auth);
        crate::runtime().unwrap().block_on(async {
            // Wait for this private server without touching global credentials.

            let mut owner = None;
            for _ in 0..50 {
                if let Ok(c) = async_nats::ConnectOptions::with_user_and_password(
                    "owner".into(),
                    "test".into(),
                )
                .connect(&url)
                .await
                {
                    owner = Some(c);
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            let owner = owner.unwrap();
            for (name, allowed) in [("reader", false), ("owner", true)] {
                let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
                let c =
                    async_nats::ConnectOptions::with_user_and_password(name.into(), "test".into())
                        .request_timeout(Some(std::time::Duration::from_millis(300)))
                        .event_callback(move |event| {
                            let tx = tx.clone();
                            async move {
                                let _ = tx.send(event);
                            }
                        })
                        .connect(&url)
                        .await
                        .unwrap();
                let result =
                    require_denial(&c, &mut events, "$JS.API.STREAM.INFO.KV_VEGA_REGISTRY").await;
                assert_eq!(result.is_err(), allowed, "{name}: {result:?}");
                if allowed {
                    // A real subscriber deliberately does not answer: this is
                    // a timeout rather than an implicit NoResponders reply.
                    let _silent = owner.subscribe("stall").await.unwrap();
                    owner.flush().await.unwrap();
                    assert!(require_denial(&c, &mut events, "stall")
                        .await
                        .unwrap_err()
                        .contains("no explicit permission denial"));
                }
            }
        });
    }

    #[test]
    fn registry_interrupted_republish_and_rollback_keep_pins_and_tombstones() {
        let dir = tempfile::tempdir().unwrap();
        let (_server, url) = crate::registry_read_test::start(dir.path(), "");
        crate::runtime().unwrap().block_on(async {
            let c = crate::registry_read_test::connect(
                &url,
                &nkeys::KeyPair::new_user().seed().unwrap(),
                "_INBOX_test",
            )
            .await;
            let kv = async_nats::jetstream::new(c.clone())
                .create_key_value(async_nats::jetstream::kv::Config {
                    bucket: vega_registry::BUCKET.into(),
                    history: 1,
                    ..Default::default()
                })
                .await
                .unwrap();
            let p = fixture("a", "p1");
            for (key, value) in &p.registry.backup {
                kv.put(key, value.to_string().into()).await.unwrap();
            }
            // Crash after one source deletion; replay the original durable plan.
            kv.delete(p.registry.backup.keys().next().unwrap())
                .await
                .unwrap();
            p.registry.apply(&kv).await.unwrap();
            p.registry.apply(&kv).await.unwrap();
            p.registry.verify(&c, &kv).await.unwrap();
            let pin = p
                .registry
                .projection
                .iter()
                .find(|(k, _)| k.contains(".lanista."))
                .unwrap();
            assert_eq!(
                vega_registry::get_one(&c, pin.0).await.unwrap().unwrap()["expires"],
                1
            );
            p.registry.rollback(&kv).await.unwrap();
            p.registry.rollback(&kv).await.unwrap();
            assert_eq!(raw_entries(&kv).await.unwrap(), p.registry.backup);
        });
    }
}
