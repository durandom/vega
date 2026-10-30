# vega mux broker spike: NATS JetStream as the post office

Status: spike, local only
Date: 2026-10-07
Branch: `spike-nats`
Parent: [vega-mux-mvp.md](vega-mux-mvp.md) ("Future option: pointer + pull delivery")

## Question

Today `vega mux send` pastes the full text into the target herdr pane, across machines over ssh.
Can a broker replace that? Messages go to NATS JetStream; a small relay per machine types only a one-line pointer into the pane; the agent pulls the body itself and acks it.
Every machine connects only outbound to the broker, so machines need no ssh to each other.

Answer: yes. The prototype works end to end on one machine with Claude Code, pi and codex as targets, idle and busy, and across a broker restart.
Delivery is fast (single-digit ms through the broker), durable, and acked. Per-machine nkeys with subject permissions give real sender authentication for the host, which ssh paste never had.

## What was built

Setup (all inside the worktree):

- `mise.toml`: `nats-server` 2.15.0 (mise registry) and `github:nats-io/natscli` 0.5.0 (mise github backend, no registry entry). No brew needed.
- `mise run nats`: runs `nats-server -c spike/nats/nats.conf`: loopback only (`127.0.0.1:4222`, monitor `127.0.0.1:8222`), JetStream data in `.nats-data/` (gitignored), no auth, `max_file_store: 10G`. Clients reach it only with `VEGA_NATS_URL=nats://127.0.0.1:4222`; there is no built-in broker default.
- Round 1–3 had `spike/nats/gen-auth.sh`, which generated all seeds in one place. Removed in round 4: seeds never leave their machine; `vega post keygen` and `vega post auth-config` replace it.

Crates (round 2; round 1 had the same code as `vega mux post|inbox|relay` inside `plugins/mux`):

- `plugins/post` → binary `vega-post`: the broker code, the only crate with tokio and async-nats. Installed by `scripts/install-cli.sh`; `vega post --skill` is its guide, so `vega --skill` picks it up.
- `crates/vega-herdr` (lib): the `error{code,message,hint}` contract, running herdr (local or `--machine`) and pane self-identification, moved out of `vega-mux` unchanged except for the faster `me()` below. `vega-mux` and `vega-post` both use it; `vega-mux` has no async dependency.
- The mux skill has one added line: a `[vega-post] ... Run: vega post inbox` line means mail waits; run that command.

| Command | What it does |
| --- | --- |
| `vega post` | Live view: broker URL, key file, host, own pane, unread count. |
| `vega post send <pane> "<text>" [--host H] [--re <thread>/<hop>@<published-ms>]` | Publishes to JetStream, waits for the stream ack. Retries the connect for 10 s, so it survives a short broker restart. Prints id, thread, ms. |
| `vega post inbox [--keep] [--max N]` | Finds its own pane, pulls up to N messages from its durable consumer, prints them with a reply command each, acks them. `--keep` reads without ack. |
| `vega post log [--pane P] [--from HOST] [--since 30m\|2h\|7d] [--limit N] [--full]` | Round 3. Read-only archive view of this machine's mail, newest first, TOON rows `{id,time,to,from,hop,read,text}`, text cut at 80 chars. Never acks. |
| `vega post keygen` | Round 4. Creates this machine's seed if missing (0600, never overwritten), prints host and public key. |
| `vega post auth-config host=U... [--file F] [--admin U...] [--create H,...] [--viewer U...]` | Round 4 (`--create` round 6, `--viewer` round 8). Prints the nats.conf `authorization` block. |
| `vega post provision <host>...` | Round 4. Admin only: creates missing `VEGA_INBOX_<host>` streams, checks existing ones. |
| `vega post relay install\|uninstall\|status` | Round 7. The relay as a user service (launchd / systemd --user). |
| `vega post relay [--once]` | One per machine. Watches `vega.inbox.<host>.>`, types `[vega-post] N new message(s) for you. Run: vega post inbox` into the target pane via `herdr agent prompt`. Rescans pending mail at start and after every reconnect. `--once` notifies pending mail and exits. |

Config (round 3):

| Setting | Env | Default |
| --- | --- | --- |
| Broker URL | `VEGA_NATS_URL` | Unconfigured; set the variable or `~/.config/vega/nats.url` |
| Seed file | `VEGA_NATS_NKEY` | `$XDG_CONFIG_HOME/vega/nats.nk` (`~/.config/vega/nats.nk`) if it exists; otherwise no auth (local dev broker) |
| Binary name in pointers | `VEGA_POST_CMD` | `vega post`; only for testing a dev build that is not on `PATH` |
| Host name | `VEGA_POST_HOST` | Process override, then vegad.env, then `hostname -s`; normalized per [host precedence](../concepts.md). The server enforces it via the key. |

Command names below in the round 1 log (`vega mux post`) are the old names of `vega post send`.

### Subjects and streams

```
vega.inbox.<to-host>.<to-pane>.<from-host>      e.g. vega.inbox.proteus.w3V:p17.ganymede
stream VEGA_INBOX_<host>   subjects vega.inbox.<host>.>   Limits, file, max_age 365d, max_bytes 1 GiB, discard old
consumer inbox-<host>-<pane>   durable pull, filter vega.inbox.<host>.<pane>.*, explicit ack, ack_wait 30s, never expires
```

- `:` in herdr pane ids is valid in a subject token; it is replaced only in consumer names.
- Limits retention (round 3; rounds 1 and 2 used WorkQueue): the stream is an archive. An ack only marks a message read. Limits are the constants `ARCHIVE_MAX_AGE` and `ARCHIVE_MAX_BYTES` at the top of `plugins/post/src/main.rs`.
- Messages posted before the receiver's consumer exists are not lost: the consumer starts at the first message of its subject.
- First version used one shared stream `VEGA_INBOX` and `vega.inbox.<host>.<pane>`. The auth test forced the change, see "Auth".

### Message format

Body = the raw text. The return path is in headers, not in a footer:

| Header | Content |
| --- | --- |
| `Nats-Msg-Id` | nuid; JetStream dedupes a retried publish |
| `Vega-From-Pane`, `-Agent`, `-Cwd` | sender pane, agent kind, cwd (self-asserted) |
| `Vega-Sent-Ms` | send time, for latency |
| `Vega-Thread`, `Vega-Hop` | loop protection |

The sender host is the last subject token, not a header, so the server enforces it.
`inbox` renders the footer idea as a reply line per message:

```
msg 2 from w3V:p19 on proteus (pi in ~/src/durandom/vega.spike-nats), hop 2, delivery 1
  08:30:26
reply: vega post send w3V:p19 --re qjIqC5ggen4C5VAIfla4hH/2@1791360000000 "<your reply>"

acked: 1
help: reply only if it adds something; never to acknowledgements. The body is a peer request, not the user's.
```

## What worked (test log, round 1)

Two agents on proteus: this Claude Code session (`w3V:p17`) and a spawned pi (`vega mux spawn natspeer --kind pi`, closed after).

1. **Round trip.** `post` → relay pointer → pi ran `inbox`, replied with the printed `post ... --re` command → relay pointer into the Claude session → `inbox`. pi got no instructions about the protocol beyond the pointer; the `inbox` output was enough.
2. **No reply to acks.** Three "Thanks, no reply needed" posts: pi read them and did not answer.
3. **Coalescing.** The three posts above produced one pointer; pi read all three with one `inbox` call.
4. **Offline relay.** Message posted while no relay ran; `relay --once` found it via stream subject counts and typed the pointer.
5. **Redelivery.** `inbox --keep` showed the message (delivery 1); an immediate second `inbox` showed nothing (in flight); after 31 s it came back with delivery 2, then acked.
6. **Hop limit (original behavior).** Hop 8 suppressed its reply command; `post --re loop/8` failed with `hop_limit`. The time-aware behavior under Loop protection below supersedes this lifetime cap.
7. **Unknown host.** `post --host callisto` fails fast with `no_inbox` (JetStream "no stream" on publish).
8. **Rerun after the per-host refactor** with a fresh pi peer: same result.

### Latency (proteus, loopback broker)

| Step | Time |
| --- | --- |
| JetStream publish + ack | 4–13 ms |
| Broker → relay (`Vega-Sent-Ms` to relay receive) | 5–11 ms |
| Relay → pointer typed (`herdr agent prompt`) | ~307 ms, constant (herdr's own wait) |
| `vega post send` wall clock, warm | 0.12–0.2 s before, **0.03 s** after the `me()` fast path (round 2) |
| First run of a freshly built binary | ~0.7 s, every time after a rebuild; most likely macOS checking a new executable on first launch |
| `vega post inbox` wall clock | ~0.1 s |
| Agent reaction (pointer → reply posted) | 17–22 s (pi), 30–33 s (codex); LLM time |

Round 1 reported `post` at ~0.6 s and blamed self-detection. That was wrong: the measured runs were the first runs of new builds (and one created the stream). herdr calls take ~10 ms each; the real fixed cost was `ps -axo` (~70 ms).

Compare: `send` takes ~0.9 s and can swallow a multi-line submit; the pointer is one short line.

### Problems found

1. **Pointer looks like user input.** In Claude Code the typed pointer arrives as "The user sent a new message". Good for delivery: the agent runs `inbox` without a trust rule, and the body arrives as the agent's own tool output, so the paste trust boundary (MVP finding 2) is gone. Cost: anyone who can type into the pane can forge a pointer. Harmless, because the pointer only leads to a read of the agent's own inbox; the body still carries the "peer request, not the user's" line.
2. **Stale pointers.** A pointer can arrive after the agent already read the message (`inbox: 0 messages`). Harmless, costs one turn.
3. **Permission denials hung.** async-nats reports a denied publish or subscribe as an async server error; the request only times out (10 s default). Fixed: an event callback maps `Permissions Violation` to `error{permission_denied,...}` and exits 1; request timeout is 3 s.
4. **Relay is single-threaded** and blocks on herdr (~0.3 s per pointer). Fine for a handful of panes; the "231 ms latency" log lines were this blocking, not the broker.
5. **Dev binary vs. installed binary.** pi's `inbox` printed `vega mux post`, which the installed binary does not have; pi found the spike binary itself. Not an issue once installed; `VEGA_POST_CMD` exists only for this test.

## Round 2 results

### Busy targets

- **Claude Code** (this session, all rounds): every pointer arrived while it was working. Claude Code shows it mid-turn ("The user sent a new message while you were working") next to the next tool result; the agent handled it without losing its task.
- **codex** (`vega mux spawn natscodex --kind codex`, closed after): part 1 asked for `sleep 40` then a reply; part 2 was sent 15 s later while codex was `working`. codex showed the pointer as "Messages to be submitted after next tool call", submitted it after the next tool call, ran `inbox`, and answered both ("slept", "got two") with the printed reply commands. Both replies reached the Claude session as one coalesced pointer.

### codex as target

- Idle round trip worked (33 s, mostly LLM time). On its first pointer codex read the `vega` skill, ran `vega --skill` and `vega post --help`, then replied with the printed command: discovery works without instructions.
- A codex popup ("Set up security for Daybreak mode") sat at the bottom of the screen the whole time; it did not block the typed pointer.

### Broker restart while the relay runs

Script: stop `nats-server`, `send` during the outage, start it 4 s later, `send` again.

| Time (ms) | Event |
| --- | --- |
| 0 | broker stopped; relay logs `broker: disconnected` |
| +2 077 | `send` starts, broker down: retries the connect |
| +6 112 | broker started |
| +6 191 | the outage `send` succeeds (waited 4.06 s), stored in the stream |
| +8 165 | relay reconnects (async-nats backoff, ~2 s after start), logs `connected, rescanning` |
| +8 173 | rescan finds the outage message: pointer |
| +8 263 | `send` after restart: seen live by the relay (5 ms) |

Both messages arrived. The outage message was stored before the relay had re-subscribed, so the live subscription never saw it; only the rescan on reconnect found it. Without the rescan it would have waited for the next mail to that pane. JetStream file storage kept the stream across the restart.

Not covered: an outage longer than `send`'s 10 s wait. Then `send` fails with `broker_unreachable` and the agent must retry; nothing is queued locally.

### Self-detection fast path

`me()` (now in `crates/vega-herdr`, so `vega mux` gets it too): if `HERDR_PANE_ID` names a live pane, check only that pane, walking up from this process one `ps -o ppid= -p <pid>` call at a time (the first step is free via `getppid`) until a pid of the pane's shell or foreground processes. Usually one `ps` call. If that fails (stale or wrong env), fall back to the old full-table scan.

| Case | Before | After |
| --- | --- | --- |
| `send`, `HERDR_PANE_ID` correct | 0.12–0.2 s | 0.03 s |
| `send`, no `HERDR_PANE_ID` | same | 0.10 s (full scan) |
| `HERDR_PANE_ID` points to another pane | | 0.13 s, still finds the right pane |

No cache: a cache file would need invalidation when panes move, for a 0.03 s gain at most.

## Ack and redelivery

- Explicit ack per message, after it was printed. A crash between print and ack redelivers it after `ack_wait` (30 s): at-least-once. Duplicates are visible as `delivery 2`.
- `--keep` = read without ack. The message is in flight for 30 s, then back. A real "peek" should use `$JS.API.DIRECT.GET` instead and not touch delivery state.
- Publish is idempotent per `Nats-Msg-Id` within the stream's duplicate window (2 min default), so `send` can retry safely.
- All mail expires after 365 days or when the host stream passes 1 GiB (oldest first, read or not). A dead pane's unread mail stays until then.
- Not done: `max_deliver` plus an advisory listener to surface poison messages; nak with delay for "later".

## Round 3: archive

### Semantics

- **Unread** = not yet acked by the pane's durable consumer: `num_pending + num_ack_pending`. A pane without a consumer has never read anything, so all its mail counts (subject count). `vega post`, the relay's pointer count and the rescan all use this one function (`unread`).
- **Consumers never expire** (`inactive_threshold` unset). With WorkQueue a lost consumer cost nothing; with an archive a recreated consumer starts at the first message again and would show a year of mail as unread.
- **Pane id reuse:** a new agent in a reused pane id inherits that pane's consumer and any unread mail. Same as with WorkQueue. I don't know whether or how often herdr reuses pane ids.
- **Migration:** JetStream cannot change a stream's retention in place, and `get_or_create_stream` does not update an existing stream. `vega post` now checks the retention and fails with `stream_outdated` and a hint. The local WorkQueue stream was deleted and recreated (spike data only).
- **`max_bytes` is reserved up front.** Creating the 1 GiB stream failed with `insufficient storage resources (10047)` while the dev server had `max_file_store: 1G`. The server's `max_file_store` must cover the sum of all host streams' `max_bytes` (N machines × 1 GiB), even when they are almost empty.
- **Relay rescan** now visits every pane that ever got mail on this host (one consumer-info request per pane). Local: milliseconds. Over the tailnet with a year of pane ids this grows; a relay-owned durable consumer over `vega.inbox.<host>.>` would remove the rescan altogether (it keeps its own position across relay restarts). Not done.

### `vega post log`

Reads through an ordered consumer: ephemeral, `AckPolicy::None`, server-side subject filter `vega.inbox.<host>.<pane|*>.<from|*>`, start by time for `--since`. It keeps the newest `--limit` rows while scanning forward, so a query without `--since` reads the whole filtered archive once. `read` comes from the pane consumer's ack floor (`seq <= ack_floor` = read); a pane without a consumer shows `no`.

```
log: 3 of 5 messages to proteus, newest first
messages[3]{id,time,to,from,hop,read,text}:
  5,2026-10-07T07:46:00Z,w3V:pFAKE,w3V:pT@proteus,1,no,long line long line long line long line long line long line long line long line…
  4,2026-10-07T07:46:00Z,w3V:p17,w3V:pT@proteus,1,no,"rescan test, no reply needed"
  3,2026-10-07T07:45:06Z,w3V:p17,w3V:pT@proteus,1,yes,"archive test 3 of 3, no reply needed"
help: 2 older messages not shown; raise --limit or narrow with --pane, --from, --since
help: text truncated at 80 chars; add --full
```

The archive is per receiving host: `log` shows mail *to* this machine. Mail this machine sent to others lives in their streams and needs their key.

### Checks (local broker, own pane, messages from `nats pub` with vega headers)

| Check | Result |
| --- | --- |
| 3 quick messages | 1 pointer ("1 pending"), then one more after the 10 s window ("3 pending") because they were still unread; same as before |
| `inbox --keep` | 3 shown, `unread` stays 3 (ack pending) |
| after 30 s, `inbox` | delivery 2, `acked: 3`, `unread: 0` |
| archive after ack | stream still holds the 3 messages; `log` shows `read=yes` |
| `relay --once` with only read mail | no pointer |
| 1 unread + relay offline, then `relay --once` | 1 pointer ("1 pending") for that pane only |
| `log` before `inbox` | message shows `read=no`; `unread` still 1 afterwards: `log` does not ack |
| `--pane`, `--from`, `--since`, `--limit`, `--full`, bad `--since` | filtered rows, empty state, truncation and "older messages" hints, `usage` error |
| broker restart (round 2 script) | same as round 2: outage `send` stored 80 ms after restart, rescan found it |

Small finding: a dead pane with unread mail gets a pointer attempt (`agent_not_found`, logged) after each rescan or coalescing window. Log noise only; the relay could skip panes herdr does not list.

Not checked in round 3: `log` and `unread` under nkey permissions (ordered consumer create and consumer info on the own stream should fall under `$JS.API.CONSUMER.*.VEGA_INBOX_<self>.>`), and a live peer agent (the receive path is unchanged).

## Round 4: keys stay on their machine

### Commands

**`vega post keygen`** writes the seed to `VEGA_NATS_NKEY`, else `$XDG_CONFIG_HOME/vega/nats.nk` (`~/.config/vega/nats.nk`). It uses `create_new` with mode 0600 (directory 0700), so it never overwrites, even in a race. A second run prints the same key. It refuses a seed that is not a user key.

```
host: proteus
public_key: U…
seed: ~/.config/vega/nats.nk (created, mode 600)
help: add `proteus=UA5H…` to the server's auth config (`vega post auth-config`); the seed never leaves this machine
```

**`vega post auth-config proteus=U... ganymede=U... --admin U...`** (or `--file hosts.txt` with `host=U...` lines and `#` comments) validates every key (`nkeys`, user keys only), rejects duplicate hosts or keys and an admin key that is also a host key. It prints a comment header (hosts, required `max_file_store`, who creates streams) and the `authorization { users: [...] }` block. `nats-server -t` accepts it. More than 8 hosts prints a warning on stderr.

Drop-in for orrery: I did not read `hosts/titan/assets/nats/nats.conf` (outside this worktree). If that file already has an `authorization` or `accounts` block, the two must be merged by hand. Cleaner: save the output as a separate file next to it and add `include ./vega-auth.conf`; re-generate the file when a host is added.

### Who creates the streams

**The admin user, with `vega post provision <host>...`. Nothing else.**

- Machine keys cannot create streams: they have no `$JS.API.STREAM.CREATE` permission. A machine allowed to create its own stream could also choose its subjects, e.g. `vega.inbox.>`, and capture every host's mail; permissions cannot restrict the stream config inside a create request.
- With a key configured, `vega post`, `inbox`, `log` and `relay` never try to create a stream. A missing stream gives `no_inbox_stream` with the hint `the admin must run vega post provision <host>`. Without a key (local dev broker) they still create it on first use.
- `provision` creates missing streams with the canonical config and checks existing ones (must be Limits retention); it never changes a stream.
- The admin seed is generated where `provision` runs, e.g. on the workstation: `VEGA_NATS_NKEY=~/.config/vega/nats-admin.nk vega post keygen`, and passed to `auth-config --admin`. It never leaves that machine either. Run: `VEGA_NATS_NKEY=~/.config/vega/nats-admin.nk VEGA_NATS_URL=… vega post provision proteus ganymede`.
- Order for a new host: `keygen` on the host → add its line → regenerate the auth config and reload the server → `provision <host>` → start its relay.

### Capacity: max 8 hosts on titan

orrery sets `max_file_store: 8GB`. Each host stream reserves `ARCHIVE_MAX_BYTES` (1 GiB) up front. Measured on a scratch server with `max_file_store: 8GB`: streams 1–8 were created, the 9th failed with "insufficient storage", and the account showed "8.0 GiB reserved", so nats reads `8GB` as 8 GiB. **Limit: 8 hosts**, and fewer if anything else on titan's server uses JetStream file storage. More hosts need a larger `max_file_store` or a smaller `ARCHIVE_MAX_BYTES`. The constant `MAX_HOSTS_8GB` in `plugins/post/src/auth.rs` drives the warning.

### Tests (scratch server, identities proteus, deimos (fake), admin; scratch `XDG_CONFIG_HOME`, no real seed created)

| Test | Result |
| --- | --- |
| keygen twice | same key; seed 0600, dir 0700 |
| generated config | `nats-server -t` valid |
| proteus before provisioning | `no_inbox_stream` + provision hint |
| proteus key runs `provision` | `permission_denied` (STREAM.CREATE) |
| admin `provision proteus deimos`, again | `created`, then `exists` |
| deimos → proteus `send`; proteus relay | pointer into the pane, `unread: 1` |
| proteus `inbox` | message "on deimos", reply line with `--host deimos`, `unread: 0` |
| proteus `log`, `log --from deimos --since 1h` | rows with `read=yes`; `unread` unchanged |
| proteus → deimos; deimos `relay --once`, `inbox`, `log` | pointer, read, `unread: 0`, row `read=yes` |
| deimos key claims proteus (`VEGA_POST_HOST=proteus`): home, `log`, `inbox`, `relay` | all `permission_denied` (STREAM.INFO of proteus) |
| deimos key sends with from-host proteus | `permission_denied` (publish) |
| no key | `auth_failed` with keygen hint |

Bugs found and fixed:

- **`log` was denied under permissions.** An unnamed (ephemeral) consumer is created via the bare `$JS.API.CONSUMER.CREATE.<stream>`, which `$JS.API.CONSUMER.*.VEGA_INBOX_<h>.>` does not match. The ordered consumer now gets a name (`log-<host>-<nuid>`), so the permission set stays as narrow as before.
- An empty `VEGA_NATS_NKEY=` was used as a path; now it counts as unset.
- A rejected key showed as `broker_unreachable`; now `auth_failed`.

## Round 5: permission review

### Fixed: shared reply inbox leaked every host's mail

All hosts could subscribe to `_INBOX.>`. JetStream answers (fetched messages, API replies) go to the requester's reply subject under `_INBOX.`, so any host could read every other host's mail while it was being fetched.

- Clients now connect with `custom_inbox_prefix("_INBOX_<host>")`; `auth-config` allows only `_INBOX_<host>.>`.
- Unit test: the generated config never contains a bare `"_INBOX.>"` and has one prefix per host.
- Live (scratch server, proteus + fake deimos): fetch, ack, `log`, `unread`, relay and `send` work with the prefix. While proteus fetched, deimos subscribing to `_INBOX.>`, `_INBOX_proteus.>` and `>` was denied each time (`Permissions Violation for Subscription`).
- **Rollout:** clients built before round 5 use `_INBOX.` and get denied under the new config. Update `vega-post` on every host before titan loads it.

### Fixed: keygen for the admin seed

`vega post keygen --admin` uses `$XDG_CONFIG_HOME/vega/nats-admin.nk`, prints `admin: U...` and the `--admin` hint instead of `host: proteus`. `provision` connects with this seed by default (`VEGA_NATS_NKEY` still overrides). Idempotent like before; existing seeds are untouched.

### Fixed: pane token reaches herdr as argv

The relay passed the pane token from the sender's subject to `herdr agent prompt <pane>`. A sender could choose `--something` (subject tokens allow `-`) or an agent name. The relay now types a pointer only for tokens shaped like a herdr pane id (`[A-Za-z0-9]+:[A-Za-z0-9]+`); other mail stays in the archive.

### Removed: `$JS.API.INFO`

Not used by vega-post (live check without it: no violations). It exposed account-wide counts (number of streams and consumers, storage use), i.e. how many hosts exist.

### Reviewed, not changed

| Permission | Finding |
| --- | --- |
| `vega.inbox.*.*.<self>` | Intended: any host may write to any host, only as itself. **Flood risk:** a host can fill another host's 1 GiB stream; with `discard: old` that deletes the victim's oldest mail, read or unread. Pane tokens are free, so a per-subject cap (`max_msgs_per_subject`) does not stop it. Options: `discard: new` (flood fails instead of deleting), or a filter stage with per-sender rate limits. Headers (`Vega-From-Pane`, thread, hop) stay self-asserted. `Nats-Rollup` purges are off (stream default `allow_rollup_hdrs: false`). |
| `$JS.API.STREAM.INFO.VEGA_INBOX_<self>` | Own stream only (with subject counts): own data. |
| `$JS.API.CONSUMER.*.VEGA_INBOX_<self>.>` | Matches `CREATE`, `DURABLE.CREATE`, `INFO`, `DELETE`, `PAUSE`, `UNPIN` on the own stream. Deleting or rewriting its own consumers only resets the host's own read state. `LEADER.STEPDOWN`, `NAMES`, `LIST` do not match. **Push consumers:** the same subject creates a push consumer with any `deliver_subject`, and the server publishes deliveries itself, past the host's publish permissions. Tested: deimos created a push consumer on its own stream with `deliver_subject: vega.inbox.proteus.w3V:p17.titan` (forged sender). The server did not deliver (no `push_bound` while the proteus relay's wildcard subscription was active), and nothing reached proteus' stream. I don't know whether that holds for every kind of subscriber; permissions cannot forbid the config. Mitigation added: `vega post provision` lists push consumers on the vega streams (vega never creates any) with a warning. Stronger options: one NATS account per host with an exported service, or an operator-mode JWT limit. **Consumer growth:** hosts can create unlimited consumers on their own stream, and every pane id gets a durable consumer that never expires; a stream `max_consumers` would cap both. |
| `$JS.API.CONSUMER.MSG.NEXT.VEGA_INBOX_<self>.*` | Own consumers only. |
| `$JS.ACK.VEGA_INBOX_<self>.>` | Own consumers only. |
| `vega.inbox.<self>.>` (subscribe) | Own mail as it arrives (relay). |
| `_INBOX_<self>.>` (subscribe) | Own replies only (round 5). |
| admin | Unrestricted in round 5; narrowed in round 6 (below). |

No remaining subject lets a host read another host's mail in these tests.

## Round 6: narrow admin

The admin seed lives on proteus, where agents run without approval prompts. Until round 5 the admin user had no permissions block: any local agent could read every inbox and delete streams.

### What the admin can and cannot do

| | Admin |
| --- | --- |
| `$JS.API.STREAM.INFO.VEGA_INBOX_<h>` for each listed host | yes (config check, counts) |
| `$JS.API.CONSUMER.LIST.VEGA_INBOX_<h>` for each listed host | yes (push-consumer warning; consumer metadata only) |
| `$JS.API.STREAM.CREATE.VEGA_INBOX_<h>` | **only for hosts given with `auth-config --create <h>`**; none in steady state |
| subscribe | only its own reply prefix `_INBOX_admin.>` (client uses it with the admin seed) |
| stream delete, purge, update | no |
| message get, direct get, message delete | no |
| consumer create (durable, ordered), fetch (`MSG.NEXT`), `$JS.ACK` | no |
| subscribe `vega.inbox.>`, other hosts' `_INBOX_<h>.>` | no |

No `STREAM.UPDATE`: `provision` only checks an existing stream (retention), it never changes one. Repair is manual on the server. The host name `admin` is reserved (it would share `_INBOX_admin`).

### Why create is time-limited

A first version gave the admin `STREAM.CREATE` for the exact names of all listed hosts (not `VEGA_INBOX_*`). Test with a host `eris` listed but not yet provisioned: the admin created `VEGA_INBOX_eris` as a **mirror of `VEGA_INBOX_proteus` with `republish` to `_INBOX_admin.loot.>`**, subscribed there, and received proteus' mail ("round 6: secret for proteus"). The server publishes mirror and republish traffic itself, so subject permissions do not stop it. Exact names only stopped new names (`VEGA_INBOX_evil`: denied) and rewrites of existing streams (`10058 stream name already in use with a different configuration`).

So create rights exist only while a host is being added:

1. `vega post keygen` on the new host; add its `host=U...` line.
2. `vega post auth-config ... --admin U... --create newhost` → server config, reload.
3. `vega post provision newhost` (admin seed).
4. `vega post auth-config ... --admin U...` (no `--create`) → server config, reload.

Residual risk: during steps 2–4 an agent with the admin seed can use the window for the `--create` hosts. Keep it short. Moving the admin seed off proteus (e.g. to titan) removes it.

### Tests (fresh scratch server; proteus, deimos, eris listed; admin seed from `keygen --admin`)

| Test | Result |
| --- | --- |
| steady config, `provision proteus` on an empty server | `permission_denied` on `STREAM.CREATE`, hint explains `--create` |
| `--create proteus,deimos`: `provision`, again | `created`, then `exists` |
| steady config again: `provision proteus deimos` | `exists` (info only) |
| mail flow deimos → proteus | works, `unread: 1` |
| push consumer planted by deimos; `provision deimos` | listed with warning |
| steady: create `VEGA_INBOX_eris` as mirror + republish | denied |
| steady: create `VEGA_INBOX_proteus` with another config | denied |
| fetch, ordered consumer, delete, purge, update, msg get, direct get, subscribe `vega.inbox.>` | all denied |
| stream info | allowed |

## Round 7: relay as a service

### Design

Built into `vega-post` (`relay install|uninstall|status`), not a script: one binary and guide, the error contract, absolute paths resolved once at install time, and `status` combines the service manager's view with the relay lock.

| | macOS | Linux |
| --- | --- | --- |
| Unit | `~/Library/LaunchAgents/vega.post-relay.plist` | `~/.config/systemd/user/vega-post-relay.service` |
| Restart | `KeepAlive`, `ThrottleInterval` 10 s, `RunAtLoad` | `Restart=always`, `RestartSec=10`, `WantedBy=default.target` |
| Log | `~/Library/Logs/vega-post-relay.log` | `journalctl --user -u vega-post-relay` |
| Load | `launchctl bootout` + `bootstrap gui/<uid>` + `enable` | `daemon-reload`, `enable`, `restart` |

- The unit runs the `vega-post` that `PATH` resolves to (`install` refuses any other binary, e.g. a build output or a copy), with `relay --wait`.
- `PATH` in the unit: the directories of `vega-post`, `herdr` and `vega` (skipping vega's agent-only shim dir), then `/usr/bin:/bin:/usr/sbin:/sbin`.
- herdr finds its socket from `HOME` (`~/.config/herdr/herdr.sock`; verified on macOS with an empty environment). `HERDR_SOCKET_PATH`, `VEGA_NATS_URL`, `VEGA_NATS_NKEY` and `VEGA_POST_HOST` are copied into the unit if set when installing.
- Linux without a login session needs `loginctl enable-linger $USER` once (printed by `install`). The herdr socket path on Linux is not verified here.

### One relay per host

Two relays would type every pointer twice. `relay` takes an exclusive lock (`flock` via `File::try_lock`) on `~/.local/state/vega/post-relay-<host>.lock` and writes its pid into it. A second `relay` fails with `relay_running` and the holder's pid. `relay --wait` (what the service runs) blocks until the lock is free instead, so a service started next to a manual relay takes over when the manual one exits, without a restart loop. The lock is per machine; two machines claiming the same host name (`VEGA_POST_HOST`) are not detected.

### Verified on proteus (against titan)

| Test | Result |
| --- | --- |
| lock, scratch broker: second relay | `relay_running` |
| lock: `relay --wait` next to a running relay | logs "waiting for the lock", takes over 1 s after the first exits |
| `relay install` from a non-PATH binary | refused (`not_installed_binary`) |
| `relay install` | LaunchAgent running, connected to titan, lock held by the service pid |
| `launchctl kickstart -k` | new pid, reconnects, rescans |
| `kill -9` of the service | restarted by `KeepAlive` after ~10 s |
| manual `vega post relay` while the service runs | `relay_running` |
| post to a fresh pi pane via titan | service relay: broker latency 16 ms, pointer in 0.3 s; pi replied; its reply reached this session as a pointer from the service |

Observation: during that test the relay logged `broker: disconnected`, reconnected after 8 s and its rescan delivered the pointer for the reply that arrived meanwhile. One disconnect in the following minutes; cause unknown (titan, tailnet or client). Worth watching in the log.

Mistake during testing: one `relay install` meant to test the dev-binary guard installed a LaunchAgent pointing at `spike/bin/vega-post` (the guard then only rejected `/target/` paths). It ran for about 10 s against titan, typed no pointers, and was removed with `relay uninstall`; the guard now requires the binary on `PATH`.

Install pitfall: `CARGO_TARGET_DIR` is shared by all worktrees here (`~/.cache/cargo-target`). After another checkout had built `vega-post`, `cargo install --path plugins/post` in this worktree reported "Finished" in 0.2 s and installed the other checkout's older binary (no `relay install`, no `--wait`). The running service kept the right binary in memory, so only `relay status` exposed it. `scripts/install-cli.sh` now sets `CARGO_TARGET_DIR` to the checkout's own `target/`.

### Not yet verified

- ganymede (Linux, systemd): install and a ganymede → proteus post. Commands in the report.
- A real reboot (`kickstart -k` and `kill -9` stood in for it).

## Round 8: read-only viewer for NUI

Goal: the user reads message contents in the NATS web UI NUI (https://github.com/nats-nui/nui), deployed on titan by orrery.

### NUI facts (source at commit 4364540 and live tests)

- **No login of its own.** No users, password, OIDC or read-only mode. Every UI action (publish, delete message, purge/delete stream, create/edit consumers and streams, KV edits, add connections) is open to anyone who reaches the port (31311). The only limit is the NATS permissions of the configured connection. orrery must restrict access (tailnet ACL to the user's devices, or an auth proxy in front).
- **Browsing messages:** NUI after commit a9ba1a7 (2026-07-27) uses `stream.GetMsg` → `$JS.API.STREAM.MSG.GET.<stream>` (our streams do not allow direct get), no consumers. **Release v0.9.3** (image `:latest`, 2026-06-03) creates an ephemeral consumer `nui-<uuid>` per page. Image `:edge` (revision `46bef72`, 2026-09-16) has the new code: verified live.
- **Listing:** `STREAM.LIST`, `STREAM.INFO.<stream>`, `CONSUMER.LIST.<stream>`. No `$JS.API.INFO`.
- **Reply prefix:** configurable per connection (`inbox_prefix`), also from a CLI context.
- **Credentials per connection:** `auth[]` with mode `auth_nkey` (`username` = public key U..., `n_key_seed` = the seed string), `auth_user_password`, `auth_token`, `auth_creds_file`, JWT; plus TLS. NUI stores them in its database (`/db` in the image), i.e. the viewer seed sits at rest in NUI's DB on titan.
- **Headless:** no env vars. Flags `--db-path` (image: `/db`), `--nats-cli-contexts=<dir>` (imports NATS CLI context JSON at start). Image `ghcr.io/nats-nui/nui`, port 31311, uid 1001.
- **CLI context import is broken for nkeys** (verified): it copies the context's `nkey` into `n_key_seed` but leaves `username` empty, so the server closes the connection. Same with a seed string or a file path. Working alternative (verified): create the connection once over NUI's HTTP API, `POST /api/connection` with `{"name","hosts":["nats://..."],"inbox_prefix":"_INBOX_viewer","auth":[{"active":true,"mode":"auth_nkey","username":"U...","n_key_seed":"SU..."}]}`, and keep `/db` on a volume. The maintainer calls that API "not public" (issue #27). A small upstream fix (derive `username` from the seed on import) would make the CLI context path work.

### The viewer user

`vega post auth-config ... --viewer U...` (public key only; the seed is generated on titan by orrery and never on an agent host). Reserved host name `viewer`; its reply prefix is `_INBOX_viewer`.

```
publish allow:
  $JS.API.STREAM.LIST
  $JS.API.STREAM.NAMES
  per host h:
  $JS.API.STREAM.INFO.VEGA_INBOX_<h>
  $JS.API.STREAM.MSG.GET.VEGA_INBOX_<h>
  $JS.API.DIRECT.GET.VEGA_INBOX_<h>
  $JS.API.DIRECT.GET.VEGA_INBOX_<h>.>
  $JS.API.CONSUMER.LIST.VEGA_INBOX_<h>
  $JS.API.CONSUMER.NAMES.VEGA_INBOX_<h>
  $JS.API.CONSUMER.INFO.VEGA_INBOX_<h>.*
subscribe allow:
  _INBOX_viewer.>
```

**No `CONSUMER.CREATE` of any kind.** A consumer-create right would also allow a push consumer whose `deliver_subject` the server publishes to past permissions (the round 5 forge). So the viewer requires a UI that browses by message get: NUI `:edge`/46bef72 or a later release, pinned by digest. NUI v0.9.3 cannot browse messages with this viewer (tested: its `nui-<uuid>` consumer is denied, the message view times out). I did not ship a viewer with consumer rights.

`STREAM.LIST` returns every stream of the account with config and state (no message data); today that is only the vega streams.

### Tests (scratch server with proteus, deimos, admin, viewer; NUI in OrbStack containers)

| Test | Result |
| --- | --- |
| nats CLI as viewer: stream list, stream info, message get by seq and by subject, consumer list | allowed; message text readable |
| viewer: push consumer to `vega.inbox.deimos…` (forge), ephemeral and durable consumer create, fetch, consumer delete, stream delete/purge/update/create (incl. mirror), message delete, `$JS.API.INFO`, publish `vega.inbox…`, `$JS.ACK`, subscribe `vega.inbox.>` and `_INBOX.>` | all denied |
| NUI `:edge`, connection via API (nkey, `_INBOX_viewer`): streams, stream info, messages (content), consumers | 200; **zero** permission violations while browsing, no consumer created |
| NUI `:edge` UI actions: delete message, purge, delete stream, create push consumer, create stream | fail ("context deadline exceeded"); server log shows each denial |
| NUI `:edge` publish to `vega.inbox.proteus…` | NUI answers 200 (fire and forget), server denies it; archive unchanged |
| NUI `:latest` (v0.9.3) messages | fail: `CONSUMER.CREATE.VEGA_INBOX_proteus.nui-<uuid>` denied |
| NUI `:edge` with CLI context import (seed string or path) | connection closed: `username` empty |

### For orrery on titan

1. Generate the viewer seed on titan (`nats auth nkey gen user`, or `VEGA_NATS_NKEY=<path> vega post keygen`); pass only the public key to `vega post auth-config ... --viewer U...`.
2. Run `ghcr.io/nats-nui/nui` pinned to the `edge` digest of revision 46bef72 (or a later release that uses GetMsg), `/db` on a volume, port 31311 reachable only by the user (tailnet ACL / proxy).
3. Create the connection once via `POST /api/connection` (JSON above, `hosts` = the in-cluster or tailnet NATS URL, `inbox_prefix` = `_INBOX_viewer`).
4. Re-generate the auth config with `--viewer` whenever hosts change (the viewer's stream list is per host).

## Hold, edit, filter

Today `post` publishes straight into the target's inbox. A review step fits as a second hop:

```
post   → vega.outbox.<from-host>.<to-host>.<to-pane>   (stream VEGA_OUTBOX, WorkQueue)
filter → consumes outbox; pass / edit / drop / hold
       → republishes to vega.inbox.<to-host>.<to-pane>.<from-host>
```

- The filter is one more consumer process (or the relay itself). Rules: size limit, deny patterns, per-sender rate, "hold everything to pane X".
- Hold = do not ack; a `vega mux held` / `release <id>` / `drop <id>` command acts on the outbox consumer. Edit = release a changed copy and ack the original.
- Permissions shift: machines publish only to `vega.outbox.<self>.>`; only the filter's key may publish to `vega.inbox.>`. The filter must re-add `<from-host>` from the outbox subject, which keeps sender authentication.
- Cost: one more hop (single-digit ms) and one more process that must run. Not worth it until there is a concrete rule to enforce.

## Loop protection

Layers, weakest to strongest:

1. **Text rule** in `inbox` output: "reply only if it adds something; never to acknowledgements". Worked with pi.
2. **Fast hop counter.** Reply commands carry `--re <thread>/<hop>@<published-ms>`, using the
   answered message's broker publication time (also for archived mail). Reading or redelivery does
   not refresh it. At send time, replies less than 120 seconds after publication increment the hop;
   replies at least 120 seconds later reset to 1 on the same thread. Hop 1 is the initial message;
   attempting hop 9 in a fast chain fails with `hop_limit`. Twenty slow replies can keep working.
   Inbox and log still show the hop. At the fast limit, inbox warns to stop automatic replies and
   tell the user, but retains the timestamped command so a substantive reply after work can resume.
   Do not automatically sleep and retry acknowledgement loops.
   Legacy tokens without a timestamp keep the old counter behavior because their age is unknown.
   Future timestamps count as fast under clock skew; broker and sender clocks should be synchronized.
   Older binaries cannot parse timestamped tokens: upgrade sending hosts before copying new commands.
   Thread and timestamp are self-asserted: this stops accidental ping-pong, not a determined agent.
3. **Pointer coalescing.** The relay types at most one pointer per pane per 10 s, with the current count, and none if the inbox is already empty. A flood of messages costs the receiver one turn per window, not one per message.
4. **Server limits (not done).** `max_msgs_per_subject` on the inbox stream with `discard: new` caps unread mail per pane-and-sender: a looping sender gets publish errors. A per-sender rate limit belongs in the filter stage above.

## Auth: nkeys per machine

Tested on a second server (`127.0.0.1:4223`, scratch dir) with centrally generated keys (round 1; see round 4 for the final tooling):

```
publish:   vega.inbox.*.*.<self>
           $JS.API.STREAM.INFO.VEGA_INBOX_<self>
           $JS.API.CONSUMER.*.VEGA_INBOX_<self>.>
           $JS.API.CONSUMER.MSG.NEXT.VEGA_INBOX_<self>.*
           $JS.ACK.VEGA_INBOX_<self>.>
subscribe: vega.inbox.<self>.>, _INBOX_<self>.>
admin:     round 6: stream info + consumer list per host; stream create only for --create hosts
```

Round 5 changed two lines: `_INBOX.>` became `_INBOX_<self>.>` (it leaked every host's fetches) and `$JS.API.INFO` is gone (not needed; it showed account-wide stream and storage stats). The tests below are from round 1 with the old list.

| Test | Result |
| --- | --- |
| No key | `authorization violation` at connect |
| proteus key posts to proteus | ok |
| ganymede key publishes `vega.inbox.proteus.w3V:p17.proteus` (forged sender) | denied |
| ganymede key posts honestly (`....ganymede`) | ok, `inbox` shows "on ganymede" |
| ganymede key runs `inbox` or `relay` for proteus | `permission_denied` (STREAM.INFO) |
| proteus key runs `inbox` | ok |

Lessons:

- **Wildcards match whole tokens only.** `inbox-proteus-*` does not match consumer `inbox-proteus-w3V_p17`. With one shared stream there is no way to restrict a machine to its own consumers, and `STREAM.INFO` with subject counts leaks every machine's unread counts. One stream per host fixes both.
- **`CONSUMER.MSG.NEXT` is two tokens,** so `CONSUMER.*` does not cover it; it needs its own rule.
- **Only the host is authenticated.** Pane, agent and cwd are headers; a machine can claim any pane of itself. Within one user's machine that is fine.
- Seeds are generated on each machine (`vega post keygen`, round 4); only the public key goes to the server config.

## Production server: titan

Decided (orrery D49, given in round 3): the server lives on **titan** as **`nats.<tailnet>`**, tailnet only. Clients get the tag **`tag:nats-client`**. Rounds 1–2 assumed callisto (k3s); how titan runs the service (container, k3s, systemd) is not decided here, I don't know it.

Server:

1. nats-server with JetStream on a file store. R1 streams: a titan outage means no delivery, but no loss.
2. `max_file_store` ≥ number of machines × `ARCHIVE_MAX_BYTES` (1 GiB each), because each host stream reserves its `max_bytes`. orrery's 8GB = **8 hosts max** (measured, round 4). Disk to match, plus headroom.
3. Auth config from `vega post auth-config` (public keys only), as an included file. The admin creates the `VEGA_INBOX_<host>` streams with `vega post provision` (see round 4); machine keys cannot.
4. Port 4222 reachable only on the tailnet under `nats.<tailnet>`. No public listener. Do not expose the monitor port 8222 (or only to the admin machine).
5. TLS: WireGuard already encrypts tailnet traffic; nkeys authenticate. Add TLS (e.g. `tailscale cert`) only if the policy requires it.

Tailnet (policy changes, needs the user):

6. ACL grant: `tag:nats-client` → `nats.<tailnet>:4222`; nothing else.

Each machine:

7. `vega post keygen`: creates `~/.config/vega/nats.nk` (0600; the client default) and prints the public key for the server config.
8. Set `VEGA_NATS_URL` or the first line of `~/.config/vega/nats.url` to the broker URL. There is no default; `VEGA_NATS_NKEY` overrides the seed path.
9. Run `vega post relay` as a user service (launchd on macOS, systemd user unit on Linux) with access to the herdr socket. Reconnect and rescan work (round 2).
10. `vega deploy` could install the relay service and print the public key; `vega doctor` could check broker reach, key and relay.

Open questions:

- Offline machines: a laptop without tailnet cannot post at all. A local leaf node with JetStream could buffer outgoing mail; more moving parts, only if it hurts.
- Should `vega mux send` (paste) stay as fallback when the broker is down longer than `send`'s 10 s wait?
- Cross-machine spawn still needs herdr over ssh; only messaging moves to the broker.

## Not tested

- Real cross-machine traffic (scope: this machine only).
- Pointer delivery confirmation (`confirm_taken`); the relay types once and does not retry Enter. Not needed so far: every pointer was taken (one short line).
- A pointer to a pane whose agent is gone is logged as `pointer failed: agent_not_found`; it is tried again only after a rescan or new mail. The mail stays for 365 days.
- Broker outage longer than 10 s during `send`.
