// Shared client protocol. Included at the crate root by both entry points.

/// Header names. `Nats-Msg-Id` makes a retried publish idempotent.
mod h {
    pub const FROM_PANE: &str = "Vega-From-Pane";
    pub const FROM_AGENT: &str = "Vega-From-Agent";
    pub const FROM_CWD: &str = "Vega-From-Cwd";
    pub const SENT_MS: &str = "Vega-Sent-Ms";
    pub const THREAD: &str = "Vega-Thread";
    pub const HOP: &str = "Vega-Hop";
    /// Set by `send --now`: the relay types the pointer at once.
    pub const NOW: &str = "Vega-Now";
    /// Set by `done`: the outcome; the parent's relay records it.
    pub const DONE: &str = "Vega-Done";
    /// Set by `send --kind`: a typed JSON body (vega_uber_msg).
    pub const KIND: &str = vega_uber_msg::HEADER;
}

/// How agents call this binary in pointers and reply lines. `VEGA_POST_CMD`
/// overrides it, e.g. to test a dev build that is not on PATH.
fn cmd() -> String {
    env::var("VEGA_POST_CMD").unwrap_or_else(|_| "vega post".into())
}

/// `VEGA_NATS_URL`, else the first line of `$XDG_CONFIG_HOME/vega/nats.url`
/// (for machines without Tailscale DNS, where only the IP works), else unconfigured.
fn url() -> String {
    vega_herdr::service_config::get("VEGA_NATS_URL", "nats.url").unwrap_or_default()
}

/// True when a broker is set (`VEGA_NATS_URL` or `nats.url`), with no implicit estate default.
fn broker_configured() -> bool {
    !url().is_empty()
}

/// `$XDG_CONFIG_HOME`, else `~/.config`.
fn config_dir() -> Option<String> {
    vega_herdr::platform::config_home().map(|p| p.to_string_lossy().into_owned())
}

/// `VEGA_NATS_NKEY`, ignoring an empty value.
fn nkey_env() -> Option<String> {
    env::var("VEGA_NATS_NKEY").ok().filter(|p| !p.is_empty())
}

/// Where this machine's nkey seed lives: `VEGA_NATS_NKEY`, else
/// `$XDG_CONFIG_HOME/vega/nats.nk` (default `~/.config/vega/nats.nk`).
fn seed_path(kind: Seed) -> Result<String, Failure> {
    if let Some(p) = nkey_env() {
        return Ok(p);
    }
    let config = config_dir().ok_or_else(|| {
        Failure::new(
            "no_home",
            "neither XDG_CONFIG_HOME nor HOME is set",
            "set VEGA_NATS_NKEY to the seed file",
        )
    })?;
    Ok(std::path::Path::new(&config)
        .join("vega")
        .join(kind.file())
        .to_string_lossy()
        .into_owned())
}

/// The seed to connect with: `VEGA_NATS_NKEY` (must exist), else the
/// default seed if it exists, else none: no auth (local dev broker).
fn nkey_path(kind: Seed) -> Option<String> {
    if let Some(p) = nkey_env() {
        return Some(p);
    }
    seed_path(kind)
        .ok()
        .filter(|p| std::path::Path::new(p).exists())
}

/// Live view: broker, my pane, unread count.
fn home() -> Result<(), Failure> {
    let bin = env::current_exe()
        .map(|p| collapse_home(&p.display().to_string()))
        .unwrap_or_else(|_| "unknown".into());
    println!("bin: {bin}");
    println!("description: Agent mail over a NATS JetStream broker (spike)");
    let broker = url();
    println!(
        "broker: {}",
        if broker.is_empty() {
            "unconfigured"
        } else {
            &broker
        }
    );
    println!(
        "key: {}",
        nkey_path(Seed::Host)
            .map(|p| collapse_home(&p))
            .unwrap_or_else(|| "none (no auth)".into())
    );
    let host = host();
    println!("host: {host}");
    let me = me()?;
    println!("me: {} ({}, {})", me.pane, me.agent, cell(&me.cwd));
    let unread = runtime()?.block_on(async {
        let client = connect("home", None).await?;
        let stream = inbox_stream(&vega_registry::broker_context(client), &host).await?;
        unread(&stream, &host, &me.pane).await
    })?;
    println!("unread: {unread}");
    println!("help[2]:");
    println!("  vega post send <pane> \"<text>\"   # the reply arrives as a pointer");
    if unread > 0 {
        println!("  vega post inbox                  # read and ack your mail");
    } else {
        println!("  vega post log                    # archive, newest first");
    }
    Ok(())
}

fn runtime() -> Result<tokio::runtime::Runtime, Failure> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| Failure::new("runtime", e.to_string(), "report this as a vega bug"))
}

fn broker_err(e: impl std::fmt::Display) -> Failure {
    Failure::new(
        "broker_error",
        e.to_string(),
        "check the broker: `nats --server $VEGA_NATS_URL server check jetstream`",
    )
}

/// The shared mail identity (process env, vegad.env, then hostname).
fn host() -> String {
    vega_herdr::state::post_host()
}

/// NATS subject tokens must not contain `.`, `*`, `>` or whitespace.
fn subject_token(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c == '.' || c == '*' || c == '>' || c.is_whitespace() {
                '_'
            } else {
                c
            }
        })
        .collect()
}

/// `vega.inbox.<to-host>.<to-pane>.<from-host>`.
fn inbox_subject(to_host: &str, pane: &str, from_host: &str) -> String {
    format!(
        "vega.inbox.{}.{}.{}",
        subject_token(to_host),
        subject_token(terminals::bare(pane)),
        subject_token(from_host)
    )
}

/// All mail for one pane, from any host.
fn pane_filter(host: &str, pane: &str) -> String {
    format!(
        "vega.inbox.{}.{}.*",
        subject_token(host),
        subject_token(terminals::bare(pane))
    )
}

/// `(to-pane, from-host)` from an inbox subject. The from-host is enforced
/// by the sender's publish permission, unlike the `Vega-From-*` headers.
fn parse_subject(subject: &str) -> Option<(&str, &str)> {
    let mut t = subject.strip_prefix("vega.inbox.")?.split('.');
    let (_to_host, pane, from) = (t.next()?, t.next()?, t.next()?);
    t.next().is_none().then_some((terminals::bare(pane), from))
}

/// Reply-subject prefix of this host's connections.
fn inbox_prefix(host: &str) -> String {
    format!("_INBOX_{}", subject_token(host))
}

/// Per-host stream: stream names must not contain `.`, `*`, `>`.
fn stream_name(host: &str) -> String {
    format!("VEGA_INBOX_{}", subject_token(host))
}

/// JetStream consumer names must not contain `.`, `*`, `>`, `/`, `\`, `:` is
/// avoided too for portability.
fn consumer_name(host: &str, pane: &str) -> String {
    let safe = |s: &str| -> String {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    };
    format!("inbox-{}-{}", safe(host), safe(terminals::bare(pane)))
}

/// Parses `--re <thread>/<hop>[@<published-ms>]` at send time. Legacy tokens
/// lack timing information and keep counting conservatively.
fn next_hop(re: Option<&str>, now: u128) -> Result<(String, u32), Failure> {
    let Some(re) = re else {
        return Ok((nuid::next().to_string(), 1));
    };
    let bad = || {
        Failure::new(
            "usage",
            format!("--re {re} is not <thread>/<hop>[@<published-ms>]"),
            "copy the reply command from `vega post inbox` verbatim",
        )
    };
    let (thread, hop) = re.rsplit_once('/').ok_or_else(bad)?;
    let (hop, published) = match hop.split_once('@') {
        Some((hop, ms)) => (hop, Some(ms.parse::<u128>().map_err(|_| bad())?)),
        None => (hop, None),
    };
    let hop: u32 = hop.parse().map_err(|_| bad())?;
    if thread.is_empty() {
        return Err(bad());
    }
    // A future timestamp (clock skew) cannot reset a fast chain.
    if published.is_some_and(|ms| now.saturating_sub(ms) >= FAST_REPLY_WINDOW.as_millis()) {
        return Ok((thread.to_string(), 1));
    }
    if hop >= MAX_HOPS {
        let hint = if published.is_some() {
            "stop automatic replies and tell the user; a substantive reply can resume with the same timestamped --re token after 120 s from the answered message"
        } else {
            "stop automatic replies and tell the user; this legacy token has no timestamp and stays capped; re-read `vega post inbox --show <id>` on an upgraded host for a timestamped reply command"
        };
        return Err(Failure::new(
            "hop_limit",
            format!("thread {thread} reached {hop} fast hops (max {MAX_HOPS})"),
            hint,
        ));
    }
    Ok((thread.to_string(), hop + 1))
}

fn stream_config(host: &str) -> stream::Config {
    stream::Config {
        name: stream_name(host),
        subjects: vec![format!("vega.inbox.{}.>", subject_token(host))],
        // An archive: acks mark mail read, they do not delete it.
        retention: stream::RetentionPolicy::Limits,
        max_age: ARCHIVE_MAX_AGE,
        max_bytes: ARCHIVE_MAX_BYTES,
        discard: stream::DiscardPolicy::Old,
        storage: stream::StorageType::File,
        ..Default::default()
    }
}

/// This host's inbox stream. With a key (production) only the admin creates
/// streams (`vega post provision`): a machine allowed to create its own
/// stream could also give it the subjects of every other host. Without a key
/// (local dev broker) it is created on first use.
async fn inbox_stream(js: &jetstream::Context, host: &str) -> Result<stream::Stream, Failure> {
    let stream = match js.get_stream(stream_name(host)).await {
        Ok(s) => s,
        Err(e) if stream_missing(&e) => {
            if nkey_path(Seed::Host).is_some() {
                return Err(Failure::new(
                    "no_inbox_stream",
                    format!("{} does not exist on the broker", stream_name(host)),
                    &format!("machine keys cannot create streams; the admin must run `vega post provision {host}`"),
                ));
            }
            js.get_or_create_stream(stream_config(host))
                .await
                .map_err(broker_err)?
        }
        Err(e) => return Err(broker_err(e)),
    };
    check_archive(&stream, host)?;
    Ok(stream)
}

fn stream_missing(e: &jetstream::context::GetStreamError) -> bool {
    matches!(
        e.kind(),
        jetstream::context::GetStreamErrorKind::JetStream(err)
            if err.error_code() == jetstream::ErrorCode::STREAM_NOT_FOUND
    )
}

fn check_archive(stream: &stream::Stream, host: &str) -> Result<(), Failure> {
    // JetStream cannot change retention in place.
    if stream.cached_info().config.retention != stream::RetentionPolicy::Limits {
        return Err(Failure::new(
            "stream_outdated",
            format!("{} is not an archive (retention is not limits)", stream_name(host)),
            "an admin must delete the stream (`nats stream rm <name>`) and run `vega post provision <host>`",
        ));
    }
    Ok(())
}

/// The durable consumer that tracks what one pane has read. It never
/// expires: a recreated consumer would deliver the whole archive as unread.
fn inbox_consumer(host: &str, pane: &str) -> pull::Config {
    pull::Config {
        durable_name: Some(consumer_name(host, pane)),
        filter_subject: pane_filter(host, pane),
        ack_policy: jetstream::consumer::AckPolicy::Explicit,
        ack_wait: ACK_WAIT,
        ..Default::default()
    }
}

/// Unread mail of one pane: not yet acked by its durable consumer (pending
/// plus delivered-but-unacked). Without a consumer the pane never read
/// anything, so all its mail counts.
async fn unread(stream: &stream::Stream, host: &str, pane: &str) -> Result<u64, Failure> {
    Ok(unread_and_floor(stream, host, pane).await?.0)
}

/// Unread count and the consumer's ack floor (stream sequence; 0 if the
/// pane never read). A higher floor than before means the agent read mail.
async fn unread_and_floor(
    stream: &stream::Stream,
    host: &str,
    pane: &str,
) -> Result<(u64, u64), Failure> {
    match stream.consumer_info(consumer_name(host, pane)).await {
        Ok(info) => Ok((
            info.num_pending + info.num_ack_pending as u64,
            info.ack_floor.stream_sequence,
        )),
        Err(e) if e.kind() == jetstream::context::ConsumerInfoErrorKind::NotFound => Ok((
            subject_counts(stream, &pane_filter(host, pane))
                .await?
                .iter()
                .map(|(_, n)| *n as u64)
                .sum(),
            0,
        )),
        Err(e) => Err(broker_err(e)),
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// `send`'s positionals: target and text, or with `--uac` only the text.
fn send_args<'a>(
    args: &'a [String],
    host: Option<&'a str>,
    uac: Option<&'a str>,
    uber: bool,
) -> Result<(To<'a>, &'a str), Failure> {
    match (uac, args) {
        (None, [text]) if uber => Ok((To::Uber, text)),
        (None, _) if uber => Err(Failure::new(
            "usage",
            "with --uber give only the text",
            "vega post send --uber \"<text>\"",
        )),
        (Some(p), [text]) => Ok((To::Uac(p), text)),
        (None, [target, text]) => Ok((To::Pane(target, host), text)),
        (Some(_), _) => Err(Failure::new(
            "usage",
            "with --uac give only the text",
            "vega post send --uac <project> \"<text>\"",
        )),
        (None, _) => Err(Failure::new(
            "usage",
            "give the target pane and the text",
            "vega post send <pane> \"<text>\", or vega post send --uac <project> \"<text>\"",
        )),
    }
}

/// `send --kind`: the body must pass its kind's schema and cap, else
/// nothing is sent (docs/design/uber-uac.md §3).
fn mail_kind(kind: Option<&str>, text: &str) -> Result<Option<vega_uber_msg::Kind>, Failure> {
    let Some(k) = kind else { return Ok(None) };
    vega_uber_msg::validate_kind(k, text.as_bytes())
        .map(|_| vega_uber_msg::Kind::parse(k))
        .map_err(|r| {
            Failure::new(
                r.code,
                format!("{}; nothing was sent", r.message),
                "fix the JSON body; schemas and caps: docs/design/uber-uac.md §3",
            )
        })
}

/// The mail headers `send` sets.
fn send_headers(
    me: &vega_herdr::Me,
    thread: &str,
    hop: u32,
    now: bool,
    done: Option<Outcome>,
    kind: Option<vega_uber_msg::Kind>,
) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("Nats-Msg-Id", nuid::next().as_str());
    if !me.pane.is_empty() {
        headers.insert(h::FROM_PANE, me.pane.as_str());
    }
    headers.insert(h::FROM_AGENT, me.agent.as_str());
    headers.insert(h::FROM_CWD, me.cwd.as_str());
    headers.insert(h::SENT_MS, now_ms().to_string().as_str());
    headers.insert(h::THREAD, thread);
    headers.insert(h::HOP, hop.to_string().as_str());
    if now {
        headers.insert(h::NOW, "1");
    }
    if let Some(o) = done {
        headers.insert(h::DONE, o.as_str());
    }
    if let Some(k) = kind {
        headers.insert(h::KIND, k.as_str());
    }
    headers
}

/// The project of the directory `send` runs in (its UAC key).
fn my_project() -> Result<Option<String>, Failure> {
    let cwd =
        env::current_dir().map_err(|e| Failure::new("project_io", e.to_string(), "check cwd"))?;
    vega_herdr::project::resolve_optional(&cwd, &st::post_host())
        .map(|p| p.map(|p| p.id.to_string()))
}

/// Whether this host's herdr lists the pane; `None` when herdr is away.
fn local_pane_open(pane: &str) -> Option<bool> {
    let pane = terminals::bare(pane);
    if terminals::is_hc(pane) {
        return match vega_herdr::hc::hc_local(&["get", pane]) {
            Ok(t) => Some(t["state"] == "live"),
            Err(e) if e.code == "pane_not_found" => Some(false),
            Err(_) => None,
        };
    }
    let v = herdr(&["agent", "list"]).ok()?;
    let agents = v["result"]["agents"].as_array()?;
    Some(agents.iter().any(|a| a["pane_id"] == pane))
}

/// The pane and host to mail, checked in the registry (reach.rs). `--uac`
/// resolves the project's UAC from one registry read. A pane its fresh host
/// no longer lists fails with `pane_gone`, unless the verified machine's hc
/// still reports the qualified terminal live. When the registry cannot tell
/// (stale host, no answer), the warning comes back and the mail goes.
async fn resolve_target(
    client: &async_nats::Client,
    to: To<'_>,
    check: bool,
) -> Result<(String, String, Option<String>), Failure> {
    let my_host = host();
    let machine = match to {
        To::Pane(pane, _) => terminals::machine_host(pane),
        _ => None,
    };
    let (target, to_host) = match to {
        To::Uac(query) => {
            let view = tokio::time::timeout(reach::WAIT, reach::read(client))
                .await
                .unwrap_or_else(|_| Err("no answer within 2 s".into()))
                .map_err(|e| {
                    Failure::new(
                        "registry_unavailable",
                        format!("cannot resolve --uac {query}: registry: {e}"),
                        "send to the pane itself: vega post send [--host <host>] <pane> \"<text>\"",
                    )
                })?;
            let here = if query == "." { my_project()? } else { None };
            let (h, p, _) = reach::resolve(&view, query, here.as_deref())?;
            return Ok((terminals::bare(&p).to_string(), subject_token(&h), None));
        }
        To::Uber => {
            let view = tokio::time::timeout(reach::WAIT, reach::read(client))
                .await
                .unwrap_or_else(|_| Err("no answer within 2 s".into()))
                .map_err(|e| {
                    Failure::new(
                        "registry_unavailable",
                        format!("cannot resolve --uber: registry: {e}"),
                        "without the registry there is no Uber-UAC; use the bead and the inbox",
                    )
                })?;
            let c = view.live_uber().ok_or_else(|| {
                Failure::new(
                    "uber_absent",
                    "no live Uber-UAC claim; nothing was sent",
                    "the Slayer's session runs `vega uber claim`; until then use the bead and the inbox",
                )
            })?;
            return Ok((
                terminals::bare(&c.pane).to_string(),
                subject_token(&c.host),
                None,
            ));
        }
        To::Pane(target, host) => terminals::mail_target(target, host, &my_host)?,
    };
    if !check && machine.is_none() {
        return Ok((target, to_host, None));
    }
    let fetched =
        match tokio::time::timeout(reach::WAIT, reach::fetch(client, &to_host, &target)).await {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(format!("registry: {e}")),
            Err(_) => Err("the registry did not answer within 2 s".to_string()),
        };
    if let Some(machine) = machine.as_deref() {
        let view = fetched.as_ref().map_err(|why| {
            Failure::new(
                "registry_unavailable",
                format!("cannot verify hc machine {machine}: {why}"),
                "restore registry access and retry; no mail was enqueued",
            )
        })?;
        view.check_hc_machine(machine)?;
    }
    if !check {
        return Ok((target, to_host, None));
    }
    let reached = match &fetched {
        Ok(v) => reach::reach(v, &to_host, &target),
        Err(e) => reach::Reach::Unknown(e.clone()),
    };
    if reached == reach::Reach::Open {
        return Ok((target, to_host, None));
    }
    // A remote core knows about a newly spawned terminal before vegad's next
    // registry reconcile. Only use hc after the qualified machine/host check
    // above, and only positive liveness evidence can override the registry.
    if reached == reach::Reach::Gone {
        if let Some(machine) = machine.as_deref() {
            let address = format!("{machine}/{target}");
            let terminal = vega_herdr::hc::hc_on(None, &["get", &address], Some(reach::WAIT));
            if terminal.is_ok_and(|t| t["state"] == "live") {
                return Ok((target, to_host, None));
            }
        }
    }
    let unsure = |why: &str| {
        Some(format!(
            "{why}; cannot tell whether {target} is open, mailed anyway"
        ))
    };
    if to_host == my_host {
        // This host's herdr is the truth for its panes: the registry trails
        // a new pane by up to one reconcile, and may be off (local mode).
        match (local_pane_open(&target), &reached) {
            (Some(true), _) => return Ok((target, to_host, None)),
            (Some(false), _) => {}
            (None, reach::Reach::Unknown(why)) => {
                let w = unsure(&format!("{why}, and herdr does not answer"));
                return Ok((target, to_host, w));
            }
            (None, _) => {
                let w = unsure("the registry does not list it and herdr does not answer");
                return Ok((target, to_host, w));
            }
        }
    } else if let reach::Reach::Unknown(why) = &reached {
        let w = unsure(why);
        return Ok((target, to_host, w));
    } else if let Some(m) = st::children()
        .into_iter()
        .filter_map(|c| {
            c.machine
                .filter(|m| subject_token(&m.to_lowercase()) == to_host && c.pane == target)
        })
        .next()
    {
        // A remote child newer than its host's last reconcile.
        if !remote_gone_proven(fetched.as_ref().ok(), &m, &target) {
            let w = Some(format!(
                "{target} is your recent child and {to_host}'s registry entry predates it; mailed anyway"
            ));
            return Ok((target, to_host, w));
        }
    }
    let full = tokio::time::timeout(reach::WAIT, reach::read(client))
        .await
        .ok()
        .and_then(Result::ok);
    let project = full
        .as_ref()
        .and_then(|v| reach::project_of(v, &to_host, &target))
        .or_else(|| my_project().ok().flatten());
    Err(Failure::new(
        "pane_gone",
        format!("{target} on {to_host} is closed (its host no longer lists it); nothing was sent"),
        &reach::gone_hint(full.as_ref(), project.as_deref(), (&to_host, &target)),
    ))
}

fn send(
    to: To,
    text: &str,
    re: Option<&str>,
    now: bool,
    done: Option<Outcome>,
    kind: Option<vega_uber_msg::Kind>,
    from_tool: Option<&str>,
) -> Result<(), Failure> {
    let (thread, hop) = next_hop(re, now_ms())?;
    let cwd = || {
        env::current_dir()
            .map(|p| collapse_home(&p.display().to_string()))
            .unwrap_or_default()
    };
    // Outside any pane (a user shell): the user, without a return
    // path; the message carries no sender pane, so nobody can reply by mail.
    // A tool (`--from-tool`) likewise has no pane, so it may mail its own.
    let me = match from_tool {
        Some(tool) => Ok(vega_herdr::Me {
            pane: String::new(),
            workspace: String::new(),
            agent: tool_sender(tool)?,
            program: String::new(),
            cwd: cwd(),
        }),
        None => me(),
    };
    let me = match me {
        Ok(m) => m,
        Err(f) if (f.code == "not_in_herdr" || f.code == "not_in_terminal") && done.is_none() => {
            vega_herdr::Me {
                pane: String::new(),
                workspace: String::new(),
                agent: "the user".into(),
                program: String::new(),
                cwd: cwd(),
            }
        }
        Err(f) => return Err(f),
    };
    let my_host = self::host();
    let self_send = |to_host: &str, target: &str| {
        if to_host == my_host && target == me.pane {
            return Err(Failure::new(
                "self_send",
                format!("{target} is your own pane"),
                "run `vega mux` to list other agents",
            ));
        }
        Ok(())
    };
    if let To::Pane(target, target_host) = to {
        let to_host = target_host
            .map(subject_token)
            .unwrap_or_else(|| my_host.clone());
        self_send(&to_host, target)?;
    }
    #[cfg(all(unix, not(feature = "client-only")))]
    if st::is_inbox_only(&my_host) {
        let (target, to_host) = local_send::resolve(to, &my_host)?;
        self_send(&to_host, &target)?;
        let headers = send_headers(&me, &thread, hop, now, done, kind);
        if to_host == my_host {
            return local_send::deliver(&target, &me, &headers, text, done);
        }
        return send_quarantine(&to_host, &target, headers, text, done);
    }

    #[cfg(any(windows, feature = "client-only"))]
    if st::is_inbox_only(&my_host) {
        return Err(Failure::new(
            "unsupported_role",
            "Windows client requires a normal host role",
            "ask the operator to provision normal host permissions",
        ));
    }
    let started = Instant::now();
    let rt = runtime()?;
    let client = rt.block_on(connect_patiently())?;
    // A tool mails the pane that started it, which is open.
    let (target, to_host, warning) =
        rt.block_on(resolve_target(&client, to, from_tool.is_none()))?;
    let target = target.as_str();
    self_send(&to_host, target)?;
    let subject = inbox_subject(&to_host, target, &my_host);
    let headers = send_headers(&me, &thread, hop, now, done, kind);
    let ack = rt.block_on(async {
        let js = vega_registry::broker_context(client);
        // No stream lookup: the target host's stream captures the subject. If
        // that host never ran `relay` or `inbox`, nothing does.
        mail_publish::mail_retry(&js, subject.clone(), headers, text.to_string())
            .await
            .map_err(|e| {
                if e.kind() == jetstream::context::PublishErrorKind::StreamNotFound {
                    Failure::new(
                        "no_inbox",
                        format!("no inbox stream for host {to_host}"),
                        "check --host; that machine must run `vega post relay` once",
                    )
                } else {
                    broker_err(e)
                }
            })
    })?;
    if done.is_none() {
        if let Some(c) = child_of(&st::children(), target, &me.pane, &to_host) {
            st::save_brief(c.machine.as_deref(), target);
        }
    }
    let warn = || {
        if let Some(w) = &warning {
            println!("warning: {w}");
        }
    };
    if let Some(o) = done {
        println!("done: {} to {target} on {to_host}", o.as_str());
        warn();
        println!("id: {}", ack.sequence);
        println!("help: your parent decides what comes next; end your turn now");
        return Ok(());
    }
    println!("sent: {target} on {to_host}");
    warn();
    println!("id: {}", ack.sequence);
    println!("thread: {thread}/{hop}");
    println!("ms: {}", started.elapsed().as_millis());
    println!(
        "pointer: {}",
        if now {
            "at once (--now)"
        } else {
            "also while the target works, once its input box is clear"
        }
    );
    println!("help: the reply arrives as your own pointer; do not poll");
    Ok(())
}

/// The `Vega-From-Agent` of `--from-tool` mail: `tool vega-<name>`. Only vega
/// tool names, and always the `tool` prefix, so no caller can pose as a person
/// or an agent ("the user", "claude").
fn tool_sender(label: &str) -> Result<String, Failure> {
    let name = label.strip_prefix("vega-").unwrap_or("");
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_lowercase()) {
        return Err(Failure::new(
            "usage",
            format!("--from-tool {label}: not a vega tool name"),
            "use vega-<name>, lowercase letters only, e.g. --from-tool vega-review",
        ));
    }
    Ok(format!("tool {label}"))
}

/// `vega post done`: a typed mail to the parent. Its relay sees the header,
/// records the outcome and wakes the parent (supervise.rs).
fn done(outcome: &str, summary: &str, to: Option<&str>, host: Option<&str>) -> Result<(), Failure> {
    let outcome = Outcome::parse(outcome)
        .ok_or_else(|| Failure::new("usage", "bad --outcome", "succeeded or failed"))?;
    if summary.trim().is_empty() {
        return Err(Failure::new(
            "usage",
            "the summary is empty",
            "vega post done --outcome succeeded \"<what changed, where, gates>\"",
        ));
    }
    let (to, host) = match to {
        Some(t) => (t.to_string(), host.map(String::from)),
        None => {
            let me = me()?;
            let parent = recorded_parent(&me.pane).ok_or_else(|| {
                Failure::new(
                    "no_parent",
                    format!("no parent recorded for {} on this machine", me.pane),
                    "pass the pane from your task footer: vega post done --to <pane> [--host <machine>] ...",
                )
            })?;
            let host =
                (subject_token(&parent.host.to_lowercase()) != self::host()).then_some(parent.host);
            (parent.pane, host)
        }
    };
    let text = format!("done ({}): {summary}", outcome.as_str());
    send(
        To::Pane(&to, host.as_deref()),
        &text,
        None,
        false,
        Some(outcome),
        None,
        None,
    )
}

/// The parent `vega mux spawn` recorded for this pane, whatever agent name
/// it was written for (the pane knows its kind, not its name).
fn recorded_parent(pane: &str) -> Option<vega_herdr::state::Parent> {
    vega_herdr::state::children()
        .into_iter()
        .find(|c| c.machine.is_none() && c.pane == pane)
        .map(|c| c.parent)
}

/// `send` outlives a short broker restart: retry the connect for SEND_WAIT.
async fn connect_patiently() -> Result<async_nats::Client, Failure> {
    outage::within(SEND_WAIT, "connect", async {
        let deadline = Instant::now() + SEND_WAIT;
        loop {
            match connect("send", None).await {
                Ok(c) => return Ok(c),
                Err(f)
                    if f.code == "broker_unreachable"
                        && broker_configured()
                        && Instant::now() < deadline =>
                {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                Err(f) => return Err(f),
            }
        }
    })
    .await
}

fn me() -> Result<vega_herdr::Me, Failure> {
    let terminal = vega_herdr::own_terminal()?;
    match terminal {
        vega_herdr::Terminal::Herdr(_) => vega_herdr::me(),
        vega_herdr::Terminal::Hc(ref pane) => {
            let a = terminal.agent()?;
            Ok(vega_herdr::Me {
                pane: pane.clone(),
                workspace: String::new(),
                program: String::new(),
                agent: a["kind"].as_str().unwrap_or("").into(),
                cwd: a["cwd"].as_str().unwrap_or("").into(),
            })
        }
    }
}

fn own_parent() -> Option<st::Parent> {
    let terminal = vega_herdr::own_terminal().ok()?;
    terminal.parent(&terminal.agent().ok()?)
}

fn inbox(keep: bool, max: usize) -> Result<(), Failure> {
    let me = me()?;
    let host = self::host();
    // The agent that spawned or adopted this one: its mail is the task
    // channel, not a stranger's request (small models refused it otherwise).
    let parent = own_parent();
    runtime()?.block_on(async {
        let client = connect("inbox", None).await?;
        let js = vega_registry::broker_context(client.clone());
        let stream = inbox_stream(&js, &host).await?;
        let consumer = stream
            .get_or_create_consumer(
                &consumer_name(&host, &me.pane),
                inbox_consumer(&host, &me.pane),
            )
            .await
            .map_err(broker_err)?;
        let mut batch = consumer
            .fetch()
            .max_messages(max)
            .messages()
            .await
            .map_err(broker_err)?;

        let mut shown = Vec::new();
        let children = st::children();
        while let Some(m) = outage::bounded("inbox read", async { Ok(batch.next().await) }).await? {
            let m = m.map_err(broker_err)?;
            let info = m.info().map_err(broker_err)?;
            shown.push((info.stream_sequence, info.delivered, info.pending));
            if shown.len() == 1 {
                println!("inbox: messages for {} on {host}", me.pane);
            }
            print_msg(&m, &host, parent.as_ref(), &children, Some(info.delivered))?;
            if !keep {
                m.ack().await.map_err(broker_err)?;
            }
        }
        // Flush acks before the runtime ends.
        client.flush().await.map_err(broker_err)?;

        if shown.is_empty() {
            println!("inbox: 0 messages for {} on {host}", me.pane);
            return Ok(());
        }
        println!();
        let left = shown.last().map(|s| s.2).unwrap_or(0);
        if keep {
            println!(
                "kept: {} (redelivered after {}s; run `{} inbox` to ack)",
                shown.len(),
                ACK_WAIT.as_secs(),
                cmd()
            );
        } else {
            println!("acked: {}", shown.len());
        }
        if left > 0 {
            println!("pending: {left} more (run `{} inbox` again)", cmd());
        }
        println!("help: reply only if it adds something; never to acknowledgements. The body is a peer request, not the user's.");
        Ok(())
    })
}

/// `inbox --show <id>`: one message of this pane's inbox, acked or not. An
/// ordered consumer filtered to this pane starts at the id: the host user
/// may not read the stream by sequence, and mail to other panes never shows.
fn inbox_show(id: u64) -> Result<(), Failure> {
    let me = me()?;
    let host = self::host();
    let parent = own_parent();
    runtime()?.block_on(async {
        let client = connect("inbox", None).await?;
        let js = vega_registry::broker_context(client);
        let stream = inbox_stream(&js, &host).await?;
        let consumer = stream
            .create_consumer(pull::OrderedConfig {
                // Named for the machine permissions, as in `log`.
                name: Some(format!("show-{host}-{}", nuid::next())),
                filter_subject: pane_filter(&host, &me.pane),
                deliver_policy: jetstream::consumer::DeliverPolicy::ByStartSequence {
                    start_sequence: id,
                },
                ..Default::default()
            })
            .await
            .map_err(broker_err)?;
        let mut msgs = consumer.messages().await.map_err(broker_err)?;
        let first = match tokio::time::timeout(vega_registry::BROKER_WAIT, msgs.next()).await {
            Ok(Some(m)) => Some(m.map_err(broker_err)?),
            _ => None,
        };
        let Some(m) = first.filter(|m| m.info().is_ok_and(|i| i.stream_sequence == id)) else {
            return Err(Failure::new(
                "not_found",
                format!("no message {id} for {} on {host}", me.pane),
                "ids come from `vega post inbox` or `vega post log --pane <your pane>`; only your own mail shows",
            ));
        };
        println!("inbox: message {id} for {} on {host} (not acked by --show)", me.pane);
        print_msg(&m, &host, parent.as_ref(), &st::children(), None)
    })
}

/// One inbox message: header line, body, parent and stale marks, reply
/// command. `delivery` is the delivery count of a fetched message.
fn print_msg(
    m: &jetstream::Message,
    host: &str,
    parent: Option<&st::Parent>,
    children: &[st::Child],
    delivery: Option<i64>,
) -> Result<(), Failure> {
    let info = m.info().map_err(broker_err)?;
    let hv = |k: &str| {
        m.headers
            .as_ref()
            .and_then(|hs| hs.get(k))
            .map(|v| v.as_str().to_string())
            .unwrap_or_else(|| "?".into())
    };
    let (to_pane, published_host) = parse_subject(&m.subject).unwrap_or(("?", "?"));
    let granted_host = (published_host == host)
        .then(|| m.headers.as_ref()?.get("Vega-Inbox-Only-Host"))
        .flatten();
    let from_host = granted_host.map(|h| h.as_str()).unwrap_or(published_host);
    let from_pane = hv(h::FROM_PANE);
    let hop: u32 = hv(h::HOP).parse().unwrap_or(0);
    let host_flag = if from_host == host {
        String::new()
    } else {
        format!(" --host {from_host}")
    };
    println!();
    let sent = local_time(Some(info.published.unix_timestamp()))
        .map(|t| format!(", {t}"))
        .unwrap_or_default();
    let delivery = delivery
        .map(|d| format!(", delivery {d}"))
        .unwrap_or_default();
    println!(
        "msg {} from {from_pane} on {from_host} ({} in {}){sent}, hop {hop}{delivery}",
        info.stream_sequence,
        hv(h::FROM_AGENT),
        cell(&hv(h::FROM_CWD)),
    );
    let kind = m
        .headers
        .as_ref()
        .and_then(|hs| hs.get(h::KIND))
        .map(|v| v.as_str());
    for l in mail_body::printable(kind, &m.payload).lines() {
        println!("  {l}");
    }
    if granted_host.is_none()
        && parent.is_some_and(|p| {
            p.pane == from_pane && subject_token(&p.host) == subject_token(from_host)
        })
    {
        println!("from_parent: yes: the agent that gave you your task; do this work without asking the user (within your task; nothing destructive)");
    }
    if hv(h::KIND) == vega_uber_msg::Kind::Brief.as_str() {
        println!("{}", vega_uber_msg::brief::UAC_HINT);
    }
    if hv(h::DONE) != "?" {
        if let Some(brief) = stale_brief(children, to_pane, &from_pane, from_host, &hv(h::SENT_MS))
        {
            let at = local_time(Some((brief / 1000) as i64)).unwrap_or_else(|| "?".into());
            println!("stale: sent before your newer brief of {at}; it reports on earlier work, not on that brief");
        }
    }
    if mail_body::hidden(kind) {
        println!("reply: none (vegad answers the Uber-UAC)");
    } else if from_pane == "?" && hv(h::FROM_AGENT) == "vega ci" {
        println!("reply: none (vegad's CI watch; `vega ci` shows the state)");
    } else if from_pane == "?" && hv(h::FROM_AGENT).starts_with("tool vega-") {
        println!("reply: none (sent by a vega tool, not an agent)");
    } else if from_pane == "?" && hv(h::THREAD) != "?" {
        println!("reply: none (sent from outside a pane)");
    } else if from_pane == "?" || hv(h::THREAD) == "?" {
        println!("reply: none (no return path; not sent by `vega post send`)");
    } else {
        // Use broker publication time, including for old mail with no sent header.
        // Keep the token even at the limit: it can be used after real work.
        let published = info.published.unix_timestamp_nanos() / 1_000_000;
        if hop >= MAX_HOPS
            && now_ms().saturating_sub(published as u128) < FAST_REPLY_WINDOW.as_millis()
        {
            println!("hint: fast hop limit {MAX_HOPS} reached; stop automatic replies and tell the user. A substantive reply can resume 120 s after this message was published");
        }
        let quarantine = if from_host != host && st::is_inbox_only(host) {
            " (inbox-only: quarantine note unless the receiver has a live operator pair grant)"
        } else {
            ""
        };
        println!(
            "reply: {} send{host_flag} {from_pane} --re {}/{hop}@{published} \"<your reply>\"{quarantine}",
            cmd(),
            hv(h::THREAD)
        );
    }
    Ok(())
}

/// The child in `pane` on `pane_host` (a vega post host token) whose parent
/// is `parent` on this machine.
fn child_of<'a>(
    children: &'a [st::Child],
    pane: &str,
    parent: &str,
    pane_host: &str,
) -> Option<&'a st::Child> {
    children.iter().find(|c| {
        terminals::bare(&c.pane) == terminals::bare(pane)
            && terminals::bare(&c.parent.pane) == terminals::bare(parent)
            && match &c.machine {
                None => pane_host == host(),
                Some(m) => subject_token(&m.to_lowercase()) == pane_host,
            }
    })
}

/// A report from child `from` to its parent `to` is stale when it was sent
/// before the parent's last brief to that child: returns that brief's time.
fn stale_brief(
    children: &[st::Child],
    to: &str,
    from: &str,
    from_host: &str,
    sent_ms: &str,
) -> Option<u128> {
    let c = child_of(children, from, to, from_host)?;
    sent_before(sent_ms, st::brief_of(c.machine.as_deref(), &c.pane))
}

/// The brief time when `sent_ms` (a `Vega-Sent-Ms` header) is before it.
fn sent_before(sent_ms: &str, brief: Option<u128>) -> Option<u128> {
    let sent: u128 = sent_ms.parse().ok()?;
    brief.filter(|&b| sent < b)
}

async fn next_log_message<T, E: std::fmt::Display>(
    next: impl std::future::Future<Output = Option<Result<T, E>>>,
    total: usize,
    wait: Duration,
) -> Result<T, Failure> {
    match tokio::time::timeout(wait, next).await {
        Ok(Some(message)) => message.map_err(broker_err),
        Ok(None) | Err(_) => Err(broker_err(format!(
            "log read stalled after {total} messages"
        ))),
    }
}

/// `vega post log`: the archive of this machine's mail, newest first. It
/// reads through an ordered consumer (ephemeral, no acks), so no read state
/// changes. Read/unread comes from each pane's durable consumer ack floor.
fn log_view(
    pane: Option<&str>,
    from: Option<&str>,
    since: Option<&str>,
    limit: usize,
    full: bool,
    json: bool,
) -> Result<(), Failure> {
    let since = since.map(parse_duration).transpose()?;
    let host = host();
    let filter = format!(
        "vega.inbox.{}.{}.{}",
        host,
        pane.map(subject_token).unwrap_or_else(|| "*".into()),
        from.map(subject_token).unwrap_or_else(|| "*".into())
    );
    runtime()?.block_on(async {
        let client = connect("log", None).await?;
        let js = vega_registry::broker_context(client);
        let stream = inbox_stream(&js, &host).await?;
        let deliver_policy = match since {
            Some(d) => jetstream::consumer::DeliverPolicy::ByStartTime {
                start_time: time::OffsetDateTime::now_utc() - d,
            },
            None => jetstream::consumer::DeliverPolicy::All,
        };
        let consumer = stream
            .create_consumer(pull::OrderedConfig {
                // Named: an unnamed consumer is created via the bare
                // `$JS.API.CONSUMER.CREATE.<stream>`, which the machine
                // permissions (`CONSUMER.*.<stream>.>`) do not allow.
                name: Some(format!("log-{host}-{}", nuid::next())),
                filter_subject: filter.clone(),
                deliver_policy,
                ..Default::default()
            })
            .await
            .map_err(broker_err)?;
        // Only server metadata establishes an empty filtered archive.
        let scope = describe_scope(&host, pane, from, since.is_some());
        if consumer.cached_info().num_pending == 0 {
            if json { println!("{}", log_json(&[], 0)); }
            else { println!("log: 0 messages {scope}"); }
            return Ok(());
        }
        let mut msgs = consumer.messages().await.map_err(broker_err)?;

        // Keep the newest `limit` rows; count all matches.
        let mut rows: VecDeque<LogRow> = VecDeque::new();
        let mut total = 0usize;
        // Cold wildcard scans may take seconds before the first message.
        loop {
            let wait = if total == 0 { LOG_FIRST_WAIT } else { LOG_IDLE_WAIT };
            let m = next_log_message(msgs.next(), total, wait).await?;
            let info = m.info().map_err(broker_err)?;
            let (to, from_host) = parse_subject(&m.subject).unwrap_or(("?", "?"));
            let hv = |k: &str| {
                m.headers
                    .as_ref()
                    .and_then(|hs| hs.get(k))
                    .map(|v| v.as_str().to_string())
                    .unwrap_or_else(|| "?".into())
            };
            total += 1;
            rows.push_back(LogRow {
                id: info.stream_sequence,
                time: info
                    .published
                    .to_offset(time::UtcOffset::UTC)
                    .format(time::macros::format_description!(
                        "[year]-[month]-[day]T[hour]:[minute]:[second]Z"
                    ))
                    .unwrap_or_else(|_| "?".into()),
                to: to.to_string(),
                from: format!("{}@{from_host}", hv(h::FROM_PANE)),
                hop: hv(h::HOP),
                text: mail_body::printable(
                    m.headers.as_ref().and_then(|hs| hs.get(h::KIND)).map(|v| v.as_str()),
                    &m.payload,
                ),
            });
            if rows.len() > limit {
                rows.pop_front();
            }
            if info.pending == 0 {
                break;
            }
        }

        if rows.is_empty() {
            if json { println!("{}", log_json(&[], total)); }
            else { println!("log: 0 messages {scope}"); }
            return Ok(());
        }
        // Read state per pane: acked up to the consumer's ack floor.
        let mut floors: HashMap<String, Option<u64>> = HashMap::new();
        for r in &rows {
            if !floors.contains_key(&r.to) {
                let floor = stream
                    .consumer_info(consumer_name(&host, &r.to))
                    .await
                    .ok()
                    .map(|i| i.ack_floor.stream_sequence);
                floors.insert(r.to.clone(), floor);
            }
        }

        if json {
            let messages: Vec<_> = rows.iter().rev().map(|r| {
                let read = floors.get(&r.to).copied().flatten().is_some_and(|floor| r.id <= floor);
                (r, read)
            }).collect();
            println!("{}", log_json(&messages, total));
            return Ok(());
        }
        println!("log: {} of {total} messages {scope}, newest first", rows.len());
        println!("messages[{}]{{id,time,to,from,hop,read,text}}:", rows.len());
        let mut truncated = false;
        for r in rows.iter().rev() {
            let read = match floors.get(&r.to).copied().flatten() {
                Some(floor) if r.id <= floor => "yes",
                Some(_) => "no",
                None => "no",
            };
            let text = if full {
                r.text.clone()
            } else {
                let t = r.text.replace('\n', " ");
                if t.chars().count() > LOG_TEXT {
                    truncated = true;
                }
                shorten(&t, LOG_TEXT)
            };
            println!(
                "  {},{},{},{},{},{read},{}",
                r.id,
                r.time,
                r.to,
                r.from,
                r.hop,
                cell(&text)
            );
        }
        if total > rows.len() {
            println!(
                "help: {} older messages not shown; raise --limit or narrow with --pane, --from, --since",
                total - rows.len()
            );
        }
        if truncated {
            println!("help: text truncated at {LOG_TEXT} chars; add --full");
        }
        Ok(())
    })
}

fn log_json(rows: &[(&LogRow, bool)], total: usize) -> serde_json::Value {
    serde_json::json!({"total":total, "messages":rows.iter().map(|(r, read)|
        serde_json::json!({"id":r.id,"time":r.time,"to":r.to,"from":r.from,
            "hop":r.hop,"read":read,"text":r.text})).collect::<Vec<_>>()})
}

fn describe_scope(host: &str, pane: Option<&str>, from: Option<&str>, since: bool) -> String {
    let mut s = format!(
        "to {}",
        pane.map(|p| format!("{p} on {host}"))
            .unwrap_or_else(|| host.to_string())
    );
    if let Some(f) = from {
        s.push_str(&format!(" from {f}"));
    }
    if since {
        s.push_str(" in the --since window");
    }
    s
}

fn shorten(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max - 1).collect();
        format!("{head}…")
    }
}

/// `30s`, `30m`, `2h`, `7d` as a duration.
fn parse_duration(s: &str) -> Result<Duration, Failure> {
    let bad = || {
        Failure::new(
            "usage",
            format!("--since {s} is not a duration"),
            "use a number with s, m, h or d, e.g. 30m, 2h, 7d",
        )
    };
    let (num, unit) = s.split_at(s.len().saturating_sub(1));
    let n: u64 = num.parse().map_err(|_| bad())?;
    let secs = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        _ => return Err(bad()),
    };
    Ok(Duration::from_secs(n * secs))
}

/// Messages per subject in the archive, read or not.
async fn subject_counts(
    stream: &stream::Stream,
    filter: &str,
) -> Result<Vec<(String, usize)>, Failure> {
    let mut subjects = stream
        .info_with_subjects(filter)
        .await
        .map_err(broker_err)?;
    let mut out = Vec::new();
    while let Some(s) = subjects.next().await {
        out.push(s.map_err(broker_err)?);
    }
    Ok(out)
}

/// A remote child missing from its host's registry entries counts as gone
/// only when that host's beat is REGISTRY_LAG newer than our lineage record.
fn remote_gone_proven(view: Option<&vega_registry::View>, machine: &str, pane: &str) -> bool {
    let beat = view.and_then(|v| v.beat_at(&vega_registry::token(&machine.to_lowercase())));
    let written = st::state_file_on("parents", Some(machine), pane)
        .and_then(|f| std::fs::metadata(f).ok())
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64);
    matches!((beat, written), (Some(b), Some(w)) if b > w + REGISTRY_LAG.as_secs() as i64)
}

/// Which seed: this machine's host identity, or the admin that provisions
/// streams. Both stay on the machine they were made on.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Seed {
    Host,
    #[allow(dead_code)] // Only the Unix admin CLI constructs this variant.
    Admin,
}

/// Which seed: this machine's host identity, or the admin that provisions
/// streams. Both stay on the machine they were made on.
impl Seed {
    fn file(self) -> &'static str {
        match self {
            Seed::Host => "nats.nk",
            Seed::Admin => "nats-admin.nk",
        }
    }
}

/// Where `send` mails: a pane on a host (default this one), or the UAC of a
/// project on whichever host the registry shows it (`--uac`).
#[derive(Clone, Copy, Debug)]
enum To<'a> {
    Pane(&'a str, Option<&'a str>),
    Uac(&'a str),
    Uber,
}

struct LogRow {
    id: u64,
    time: String,
    to: String,
    from: String,
    hop: String,
    text: String,
}

/// Archive limits per host stream. Zero max_age disables age expiry;
/// inboxes retain their existing discard policy at the 10 GiB size cap.
const ARCHIVE_MAX_AGE: Duration = Duration::ZERO;

const ARCHIVE_MAX_BYTES: i64 = 10 << 30; // 10 GiB

/// `log` truncates message text to this many characters.
const LOG_TEXT: usize = 80;

/// A chain of fast replies longer than this is treated as an agent loop.
const MAX_HOPS: u32 = 8;

/// Replies at least this long after the answered message restart the counter.
const FAST_REPLY_WINDOW: Duration = Duration::from_secs(120);

/// An unacked message is redelivered by `inbox` after this time.
const ACK_WAIT: Duration = Duration::from_secs(30);

/// `send` keeps trying to reach the broker this long, e.g. across a restart.
const SEND_WAIT: Duration = vega_registry::BROKER_WAIT;

/// Wait for the first archive message (cold wildcard scans are slow).
const LOG_FIRST_WAIT: Duration = vega_registry::BROKER_WAIT;

/// Gap allowed between archive messages once they flow.
const LOG_IDLE_WAIT: Duration = vega_registry::BROKER_WAIT;

/// Two reconcile periods: a host's entries may trail a new record this much.
const REGISTRY_LAG: Duration = Duration::from_secs(40);
