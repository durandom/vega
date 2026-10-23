//! Machine identities: `keygen` makes this machine's nkey seed, which never
//! leaves the machine; `auth-config` turns public keys into the server's
//! `authorization` block.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use vega_herdr::{collapse_home, Failure};

use crate::{
    host, inbox_prefix, seed_path, stream_name, Seed, ADMIN_INBOX, ARCHIVE_MAX_BYTES, VIEWER_INBOX,
};

/// Hosts that fit orrery's `max_file_store: 8GB` at ARCHIVE_MAX_BYTES each:
/// every host stream reserves its `max_bytes` up front.
pub const MAX_HOSTS_8GB: usize = 8;

/// Creates the seed if missing (0600, never overwritten) and prints the
/// public key. Running it again prints the same key.
pub fn keygen(kind: Seed) -> Result<(), Failure> {
    let path = seed_path(kind)?;
    let p = std::path::Path::new(&path);
    let created = if p.exists() {
        false
    } else {
        if let Some(dir) = p.parent() {
            fs::create_dir_all(dir).map_err(|e| io_err(&path, e))?;
            let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
        }
        let kp = nkeys::KeyPair::new_user();
        let seed = kp.seed().map_err(|e| key_err(&path, e))?;
        // create_new: never overwrite, even if another keygen raced us.
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(p)
            .map_err(|e| io_err(&path, e))?;
        writeln!(f, "{seed}").map_err(|e| io_err(&path, e))?;
        true
    };
    let mode = fs::metadata(p)
        .map_err(|e| io_err(&path, e))?
        .permissions()
        .mode()
        & 0o777;
    let seed = fs::read_to_string(p).map_err(|e| io_err(&path, e))?;
    let kp = nkeys::KeyPair::from_seed(seed.trim()).map_err(|e| key_err(&path, e))?;
    if !kp.public_key().starts_with('U') {
        return Err(Failure::new(
            "not_a_user_key",
            format!("{} holds a non-user nkey", collapse_home(&path)),
            "move it away and run `vega post keygen` again",
        ));
    }

    let key = kp.public_key();
    match kind {
        Seed::Host => println!("host: {}", host()),
        Seed::Admin => println!("admin: {key}"),
    }
    println!("public_key: {key}");
    println!(
        "seed: {} ({}, mode {mode:o})",
        collapse_home(&path),
        if created { "created" } else { "exists" }
    );
    if mode & 0o077 != 0 {
        println!("warning: the seed is readable by others; run `chmod 600 {path}`");
    }
    match kind {
        Seed::Host => println!(
            "help: add `{}={key}` to the server's auth config (`vega post auth-config`); the seed never leaves this machine",
            host()
        ),
        Seed::Admin => println!(
            "help: pass `--admin {key}` to `vega post auth-config`; `vega post provision` uses this seed; it never leaves this machine"
        ),
    }
    Ok(())
}

fn io_err(path: &str, e: std::io::Error) -> Failure {
    Failure::new(
        "seed_io",
        format!("{}: {e}", collapse_home(path)),
        "check VEGA_NATS_NKEY / XDG_CONFIG_HOME and the directory permissions",
    )
}

fn key_err(path: &str, e: impl std::fmt::Display) -> Failure {
    Failure::new(
        "bad_seed",
        format!("{}: {e}", collapse_home(path)),
        "the file is not an nkey seed; move it away and run `vega post keygen` again",
    )
}

/// One `host=U...` entry, validated.
#[derive(Debug, PartialEq)]
pub struct Entry {
    pub host: String,
    pub key: String,
}

pub fn parse_entry(s: &str) -> Result<Entry, Failure> {
    let bad = |why: &str| {
        Failure::new(
            "usage",
            format!("`{s}`: {why}"),
            "pass host=U... as printed by `vega post keygen`",
        )
    };
    let (host, key) = s.trim().split_once('=').ok_or_else(|| bad("missing ="))?;
    let (host, key) = (host.trim(), key.trim());
    if host.is_empty()
        || !host
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        return Err(bad("host must be [a-z0-9_-]+"));
    }
    if !key.starts_with('U') || nkeys::KeyPair::from_public_key(key).is_err() {
        return Err(bad("not a user public key (U...)"));
    }
    Ok(Entry {
        host: host.into(),
        key: key.into(),
    })
}

/// Entries from args plus `--file` lines (`#` comments and blank lines skipped).
pub fn read_entries(args: &[String], file: Option<&str>) -> Result<Vec<Entry>, Failure> {
    let mut lines: Vec<String> = args.to_vec();
    if let Some(f) = file {
        let text = fs::read_to_string(f).map_err(|e| {
            Failure::new(
                "usage",
                format!("{f}: {e}"),
                "pass a file of host=U... lines",
            )
        })?;
        lines.extend(
            text.lines()
                .map(|l| l.split('#').next().unwrap_or("").trim().to_string())
                .filter(|l| !l.is_empty()),
        );
    }
    let entries = lines
        .iter()
        .map(|l| parse_entry(l))
        .collect::<Result<Vec<_>, _>>()?;
    if entries.is_empty() {
        return Err(Failure::new(
            "usage",
            "no host=U... entries",
            "vega post auth-config proteus=U... ganymede=U... --admin U...",
        ));
    }
    if let Some(e) = entries
        .iter()
        .find(|e| e.host == "admin" || e.host == "viewer")
    {
        return Err(Failure::new(
            "usage",
            format!("host name {} is reserved", e.host),
            "admin and viewer have their own reply prefixes (_INBOX_admin, _INBOX_viewer); rename the host",
        ));
    }
    for (i, a) in entries.iter().enumerate() {
        if let Some(b) = entries[i + 1..]
            .iter()
            .find(|b| b.host == a.host || b.key == a.key)
        {
            return Err(Failure::new(
                "usage",
                format!("{} and {} share a host or key", a.host, b.host),
                "one key per host, one host per key",
            ));
        }
    }
    Ok(entries)
}

/// A `--cmd-grant host=subject` entry: the subject must be one exact command
/// subject `vega.cmd.<to>.<kind>.<host>` sent as that host, no wildcards.
pub fn parse_grant(s: &str) -> Result<(String, String), Failure> {
    let bad = |why: &str| {
        Failure::new(
            "usage",
            format!("--cmd-grant {s}: {why}"),
            "use host=vega.cmd.<to>.<kind>.<host>, e.g. deimos=vega.cmd.proteus.open.deimos",
        )
    };
    let (host, subject) = s.split_once('=').ok_or_else(|| bad("no ="))?;
    let t: Vec<&str> = subject.split('.').collect();
    if t.len() != 5 || t[0] != "vega" || t[1] != "cmd" {
        return Err(bad("not vega.cmd.<to>.<kind>.<host>"));
    }
    if t.iter().any(|x| x.is_empty() || x.contains(['*', '>'])) {
        return Err(bad("wildcards and empty tokens are not allowed"));
    }
    if t[4] != host {
        return Err(bad("the last token must be the granted host"));
    }
    Ok((host.to_string(), subject.to_string()))
}

/// A `--registry-read host=key` entry: one exact registry key, no wildcards
/// (vega-lhc.90), e.g. deimos=proteus.theme.
pub fn parse_registry_read(s: &str) -> Result<(String, String), Failure> {
    let bad = |why: &str| {
        Failure::new(
            "usage",
            format!("--registry-read {s}: {why}"),
            "use host=<exact-key>: <writer>.uac.v2.<hex(ProjectId)> or <writer>.lanista.v2.<hex(ProjectId)>.<hex(seat)>.<hex(lane)>.<hex(tier)>; theme: deimos=proteus.theme",
        )
    };
    let (host, key) = s.split_once('=').ok_or_else(|| bad("no ="))?;
    let ok = |t: &str| {
        !t.is_empty()
            && t.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    };
    if !ok(host) || !key.split('.').all(ok) {
        return Err(bad(
            "the key must be dot-separated [A-Za-z0-9_-] tokens, no wildcards",
        ));
    }
    let parts: Vec<_> = key.split('.').collect();
    if matches!(parts.get(1), Some(&"uac" | &"lanista")) {
        let count = if parts[1] == "uac" { 4 } else { 7 };
        if parts.len() != count
            || parts.get(2) != Some(&"v2")
            || parts[3..]
                .iter()
                .any(|p| vega_herdr::project::decode_component(p).is_err())
        {
            return Err(bad(
                "project keys require v2 and reversible UTF-8 hex components",
            ));
        }
    }
    Ok((host.to_string(), key.to_string()))
}

/// The `authorization` block for nats.conf. Per host: publish into any inbox
/// only as itself (last token), read only its own stream.
/// `inbox_only` hosts receive and read their own mail but may send none: no
/// `vega.inbox` publish at all. For machines that must not instruct the
/// others' agents (e.g. a corp-managed laptop).
/// `quarantine` hosts (inbox-only ones) may also reply into
/// `vega.quarantine.*.*.<self>`, which the other hosts serve as notes for
/// the human, never as mail (quarantine.rs).
/// `registry_read` (inbox-only hosts) grants one last-by-subject direct get
/// of one registry key each; no stream info, put, delete or watch.
/// Every host reads the lanista catalog by direct get (catalog.rs, no
/// consumer); `lanista_writer` (not inbox-only) also puts it.
#[allow(clippy::too_many_arguments)]
pub fn render(
    entries: &[Entry],
    admin: Option<&str>,
    create: &[String],
    inbox_only: &[String],
    viewer: Option<&str>,
    cmd_grants: &[(String, String)],
    quarantine: &[String],
    registry_read: &[(String, String)],
    lanista_writer: Option<&str>,
) -> String {
    let gib = ARCHIVE_MAX_BYTES / (1 << 30);
    let need = entries.len() as i64 * gib;
    let hosts: Vec<&str> = entries.iter().map(|e| e.host.as_str()).collect();
    let mut o = String::new();
    o.push_str("# vega post: generated by `vega post auth-config`; do not edit by hand.\n");
    o.push_str(&format!("# hosts: {}\n", hosts.join(", ")));
    o.push_str(&format!(
        "# JetStream: each VEGA_INBOX_<host> stream reserves {gib} GiB, so max_file_store must be >= {need} GiB.\n"
    ));
    o.push_str("# Streams are created by the admin user (`vega post provision <host>...`); machine keys cannot.\n");
    o.push_str("authorization {\n  users: [\n");
    if let Some(a) = admin {
        o.push_str(&render_admin(a, entries, create));
    }
    if let Some(v) = viewer {
        o.push_str(&render_viewer(v, entries));
    }
    for e in entries {
        let h = &e.host;
        // Commands (vegad M3b): a host requests as itself and serves its own
        // subject; replies go back through `allow_responses`. An inbox-only
        // host may send exactly the command subjects granted to it.
        let (note, send, serve, responses) = if inbox_only.contains(h) {
            let mut grants: String = cmd_grants
                .iter()
                .filter(|(g, _)| g == h)
                .map(|(_, s)| format!("\n          \"{s}\","))
                .collect();
            let mut note = if grants.is_empty() {
                " (inbox only: may not send)".to_string()
            } else {
                " (inbox only: may not send mail; granted commands only)".to_string()
            };
            // Quarantine (vega-lhc.83): replies as itself into a subject space
            // no relay delivers to agents; the target's vegad notes them for
            // the human. Request/reply: the answer comes to `_INBOX_<h>`.
            if quarantine.contains(h) {
                grants.push_str(&format!("\n          \"vega.quarantine.*.*.{h}\","));
                note =
                    " (inbox only: may not send mail; replies into quarantine, for the human only)"
                        .into();
            }
            // Registry (vega-lhc.90): exactly the granted keys, read-only.
            for (_, k) in registry_read.iter().filter(|(g, _)| g == h) {
                grants.push_str(&format!(
                    "\n          \"{}\",",
                    vega_registry::direct_get_subject(k)
                ));
                note.insert_str(note.len() - 1, &format!("; reads registry key {k}"));
            }
            grants.push_str(&catalog_grants(h, None));
            (note, grants, String::new(), String::new())
        } else {
            (
                String::new(),
                format!(
                    "\n          \"vega.inbox.*.*.{h}\",\n          \"vega.cmd.*.*.{h}\",\n          \"$KV.VEGA_REGISTRY.{h}.>\",\n          \"$JS.API.STREAM.INFO.KV_VEGA_REGISTRY\",\n          \"$JS.API.DIRECT.GET.KV_VEGA_REGISTRY.>\",{}",
                    catalog_grants(h, lanista_writer)
                ),
                // Serves quarantined replies to itself; answers them via
                // `allow_responses`.
                format!(", \"vega.cmd.{h}.>\", \"vega.quarantine.{h}.>\""),
                "\n        allow_responses: { max: 1, expires: \"30s\" }".to_string(),
            )
        };
        o.push_str(&format!(
            r#"    # {h}{note}
    {{
      nkey: {key}
      permissions: {{
        publish: {{ allow: [{send}
          "$JS.API.STREAM.INFO.VEGA_INBOX_{h}",
          "$JS.API.CONSUMER.*.VEGA_INBOX_{h}.>",
          "$JS.API.CONSUMER.MSG.NEXT.VEGA_INBOX_{h}.*",
          "$JS.ACK.VEGA_INBOX_{h}.>"
        ] }}
        subscribe: {{ allow: ["vega.inbox.{h}.>", "{inbox}.>"{serve}] }}{responses}
      }}
    }}
"#,
            key = e.key,
            inbox = inbox_prefix(h)
        ));
    }
    o.push_str("  ]\n}\n");
    o
}

/// The lanista catalog (catalog.rs): direct gets for every host, the put
/// subjects for the writer.
fn catalog_grants(host: &str, writer: Option<&str>) -> String {
    let mut subjects = crate::catalog::read_subjects().to_vec();
    if writer == Some(host) {
        subjects.extend(crate::catalog::write_subjects());
    }
    subjects
        .iter()
        .map(|s| format!("\n          \"{s}\","))
        .collect()
}

/// The admin may only provision: read stream info and list consumers of the
/// listed hosts, and create the streams of `create` hosts. Create is the
/// dangerous right: a not-yet-existing stream can be created as a mirror of
/// another host's stream that republishes every message to the admin's
/// reply prefix (tested). So create rights exist only while a host is being
/// added. Creating an existing name with another config fails (10058), so
/// a provisioned stream cannot be rewritten. No delete, purge, update,
/// message get, consumer create, fetch, ack or `vega.inbox` subscribe.
fn render_admin(key: &str, entries: &[Entry], create: &[String]) -> String {
    let mut allow = Vec::new();
    // The registry bucket (vegad M4): the admin creates it once, with
    // `--create registry`, like a host stream.
    if create.iter().any(|c| c == "registry") {
        allow.push("\"$JS.API.STREAM.CREATE.KV_VEGA_REGISTRY\"".to_string());
        // Bucket creation reads the account limits first (read-only).
        allow.push("\"$JS.API.INFO\"".to_string());
    }
    // The lanista catalog bucket, likewise once with `--create lanista`.
    if create.iter().any(|c| c == "lanista") {
        allow.push(format!("\"{}\"", crate::catalog::create_subject()));
        if !create.iter().any(|c| c == "registry") {
            allow.push("\"$JS.API.INFO\"".to_string());
        }
    }
    allow.push("\"$JS.API.STREAM.INFO.KV_VEGA_REGISTRY\"".to_string());
    allow.push(format!("\"{}\"", crate::catalog::info_subject()));
    for e in entries {
        let s = stream_name(&e.host);
        if create.contains(&e.host) {
            allow.push(format!("\"$JS.API.STREAM.CREATE.{s}\""));
        }
        allow.push(format!("\"$JS.API.STREAM.INFO.{s}\""));
        allow.push(format!("\"$JS.API.CONSUMER.LIST.{s}\""));
    }
    let note = if create.is_empty() {
        "    # admin: stream info and consumer lists of the hosts below; creates no\n    # streams (steady state); cannot read mail, delete, purge or change streams.\n".to_string()
    } else {
        format!(
            "    # admin: may CREATE the streams of: {} (provisioning; regenerate without\n    # --create afterwards). Cannot read mail, delete, purge or change streams.\n",
            create.join(", ")
        )
    };
    format!(
        r#"{note}    {{
      nkey: {key}
      permissions: {{
        publish: {{ allow: [
          {}
        ] }}
        subscribe: {{ allow: ["{ADMIN_INBOX}.>"] }}
      }}
    }}
"#,
        allow.join(",\n          ")
    )
}

/// Read-only viewer (NUI): list streams, read stream info and messages by
/// sequence or subject (`STREAM.MSG.GET`, `DIRECT.GET`), list consumers.
/// Deliberately no `CONSUMER.CREATE`: any consumer create also allows a
/// push consumer whose `deliver_subject` the server publishes to past
/// permissions (e.g. into `vega.inbox.*` with a forged sender). So the
/// viewer works only with clients that browse via message get (NUI after
/// 2026-07-27, not its v0.9.3 release). No publish, ack, fetch, delete,
/// purge, update or create.
fn render_viewer(key: &str, entries: &[Entry]) -> String {
    let mut allow = vec![
        "\"$JS.API.STREAM.LIST\"".to_string(),
        "\"$JS.API.STREAM.NAMES\"".to_string(),
    ];
    for e in entries {
        let s = stream_name(&e.host);
        allow.push(format!("\"$JS.API.STREAM.INFO.{s}\""));
        allow.push(format!("\"$JS.API.STREAM.MSG.GET.{s}\""));
        allow.push(format!("\"$JS.API.DIRECT.GET.{s}\""));
        allow.push(format!("\"$JS.API.DIRECT.GET.{s}.>\""));
        allow.push(format!("\"$JS.API.CONSUMER.LIST.{s}\""));
        allow.push(format!("\"$JS.API.CONSUMER.NAMES.{s}\""));
        allow.push(format!("\"$JS.API.CONSUMER.INFO.{s}.*\""));
    }
    format!(
        r#"    # viewer: read-only (stream list/info, message get, consumer list/info) for
    # a UI such as NUI; set its connection's inbox prefix to {VIEWER_INBOX}.
    # No publish, ack, fetch, consumer create, delete, purge or update.
    {{
      nkey: {key}
      permissions: {{
        publish: {{ allow: [
          {}
        ] }}
        subscribe: {{ allow: ["{VIEWER_INBOX}.>"] }}
      }}
    }}
"#,
        allow.join(",\n          ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> String {
        nkeys::KeyPair::new_user().public_key()
    }

    #[test]
    fn entries_are_validated() {
        let k = key();
        assert_eq!(
            parse_entry(&format!("proteus={k}")).unwrap(),
            Entry {
                host: "proteus".into(),
                key: k.clone()
            }
        );
        assert!(parse_entry(&format!("Proteus={k}")).is_err());
        assert!(parse_entry("proteus=Uxyz").is_err());
        let account = nkeys::KeyPair::new_account().public_key();
        assert!(parse_entry(&format!("proteus={account}")).is_err());
        assert!(parse_entry("proteus").is_err());
    }

    #[test]
    fn duplicates_are_rejected() {
        assert!(read_entries(&[format!("admin={}", key())], None).is_err());
        assert!(read_entries(&[format!("viewer={}", key())], None).is_err());
        let k = key();
        let args = vec![format!("a={k}"), format!("b={k}")];
        assert!(read_entries(&args, None).is_err());
        assert!(read_entries(&[], None).is_err());
    }

    #[test]
    fn render_scopes_each_host_to_itself() {
        let e = vec![
            Entry {
                host: "proteus".into(),
                key: key(),
            },
            Entry {
                host: "deimos".into(),
                key: key(),
            },
        ];
        let out = render(&e, Some(&key()), &[], &[], None, &[], &[], &[], None);
        assert!(out.contains("\"vega.inbox.*.*.proteus\""));
        assert!(out.contains("\"$JS.ACK.VEGA_INBOX_deimos.>\""));
        assert!(out.contains("max_file_store must be >= 2 GiB"));
        assert!(out.contains("# admin"));
        // Steady state: the admin creates nothing.
        assert!(!out.contains("STREAM.CREATE"));
        let guarded = render(&e, None, &[], &["deimos".into()], None, &[], &[], &[], None);
        assert!(guarded.contains("\"vega.inbox.*.*.proteus\""));
        assert!(!guarded.contains("vega.inbox.*.*.deimos"));
        assert!(guarded.contains("# deimos (inbox only: may not send)"));
        assert!(guarded.contains("\"$JS.ACK.VEGA_INBOX_deimos.>\""));
        assert!(guarded.contains("\"_INBOX_deimos.>\""));
        // Commands: hosts request as themselves and serve their own subject;
        // an inbox-only host gets exactly its grants.
        assert!(out.contains("\"vega.cmd.*.*.proteus\""));
        assert!(out.contains("\"vega.cmd.proteus.>\""));
        assert!(out.contains("allow_responses: { max: 1, expires: \"30s\" }"));
        assert!(!guarded.contains("vega.cmd.*.*.deimos"));
        assert!(!guarded.contains("\"vega.cmd.deimos.>\""));
        let granted = render(
            &e,
            None,
            &[],
            &["deimos".into()],
            None,
            &[("deimos".into(), "vega.cmd.proteus.open.deimos".into())],
            &[],
            &[],
            None,
        );
        assert!(granted.contains("\"vega.cmd.proteus.open.deimos\""));
        assert!(!granted.contains("vega.inbox.*.*.deimos"));
        assert!(granted.contains("granted commands only"));
        // Quarantine: the inbox-only host publishes only its own quarantine
        // subjects; the others serve theirs and never get it by default.
        assert!(!guarded.contains("\"vega.quarantine.*.*.deimos\""));
        assert!(out.contains("\"vega.quarantine.proteus.>\""));
        assert!(!guarded.contains("vega.quarantine.deimos.>"));
        let q = render(
            &e,
            None,
            &[],
            &["deimos".into()],
            None,
            &[],
            &["deimos".into()],
            &[],
            None,
        );
        assert!(q.contains("\"vega.quarantine.*.*.deimos\""));
        assert!(q.contains("replies into quarantine"));
        assert!(!q.contains("vega.inbox.*.*.deimos"));
        assert!(!q.contains("vega.quarantine.*.*.proteus"));
        assert!(!q.contains("\"vega.quarantine.>\""));
        assert!(!q.contains("\"vega.quarantine.deimos.>\""));
        let adding = render(
            &e,
            Some(&key()),
            &["deimos".into()],
            &[],
            None,
            &[],
            &[],
            &[],
            None,
        );
        assert!(adding.contains("\"$JS.API.STREAM.CREATE.VEGA_INBOX_deimos\""));
        assert!(!adding.contains("STREAM.CREATE.VEGA_INBOX_proteus"));
        assert!(out.contains("\"$JS.API.STREAM.INFO.VEGA_INBOX_proteus\""));
        assert!(out.contains("\"$JS.API.CONSUMER.LIST.VEGA_INBOX_deimos\""));
        assert!(out.contains("\"_INBOX_admin.>\""));
        for bad in [
            "VEGA_INBOX_*",
            "STREAM.DELETE",
            "STREAM.PURGE",
            "STREAM.UPDATE",
            "STREAM.MSG",
            "DIRECT.GET.VEGA_INBOX",
            "CONSUMER.CREATE",
            "CONSUMER.CREATE.KV_VEGA_REGISTRY",
            "MSG.NEXT.VEGA_INBOX_*",
            "\"vega.inbox.>\"",
            "\"$KV.VEGA_REGISTRY.>\"",
        ] {
            assert!(!out.contains(bad), "{bad}");
        }
        // Registry: write own keys, read all by direct get, no consumers.
        assert!(out.contains("\"$KV.VEGA_REGISTRY.proteus.>\""));
        assert!(out.contains("\"$JS.API.DIRECT.GET.KV_VEGA_REGISTRY.>\""));
        assert!(!guarded.contains("$KV.VEGA_REGISTRY.deimos"));
        assert!(!out.contains("STREAM.CREATE.KV_VEGA_REGISTRY"));
        assert!(
            !out.contains("{ nkey:"),
            "every user has a permissions block"
        );

        // The viewer reads; it never creates consumers, publishes, acks or fetches.
        let v = render(&e, None, &[], &[], Some(&key()), &[], &[], &[], None);
        let start = v.find("# viewer").unwrap();
        let end = start + v[start..].find("\n    }\n").unwrap();
        let viewer = &v[start..end];
        assert!(viewer.contains("\"$JS.API.STREAM.MSG.GET.VEGA_INBOX_deimos\""));
        assert!(viewer.contains("\"_INBOX_viewer.>\""));
        for bad in [
            "CONSUMER.CREATE",
            "CONSUMER.DURABLE",
            "CONSUMER.*",
            "MSG.NEXT",
            "$JS.ACK",
            "STREAM.DELETE",
            "STREAM.PURGE",
            "STREAM.UPDATE",
            "STREAM.CREATE",
            "STREAM.MSG.DELETE",
            "vega.inbox",
        ] {
            assert!(!viewer.contains(bad), "viewer must not have {bad}");
        }
        // Replies are per host: a shared `_INBOX.>` would let any host read
        // every other host's fetches.
        assert!(!out.contains("\"_INBOX.>\""));
        assert!(out.contains("\"_INBOX_proteus.>\""));
        assert!(out.contains("\"_INBOX_deimos.>\""));
        // nats.conf would read an unquoted $JS as a variable.
        assert!(!out.lines().any(|l| l.trim_start().starts_with("$JS")));
    }

    #[test]
    fn registry_read_grants_one_direct_get() {
        assert_eq!(
            parse_registry_read("deimos=proteus.theme").unwrap(),
            ("deimos".into(), "proteus.theme".into())
        );
        for bad in [
            "deimos",
            "deimos=proteus.*",
            "deimos=proteus.>",
            "deimos=proteus..theme",
            "deimos=",
            "deimos=proteus.theme ",
            "deimos=$KV.x",
            "deimos=proteus.uac.old",
            "deimos=proteus.uac.v2.zz",
            "deimos=proteus.lanista.6162.61.62.63",
            "deimos=proteus.lanista.v2.61.62.63",
            "=proteus.theme",
        ] {
            assert!(parse_registry_read(bad).is_err(), "{bad}");
        }
        let e = vec![
            Entry {
                host: "proteus".into(),
                key: key(),
            },
            Entry {
                host: "deimos".into(),
                key: key(),
            },
        ];
        let out = render(
            &e,
            None,
            &[],
            &["deimos".into()],
            None,
            &[],
            &[],
            &[("deimos".into(), "proteus.theme".into())],
            None,
        );
        let start = out.find("# deimos").unwrap();
        let deimos = &out[start..];
        assert!(deimos.starts_with(
            "# deimos (inbox only: may not send; reads registry key proteus.theme)\n"
        ));
        // Exactly one registry subject: the direct get of that key.
        let registry: Vec<&str> = deimos
            .lines()
            .map(str::trim)
            .filter(|l| l.contains("REGISTRY"))
            .collect();
        assert_eq!(
            registry,
            ["\"$JS.API.DIRECT.GET.KV_VEGA_REGISTRY.$KV.VEGA_REGISTRY.proteus.theme\","]
        );
        // The other hosts keep theirs; no grant leaks to a non-inbox-only host.
        let proteus = &out[out.find("# proteus").unwrap()..start];
        assert!(proteus.contains("\"$KV.VEGA_REGISTRY.proteus.>\""));
        assert!(!proteus.contains("proteus.theme"));
        // A grant for a host that is not inbox-only renders nothing.
        let full = render(
            &e,
            None,
            &[],
            &[],
            None,
            &[],
            &[],
            &[("deimos".into(), "proteus.theme".into())],
            None,
        );
        assert!(!full.contains("proteus.theme"));
    }
}
