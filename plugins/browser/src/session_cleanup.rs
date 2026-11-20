//! Legacy session inventory and guarded cleanup.
use vega_browser::{
    dev,
    legacy::{self, Daemon},
    owner, pane,
};
use vega_herdr::{cell, Failure};

use crate::valid_session;

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(
        "session_cleanup_failed",
        error.to_string(),
        "vega browser gc --dry-run",
    )
}

#[derive(PartialEq)]
enum Owner {
    Ownerless,
    Remote,
    Live,
    Dead,
    Unknown,
}

fn shared_names() -> Result<Vec<String>, Failure> {
    let mut names = Vec::new();
    for name in owner::load().into_keys() {
        if dev::shared::bound(&name)?.is_some() {
            names.push(name);
        }
    }
    Ok(names)
}

fn classify(record: Option<&owner::Record>, local: &str) -> Owner {
    let Some(record) = record else {
        return Owner::Ownerless;
    };
    if record.host.is_empty() || local.is_empty() || local == "unknown" {
        return Owner::Unknown;
    }
    if record.host != local {
        return Owner::Remote;
    }
    match record.pane.as_deref() {
        None => Owner::Ownerless,
        Some(pane) => match pane::alive(pane) {
            Some(true) => Owner::Live,
            Some(false) => Owner::Dead,
            None => Owner::Unknown,
        },
    }
}

impl Owner {
    fn label(&self) -> &'static str {
        match self {
            Self::Ownerless => "ownerless",
            Self::Remote => "remote",
            Self::Live => "live",
            Self::Dead => "dead",
            Self::Unknown => "unknown",
        }
    }
}

pub fn inventory() -> Result<(), Failure> {
    let names = legacy::socket_names().map_err(failure)?;
    let dev = dev::session_names()?;
    let shared = shared_names()?;
    let local = vega_herdr::local_host();
    let rows = names
        .iter()
        .filter(|session| !dev.contains(session) && !shared.contains(session))
        .map(|name| {
            let ownership =
                owner::get(name).map_or(Owner::Unknown, |rec| classify(rec.as_ref(), &local));
            let identity = if ownership == Owner::Ownerless {
                Daemon::find(name).ok().map(|d| d.identity().to_owned())
            } else {
                None
            };
            (name, ownership, identity)
        })
        .collect::<Vec<_>>();
    println!("legacy[{}]{{session,owner,identity}}:", rows.len());
    for (name, ownership, identity) in rows {
        println!(
            "  {},{},{}",
            cell(name),
            ownership.label(),
            identity.unwrap_or_default()
        );
    }
    Ok(())
}

pub fn disconnect(session: &str, identity: &str) -> Result<(), Failure> {
    valid_session(session)?;
    let lock = owner::Locked::acquire().map_err(failure)?;
    if dev::session_names()?.iter().any(|name| name == session)
        || dev::shared::bound(session)?.is_some()
        || classify(
            lock.get(session).map_err(failure)?.as_ref(),
            &vega_herdr::local_host(),
        ) != Owner::Ownerless
    {
        return Err(failure("session is no longer a legacy ownerless entry"));
    }
    let daemon = Daemon::find(session).map_err(failure)?;
    if daemon.identity() != identity {
        return Err(failure(
            "reviewed daemon is no longer active with that identity",
        ));
    }
    daemon.close(session).map_err(failure)?;
    lock.remove(session).map_err(failure)?;
    println!("disconnected: {session}");
    Ok(())
}

pub fn gc() -> Result<(), Failure> {
    let names = legacy::socket_names().map_err(failure)?;
    let dev = dev::session_names()?;
    let shared = shared_names()?;
    let local = vega_herdr::local_host();
    let mut closed = 0;
    let mut kept = 0;
    for name in names
        .iter()
        .filter(|name| !dev.contains(name) && !shared.contains(name))
    {
        let original = Daemon::find(name);
        let lock = owner::Locked::acquire().map_err(failure)?;
        let record = lock.get(name).map_err(failure)?;
        let ownership = classify(record.as_ref(), &local);
        if ownership != Owner::Dead {
            println!("kept {name}: {}", ownership.label());
            kept += 1;
            continue;
        }
        // Acquire the socket identity before probing liveness, then verify
        // it again on the connected socket before sending any command.
        if dev::session_names()?.contains(name) || dev::shared::bound(name)?.is_some() {
            kept += 1;
            continue;
        }
        match original.and_then(|daemon| daemon.close(name)) {
            Ok(()) => {
                lock.remove(name).map_err(failure)?;
                println!(
                    "closed {name} (owner pane {} is gone)",
                    record.and_then(|r| r.pane).unwrap_or_default()
                );
                closed += 1;
            }
            Err(e) => {
                println!("kept {name}: could not verify or close daemon ({e})");
                kept += 1;
            }
        }
    }
    for name in shared {
        if dev::shared::gc_close(&name)? {
            println!("closed {name} (shared Chromium owner pane is gone)");
            closed += 1;
        } else {
            kept += 1;
        }
    }
    for name in dev {
        if dev::gc_close(&name)? {
            println!("closed {name} (dev owner pane is gone)");
            closed += 1;
        } else {
            kept += 1;
        }
    }
    println!("gc: {closed} closed, {kept} kept");
    Ok(())
}
