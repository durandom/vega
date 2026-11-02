use std::process::ExitCode;
use vega_herdr::{cell, mail_grants as store, print_failure, state, Failure};

const HELP: &str = include_str!("../help/grant.md");

pub fn main(args: &[String]) -> ExitCode {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{HELP}");
        return ExitCode::SUCCESS;
    }
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(f) => {
            let code = if f.code == "usage" { 2 } else { 1 };
            print_failure(&f);
            ExitCode::from(code)
        }
    }
}

fn run(args: &[String]) -> Result<(), Failure> {
    let usage = |msg: String| Failure::new("usage", msg, HELP);
    let io = |msg: String| {
        Failure::new(
            "grant_store",
            msg,
            "run on the receiving vegad host with its HOME and XDG_STATE_HOME",
        )
    };
    let (revoke, sender, target, duration) = match args {
        [cmd] if cmd == "grants" => return listing(),
        [cmd, sender, target, flag, duration] if cmd == "grant" && flag == "--expires" => (false, sender, target, Some(duration)),
        [cmd, sender, target] if cmd == "revoke" => (true, sender, target, None),
        _ => return Err(usage("expected grant <sender> <target> --expires <duration>, revoke <sender> <target>, or grants; valid flag: --expires (--help always allowed)".into())),
    };
    let sender = store::Peer::parse(sender).map_err(&usage)?;
    let target = store::Peer::parse(target).map_err(&usage)?;
    let host = state::post_host();
    if target.host != host
        || (!revoke && !state::is_inbox_only(&sender.host))
        || sender.host == target.host
    {
        return Err(usage(format!(
            "sender must be an inbox-only host; target must be this receiving host ({host})"
        )));
    }
    let now = store::now();
    let g = if revoke {
        let Some(mut g) = store::list()
            .map_err(&io)?
            .into_iter()
            .find(|g| g.sender == sender && g.target == target)
        else {
            println!("grant: absent (already revoked)");
            return Ok(());
        };
        g.revoked.get_or_insert(now);
        g
    } else {
        let seconds = store::duration(duration.unwrap()).ok_or_else(|| {
            usage("--expires requires a positive duration: 30m, 2h, 1d (s/m/h/d)".into())
        })?;
        let expires = now
            .checked_add(seconds)
            .ok_or_else(|| usage("expiry is too large".into()))?;
        store::Grant {
            sender,
            target,
            created: now,
            expires,
            revoked: None,
        }
    };
    store::save(&g).map_err(io)?;
    println!(
        "grant: {} -> {}, {}, expires {} (Unix seconds)",
        g.sender.label(),
        g.target.label(),
        if revoke { "revoked" } else { "live" },
        g.expires
    );
    println!("help: vega uac grants");
    Ok(())
}

pub fn listing() -> Result<(), Failure> {
    let grants = store::list().map_err(|e| {
        Failure::new(
            "grant_store",
            e,
            "inspect the receiving host's vega/mail-grants state files",
        )
    })?;
    if grants.is_empty() {
        println!("grants: 0 inbox-only mail pair grants on this host");
    } else {
        println!(
            "grants[{}]{{sender,target,status,created,expires,revoked}}:",
            grants.len()
        );
        for g in grants {
            let status = if g.revoked.is_some() {
                "revoked"
            } else if g.live(store::now()) {
                "live"
            } else {
                "expired"
            };
            println!(
                "  {},{},{status},{},{},{}",
                cell(&g.sender.label()),
                cell(&g.target.label()),
                g.created,
                g.expires,
                g.revoked
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "null".into())
            );
        }
    }
    Ok(())
}
