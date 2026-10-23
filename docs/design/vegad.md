# vegad: one daemon per machine for delivery, wake-ups and reconciliation

Status: design, 2026-10-08. Epic: vega-lhc. Supersedes the separate designs in vega-lhc.4 (idle
notify) and vega-lhc.12 (registry); both become parts of this daemon.

## Problem

Field reports from a UAC coordinator (spellkave, 2026-10-08) and our own evals:

- Fan-in has no primitive. Nine status asks gave nine separate wake-ups, interleaved with user turns,
  each carrying a 4-line footer.
- Two channels: local replies are pasted as prompts (`vega mux send`), remote replies go through
  NATS and a pointer (`vega post`). Two formats, and only post has a correlation id.
- A paste lands mid-turn. A child that finishes without replying is not noticed.
- Who spawned or coordinates whom is invisible across machines; closed panes leave stale records.

## Goal

The same delivery for every agent harness (Claude, Codex, pi, agy), with the fewest tokens:
at most one short pointer per agent turn, and one command to read everything.

Non-goals: a new transport (NATS on titan stays), a herdr fork, harness-specific plugins as the
main path.

## Shape

`vegad` is the existing `vega post relay` service, grown up. One per machine. herdr starts it, it
upgrades itself in place, and no OS service manager is involved (see Lifecycle).

On Silverblue the binaries are built in a toolbox (newer glibc) but herdr's startup hook runs on the
host, where they fail with `GLIBC_x not found`. `relay install` detects a toolbox
(`/run/.toolboxenv`, container name from `/run/.containerenv`) and writes the hook as
`/usr/bin/toolbox run -c <name> <vega-post> relay start`, so vegad runs in the container that built
it. HOME and the herdr socket are shared. No build toolchain change.

Verified (vega-mns.3, 2026-10-10, Fedora Silverblue, toolbox `code`): vegad survives the exit of
`toolbox run`. `relay start` spawns it with `setsid` (own session), stdin from `/dev/null` and output
to the log; `podman exec` does not kill the exec session's leftover processes. Running
`/usr/bin/toolbox run -c code <vega-post> relay start` from the host returned exit 0 after
"relay: started", and the vegad was still alive and answering `relay status` afterwards (own session,
reparented away from the `toolbox run` tree). No code change was needed.

```
            titan: NATS JetStream (mail streams, KV registry)
                 ▲                         ▲
   vega post ... │                         │ reconcile
                 │                         │
  agent ──cli──► vegad (per machine) ◄── herdr events.subscribe (status, pane closed)
    ▲                │
    └── one typed line: "[vega-post] 3 new. Run: vega post inbox"   (only when the agent is free)
```

Jobs, all in one process:

1. **Deliver mail.** Every message, local or remote, is a post message with a correlation id.
2. **Deliver when free.** vegad watches each agent's status. It types the pointer only when the
   recipient is `idle` or `done`, and coalesces everything pending into one line. `--now` overrides
   for urgent messages ("stop"). Mail from the recipient's recorded parent (its task channel, known
   from `FROM_PANE` and the `parents/` records) is also typed while the recipient is `working`:
   Claude and Codex queue typed input as a follow-up, so a decision reaches a sentinel in a long
   turn (vega-lhc.44; a codex sentinel held five parent mails through a 5 h turn). Never while
   `blocked` (a dialog: a stray Enter can approve; herdr also rejects it) or `unknown`, and the
   draft guard stays in front. Such a pointer stands for the rest of that turn: no second line
   until the inbox is read or a new turn starts. Lost pointers (vega-lhc.73): a turn that ends on
   a provider abort (below) may drop input typed into it (pi does), so the abort clears the
   pointer and unread mail is pending again. After an exhausted quota vegad types no pointer for
   5 minutes (`STALE`), unless the agent works again first (resumed, or another model) or the mail
   is `--now`; then one pointer probes, and a further abort holds again. Safety net: mail whose
   last pointer was typed 5 minutes ago and is still unread gets one more pointer when the agent
   is `idle` or `done`, at most once per 5 minutes; the log marks it `again`. It also catches
   aborts vegad does not detect (a child that reported earlier, a pane that is no child).

   **Draft guard (vega-lhc.194).** Before each pointer (also urgent and parent mail) vegad reads the
   pane's screen (`delivery::draft`): a pointer never joins text the user is typing. It knows three
   input boxes: Claude Code (`❯`, under a rule), Codex (`›`, below the last rule) and pi (the box
   between the last two rules, no glyph). A draft holds the pointer; one unchanged for 5 minutes is
   stashed, cleared, the pointer typed and the draft typed back (a draft it cannot type back, such as
   a paste placeholder, a Codex draft or a multi-line pi draft, keeps holding and notifies the user
   once). A screen with no known box (another harness, a full-screen view) is never read as empty:
   the pointer waits until the screen has not changed for 20 s (the user is not typing); one that
   keeps changing for 5 minutes notifies the user once and keeps holding. Residual risk: a draft
   left untouched for 20 s in an unknown harness is joined. Not guarded, by design: `vega mux send
   --now` and its fallbacks (no relay, local mode, broker away) paste directly with `herdr agent
   prompt`.
3. **Gather.** `vega post ask <targets...> --fields task,branch,bead,blockers` sends one question
   with a correlation id. vegad wakes the asker once: when all answered, or at the deadline.
   `vega post inbox --re <id>` returns one table.
4. **Notice silence.** A child whose status turns `idle`/`done` without having sent anything to its
   parent since its last prompt: one notice to the parent per idle episode.
5. **Reconcile.** Local records (tasks, parents, bead at spawn) are the truth for this machine's
   panes; vegad mirrors them to a NATS KV bucket and removes records of closed panes. `vega uac`
   reads the KV view, or the local view with an explicit "broker unreachable" line.
6. **Show the human.** Mail counts as herdr sidebar tokens; zero agent tokens.

## Message model

- `vega mux send` becomes post plus vegad delivery. The direct paste stays as `--now` and as the
  fallback when vegad is not running.
- The footer shrinks to one line in the inbox listing, not per message: the reply command appears
  once per batch.
- Replies can carry fields (`--field task=... --field blockers=...`) so the asker gets a table, not
  prose to re-parse.
- `adopt`: when a coordinator asks a session it did not spawn and takes it over, it records itself as
  parent. A second coordinator then sees the conflict in `vega uac`.

## herdr integration

What herdr 0.9.3 offers and what we use:

| herdr feature | Use |
|---|---|
| `events.subscribe` (socket, NDJSON stream) | Yes: `pane.agent_status_changed`, `pane.closed`, `pane.exited`, `pane.created`. One stream per vegad. On `events_lost`, resubscribe and resync with `session.snapshot`. |
| `pane.report_metadata` (`tokens`, `ttl_ms`, `source`) | Yes: a mail-count token per agent pane for the sidebar. Needs a sidebar row template that shows the token; verify. |
| `notification.show` | Yes, through `vega_herdr::notes::notify`: the toast plus a note for the board list (Notifications). Draft and lost-draft notices today. |
| `agent.prompt` / `pane.send_text` | Yes, the one way to reach the agent: the pointer line. |
| plugin `[[startup]]` | Yes: `relay start` starts vegad detached if it does not run, then exits. The hook stays one-shot; vegad is not its child (see Lifecycle). |
| plugin `[[events]] on = ...` | No: it spawns a process per event; the stream in vegad is cheaper and keeps state. |
| plugin `[[actions]]` / `[[panes]]` | Later: an inbox popup for the human. |

Decision to revisit: we stayed on the herdr CLI because spellkave's own socket client broke on
protocol drift. `events.subscribe` and `pane.report_metadata` have no CLI. vegad therefore needs a
small socket client. Keep it minimal (these two methods plus `session.snapshot`), check
`schema_version` from `herdr api schema` at start, and fall back to polling `herdr agent list` when
the schema is unknown.

## Lifecycle (vega-lhc.54)

vegad needs starting, not supervising: a restart loses little, and herdr is where vegad is needed.
So herdr and the board start it, it upgrades itself in place, and launchd / systemd are gone.

**What herdr does with a plugin's children** (herdr 0.9.3 source and a live probe, 2026-10-10):
startup hooks, actions and event hooks are plain child processes of the server (no process group,
no setsid). herdr kills none of their descendants: not when the hook exits, not on server stop,
not on live handoff, not on plugin update. The "cleanup" in herdr's plugin docs reclaims the files
of old GitHub-managed installations; vegad runs `~/.local/bin/vega-post`, outside any plugin root,
and vega.vegad is a linked plugin. One trap: herdr reads a command's stdout/stderr to EOF, so a
descendant that keeps the pipes keeps the hook "running" and holds one of 32 command slots. A
plugin pane is no host either: a PTY, closed by the user, gone after a server restart.

**Who starts it**

- herdr: plugin `vega.vegad` (written and linked by `relay install`) runs `vega-post relay start`
  as its `[[startup]]` hook, at every server start and again on every live handoff.
- The board: it shows vegad's state (running, pid, version, binary current or stale) and `V`
  starts it.
- By hand: `vega post relay start`.
- Not as a side effect of other commands: a `vega post send` that starts a daemon hides where it
  runs and with which env. `vega mux send` already falls back to a direct paste.

`relay start` is idempotent: if the lock is held it prints the pid. Otherwise it spawns
`vega-post relay` in a new session (setsid), stdin from /dev/null, stdout and stderr appended to
`$XDG_STATE_HOME/vega/post-relay-<host>.log` (moved to `.log.1` above 10 MB at start), and waits
up to 3 s for the lock. Env: the starter's, minus pane variables (`HERDR_PANE_ID`, ...) and the
agent shims on PATH, plus `vegad.env` (`$XDG_STATE_HOME/vega/vegad.env`: `VEGA_NATS_URL`,
`VEGA_NATS_NKEY`, `VEGA_POST_HOST`, `HERDR_SOCKET_PATH` as `relay install` found them, the values
the unit used to pin). The pinned socket keeps a named herdr session from binding vegad to itself.

**One instance**: the existing lock (`post-relay-<host>.lock`). Two racing starts each spawn; the
loser fails on the lock and exits.

**When herdr exits, vegad keeps running.** Following herdr would restart vegad on every live
handoff (the old server exits each herdr update) and drop its delivery state each time. Mail,
commands and the registry do not need herdr; agent status falls back to `herdr agent list` polling
(one log line per outage, not per poll) and retries the stream, so vegad picks up the next server
by itself. After a reboot nothing runs until herdr starts; mail waits in JetStream and the start
rescans it.

**Upgrade in place.** vegad records the identity of its executable (device, inode, size, mtime of
`current_exe`) at start and checks it every tick (1 s). When the file changed and is the same on
two ticks in a row (the install finished), and no draft is stashed and no command is in flight,
vegad:

1. writes `post-relay-<host>.handoff.json` (its pid, the time, the delivery state: pending,
   urgent and parent mail, pointer floors, arrival times; the supervision state: last status,
   busy, silent suspects);
2. clears close-on-exec on the lock's fd and names it in `VEGA_RELAY_LOCK_FD`;
3. execs the same path with the same arguments.

The flock belongs to the open file description, which exec keeps: the new image adopts the fd and
never lets go of the lock, so no second vegad can slip in. The pid stays, so the lock file and a
still-installed unit see the same process. The new image reads the handoff only if the pid is its
own and it is younger than 60 s, then deletes it. Connections (NATS, herdr) are made fresh, with a
mail rescan as at any start. Agent statuses come back with the first snapshot; the wake memory is
already on disk. If exec fails (a broken binary), vegad logs it and keeps running the old image
until the file changes again; a binary that execs but then crashes leaves no vegad, which the board
shows as stopped (`V` starts it). `cargo install` replaces by rename (new inode); an in-place copy
changes size or mtime. On macOS the first launch of a new file takes a few seconds (seen: 5 s).

**Migration.** `relay install` (the user runs it once per host): stops and removes the launchd
plist / systemd unit if present, writes `vegad.env`, writes and links plugin vega.vegad, starts
vegad. `relay uninstall`: stops vegad, unlinks the plugin, removes a leftover unit. Until a host
is migrated, `relay start` with no relay running kickstarts its old unit as before. A relay from
before self-exec does not upgrade itself: `relay status` shows it `stale` until `relay install`.

**Install fallback (vega-lhc.122).** The exec waits for a quiet point (no stashed draft, no command
handler running); on proteus one never came and vegad ran a replaced binary for a day. So
`scripts/install-cli.sh` ends with `scripts/vegad-refresh.sh`: while `relay status` says `stale`
it waits up to 15 s for the self-exec, then runs `relay stop` and `relay start` (the in-memory
state the handoff carries is lost; mail is rescanned at start). A vegad that is not running stays
stopped, except under `vega deploy apply` (below). `mise run install-cli`, `vega upgrade` and
`vega deploy apply` all run it.

**Deploy.** `vega deploy apply <machine>|--all [--from-branch <branch>]` deploys a pushed branch,
defaulting to `main`: fetch, check out the local branch (create it from the fetched commit if missing),
then fast-forward only; a fresh host clones that branch. Divergence fails before install. The target
stays on the selected branch; omit `--from-branch` on the next deploy to return to `main`.
A host whose checkout sits on a feature branch makes gitsync skip main's fast-forward if no other
checkout has main checked out ([git-sync.md](git-sync.md)). Deploy status always compares with pushed
`main`. After install, the deploying host's local config overrides replace the target's and
`vega config apply` runs; config failures never fail the deploy.

`vega deploy apply` restarts no running vegad itself: it re-execs into the new binary,
or install-cli's fallback restarts it. A stopped vegad is started once (`relay start`, with
`~/.local/bin` on PATH; vega-lhc.142): a dead vegad blanks herdr tokens and stops mail. It reports
`relay=current|stale|stopped`, waiting up to 10 s for `current`.

## Fallback

The tailnet is almost always up, but the design must not depend on it:

- Broker unreachable: vegad keeps a local queue and delivers local mail itself; remote mail waits.
  Every view says "broker unreachable" instead of showing a partial picture as whole.
- vegad not running: `vega mux send` pastes directly (today's behavior) and says so.
- No broker at start: local mode (below), once this host's identity migration
  completed. Before that, vegad stays alive in identity quiesce, with project
  workers/publication off and the cutover error in its log/status. It retries
  every five seconds and retains self-exec; a missing approved plan or rollback
  intent is also quiesced rather than a service crash loop.

## Local mode (vega-lhc.118)

For a host with a completed identity migration, an occasional user outside the estate has no broker, and a vegad that exits at once would also drop the jobs
that need only herdr, among them the upgrade proposal (upgrade.md). So when the connect at start fails with
`broker_unreachable` (none configured, or the configured one down), vegad
stays up in local mode (`local()` in plugins/post/src/main.rs):

An unmigrated or failed-cutover host instead remains alive in identity quiesce
before this local loop or any project worker starts. It reports each changed
error once and retries without workers; self-exec is available. A completed
online rejoin that detects mixed versions or reappeared legacy keys uses the
same quiesce. Once repaired, it automatically continues startup.

- On: herdr status events, sidebar metadata, attention and parent wakes for local panes, parks, the upgrade
  check, git sync, theme follow, the self-exec upgrade (the handoff also carries undelivered tool mail).
- Off: the mail relay (pointers, `vega post`), the registry, commands, quarantine, beads sync and the CI poller
  (both deliver by mail). `relay status` says so once: `broker: local (no broker configured: …)` or `local
  (broker <url> unreachable)`, then `off: <jobs>`. The state is `post-relay-<host>.broker` (`connected` or
  `local (…)`); `vega_herdr::state::relay_local` reads it, so `vega mux send` pastes at once instead of
  trying the broker first, and `vega bd` / `vega ci watch` say their job is off.
- Local delivery: tool mail for a pane on this host (today only the upgrade proposal) is typed as one line,
  `[<tool>] <text>` with the lines joined by ` | ` (a multi-line paste can stay staged in Claude's input),
  when the agent is `idle`/`done`, behind the draft guard. A closed pane drops its mail.
- A probe connects every 60 s off the loop; once a broker answers, vegad switches to the broker loop and hands
  it the delivery and supervision state plus undelivered tool mail (published as mail then). It never goes
  back: a broker that drops later is the reconnect case above.
- `--once` still needs a broker.

`vega onboard` therefore recommends `vega post relay install` on every host.

## Security

- Each host writes only its own KV keys and its own sender subjects (as today for mail).
- deimos is inbox-only (user decision). It replies into quarantine (Slayer decision 2026-10-12,
  vega-lhc.83), see below.

### Quarantine (inbox-only replies)

An inbox-only host may reply, but its mail never reaches an agent:

- Broker: `auth-config --inbox-only deimos --quarantine deimos` lets deimos publish only
  `vega.quarantine.*.*.deimos` (still no `vega.inbox.*`); every other host may subscribe
  `vega.quarantine.<self>.>` and answers through its existing `allow_responses`. No stream, no
  provisioning.
- Send: on a host in the inbox-only list (`VEGA_POST_INBOX_ONLY`, else
  `$XDG_CONFIG_HOME/vega/inbox-only`, else `deimos`), `vega post send` and `done` make a core NATS
  request to `vega.quarantine.<to>.<pane>.<self>` (at most 4 KiB). The target's vegad writes a note
  (kind `quarantine`, title `deimos (quarantine) to <pane>: ...`, toast plus `vega post notes` and the
  board) and answers `stored`. It never types into a pane and never enters delivery or an ask's
  fan-in. No vegad there: `no_responder`, nothing stored. `ask` from such a host is refused.
- Errors: a denied `vega.inbox` publish says "inbox-only by design" without the broker-config hint;
  a denied quarantine publish (grant not rolled out) says so and tells the agent to tell the user.
- Fan-in: `vega post ask` sends the question to agents on inbox-only hosts but does not wait for them.
  Their reply lines (`vega post inbox`, `vega mux` footers) say the answer goes into quarantine.
- Trade-off: no archive and no retry for quarantined replies (the sender sees whether it was stored);
  a stream would need admin provisioning and per-host consumer grants.
- Pointer text is fixed by vegad; message bodies never get typed into a pane.

## Token budget

- Pointer: about 15 tokens, at most once per agent turn.
- `vega post inbox`: TOON table, bodies truncated with `--full` on demand, reply command once per batch.
- A gather of N answers: one wake-up and one call, instead of N pasted prompts.

## Supervision (from orca orchestration)

Lessons from orca (repos/orca/skill-guides/orchestration*, src/shared/orchestration-fleet-attention.ts):

- **Completion is explicit.** A sentinel reports exactly once with an outcome:
  `vega post done --outcome succeeded|failed "<summary>"` to its parent (the spawn footer says so).
  Failure is never only prose.
- **Absence proves nothing.** No status, an old status or an unchanged screen is `unverifiable`:
  keep waiting or inspect, never stop, retry or clean up. Act only on proof: the pane closed or the
  agent exited, or the sentinel went idle/done after its prompt without sending `done`.
- **Attention is computed centrally**, per sentinel, with a literal next command:

| Attention | Source |
|---|---|
| `input` | herdr status `blocked` (approval or other UI) |
| `asking` | a question dialog, or idle after its prompt without `done` or reply, and the last output paragraph of the turn ends with `?` (the question shows in `vega uac`) |
| `quota` | idle after its prompt without `done`, and the turn ended on a provider abort (below) |
| `failure` | `done --outcome failed` |
| `waiting` | would be `silent`, but a `vega ci watch` of its pane still owes it a verdict mail (docs/design/ci-watch.md); no wake |
| `unanswered` | a known idle child is an unresolved post ask target; requires a resolved host and positive current-name mode record; wakes once per actionable transition |
| `silent` | idle/done since its last prompt, no `done` sent (its last 1-2 lines show in `vega uac`) |
| `exited` | `pane.closed`, `pane.exited`, or the agent left the pane |
| `completed` | `done --outcome succeeded`, waiting for the parent to land or release it |
| `parked` | the parent parked it on an open decision bead (`vega uac park`); no wake |
| `decided` | its park's bead closed: the parent resumes or settles it |
| `all_done` | every sentinel of a parent is completed |

- **Headed panes** (vega-lhc.68). A headed child (`vega mux mode`: the owner works in
  it) addresses `asking`, `unanswered`, `silent` and `input` to the owner, not the parent: the owner sees the
  question, the idle turn end or the dialog in the pane, and the parent must not answer an
  approval or trust dialog anyway. vegad keeps them out of the wake set (`state::for_user`), so
  they never wake the parent; `vega uac` still lists them with next `for the user (headed pane,
  no wake)`. `exited`, `failure`, `quota`, `completed` and `decided` still wake. Headless
  children are unchanged.
- **Provider aborts** (vega-lhc.58, `plugins/post/src/quota.rs`). A turn that ends on a rate limit,
  overload or used-up quota looks like `silent`, but the next step differs. When vegad confirms a
  silent child on its machine, it reads the agent's session log (vega-transcript): an error as the
  log's last entry is the abort; without a log or an error there (Codex does not log its usage
  limit), the last lines above the input box. Transient (overloaded, `529`, rate limited, at
  capacity, `429 Too Many Requests`): retry soon. Exhausted (usage/plan limit, `quota exceeded`,
  out of credits, a reset duration next to a 424/429): wait for the reset. The reset comes from
  the text (`resets in 2h33m`, `try again at <clock>`, `resets 3pm (Europe/Berlin)`, Claude's
  `|<unix>`) as a local clock, else from token-burn's binding window (`… (token-burn)`; pi's
  provider is guessed from the error). Ordinary failures (5xx, gateway, context too long, login)
  stay `silent`. The `silent/` record keeps it (`quota: {transient, error, resets}`), the registry
  link carries it to the parent's host, and `vega uac` shows the error with the reset and the
  next step: retry (`vega mux send <pane> "continue"`), or move the work to another kind/provider.
  A child on another machine is confirmed without a screen or log: no quota.
- **Stale reports.** The parent's machine records when it last briefed each sentinel (`briefs` state:
  spawn, adopt, or `vega post send` to it). A `done` sent before that brief reports on earlier work: vegad
  does not record it as the outcome, and `vega post inbox` marks it `stale: sent before your newer brief of
  <time>`. A pasted `vega mux send --now` is not recorded as a brief.
- **Settlement.** After a `done`, the parent decides exactly one: give the sentinel follow-up work,
  keep it, or close it. `vega uac` lists sentinels that still wait for this decision.
- **No heartbeats.** herdr's status events give liveness without worker tokens.
- **Parking** (vega-lhc.32). A completed or asking sentinel that waits on a Slayer decision
  otherwise stays in its parent's attention set, and every change elsewhere in that set wakes the
  parent again for it. `vega uac park <pane> --bead <id> [--machine <m>]` writes a `parked/` record
  on the parent's machine (bead, title, the checkout and the `bd` binary that found it: vegad's
  PATH has no bd). A park hides every attention but `exited` and `input`, and the relay keeps
  `parked` out of the wake set, so the earlier attention leaves the `woke/` memory as any vanished
  attention does. On the metadata clock (15 s) vegad runs one `bd show` per open park, none
  without parks; a closed bead marks the record closed, the attention turns `decided`, which is
  new, so the parent gets one line (`a decision closed; resume your parked sentinel`). A bead bd
  cannot show proves nothing: the park stays, logged once. New work from the parent (the settle
  that clears `done`, `silent`, `replied`) removes the park, so a resumed sentinel is supervised
  as before; so do `vega uac unpark` and `vega mux close`. Polling over hooks: `vega uac answer`
  does not close the bead and `bd close` has no hook, so only vegad sees every close. Auto-park
  (an open decision bead naming the pane) is not built: a bead does not name panes reliably.

vegad wakes the parent, never mid-turn: when the parent is idle/done and at least one of its
sentinels has new attention, it types one line, e.g. `[vega] 2 sentinels need you. Run: vega uac`, or with a question
`[vega] 1 sentinel asks: Should I also cover Hamburg? Run: vega uac`.
`vega uac` shows an `attention` column and the next command per row. One notice per new attention
state; nothing repeats while the state is unchanged.

## Order of work

1. M1 (done, vega-lhc.17): relay plus herdr event stream; deliver-when-free and coalescing for post.
2. M2 (done, vega-lhc.20/.21/.23): `vega post done --outcome`, attention in vegad, waking the parent,
   the attention column and next commands in `vega uac`, quiet UAC, `vega uac status`, `ask`/gather.
3. M3 (done, vega-lhc.25, vega-lhc.18): `vega mux send` through post (with `--now` and the direct
   fallback), one-line footer; the command channel `vega post cmd` (`open` handler: vega-6qj).
4. M4 (done, vega-lhc.12): registry in NATS KV with reconcile (crates/vega-registry), supervision,
   `uac claim` and `adopt --machine` across machines; sidebar metadata; the herdr startup hook.

Live on titan after the Slayer applies the regenerated auth config (commands + registry rights,
`--cmd-grant deimos=vega.cmd.proteus.open.deimos`) and runs `vega post provision registry`; until
then every vegad logs "commands: not served" and "registry: off" and keeps mail and local
supervision running.

## Registry (M4)

Bucket VEGA_REGISTRY, keys start with the writing host's token: `<host>.beat`, `<host>.agent.<pane>`
(status, name, kind, cwd, repo, finished turns, and `waiting` {status, since} while the agent waits for
the Slayer: [mux-next.md](mux-next.md)), `<host>.link.<child host>.<pane>` (lineage and
done/silent/replied of children that host spawned or adopted), `<host>.uac.v2.<hex(id)>`,
`<host>.note.<id>` and `<host>.notes.seen.<source>` (Notifications), `<host>.theme` (theme.md),
`<host>.janitor` (janitor.md: the state read skips it, vegad pulls the other hosts' every 5 min into
files). Agent entries add `schema: 2` and the full remote-derived `project` ID
when local resolution succeeds; repo/cwd remain location metadata. UAC payloads
carry `schema: 2`, the full ID, pane/host/claim time and published aliases.
Readers validate the key against that ID. Old UAC keys and non-v2 payloads do
not participate. vegad reconciles every 15 s. Ordinary reconcile preserves
legacy UAC/pin keys until the backed-up owner cutover explicitly deletes them;
see [project-registry-cutover.md](project-registry-cutover.md).
Readers use the stream's subject list and direct gets, no consumers; the state read skips the note
keys (only their names count, for the badge and for deleting pruned notes). A host
without a fresh beat (90 s) proves nothing; a remote child counts as gone only once its host
reconciled 40 s after our record. Inbox-only hosts get no registry rights (decision vega-nic.1), except
single keys granted read-only with `auth-config --registry-read <host>=<key>` (vega-lhc.90): one
last-by-subject direct get each, no stream info, put or watch. Such a host's vegad runs no reconcile;
it reads only `<control>.theme` (theme.md).

## Reachability (vega-lhc.129)

Field report 2026-10-18: a session on ganymede mailed a proteus UAC pane closed the night before. send
succeeded, vegad dropped the pointer, the sender heard nothing and took `herdr machine list` (only the
machines ganymede saved) for "proteus is offline". Who is reachable is vega post and the registry.

- **send checks the target** (plugins/post/src/reach.rs): two direct gets on send's own connection,
  at once (`<host>.beat`, `<host>.agent.<pane>`). A fresh host without the pane: `pane_gone`, nothing
  sent, and the hint names the project's live UAC (the gone pane's UAC registration, else the
  sender's checkout; one more registry read, on this error path only). A stale or unknown host, no
  answer within 2 s, or no registry right: a `warning:` line, and the mail goes. A local target the
  registry does not show open is checked in herdr, the truth for its own panes (the registry trails a
  new pane by up to one reconcile, and is off in local mode); a remote child whose record is newer
  than its host's beat (+40 s) is not proven gone. Tool mail
  (`--from-tool`) is not checked; inbox-only hosts quarantine as before.
- **Bounce**: delivery keeps each unread mail's sender (host, pane, agent) next to its arrival time.
  When a pane closes, vegad asks the broker how many of its mails are unread and confirms herdr no
  longer lists it (a restarting herdr may report every pane gone), then mails each sender of the
  unread ones once, as `tool vega-post` without a reply path, with the same hint. Mail from the user
  without a pane (the board) becomes a note on this host; tool mail is not bounced. In local mode
  (no broker) bounces are logged and dropped.
- **By role**: `vega post send --uac <project> "<text>"` reads beats, agents and UAC registrations
  of every host (one read) and mails the one live UAC: a registration on a fresh host whose pane its
  agent entry still lists. `<project>` is the full project ID (for example
  `github.com/durandom/orrery`), its repository name (`orrery`), a published
  `folder_name`, `origin_name` or merged alias, or `.` (this checkout's resolved
  ID). A clone at `~/src/orrery/main/.git` can publish folder `main` and origin
  `orrery`; these are aliases, never identities. Readers compare published
  aliases without opening remote hosts' paths. Deduplicated aliases do not add
  candidates. Legacy path keys and old registry payloads are not accepted.
  None: `no_uac`; several: `ambiguous_uac` lists full IDs and host/pane, including
  collisions between aliases. Use the full ID or explicit host/pane to
  disambiguate. Outside a Git repo, `--uac .` reports `no_uac`.

Local filesystem consumers use `state::uac_records`/`uac_of`, which project the
owner's `common_dir` metadata from the same v2 claim files. Registry and UAC
routing use `project_uac_records`/`project_uac_of` for IDs. Neither view reads
legacy claim files; hosts in claims and comparisons use the registry host
identity (`post_host`), while bd actors retain their local host contract.

## Sidebar

vegad reports herdr pane metadata every 15 s (tokens expire after 60 s without vegad): a sentinel
shows its spawn name, its task or done report as title, `$attention` and `$parent`; the UAC pane
shows `<atom> UAC` and `$role`, plus its badges (see Badges); a pane
with unread mail gets `$mail`, and `mail_since` (Unix ms of the
oldest unread arrival, as vegad saw it; since its restart for older mail), which `vega uac` shows as
the `unread_mail` age. herdr renders `$name` tokens
only where the sidebar rows name them. Snippet for `~/.config/herdr/config.toml` (rendered from `herdr/config.toml` by `vega config`, [config.md](config.md)):

```toml
[ui.sidebar.agents]
rows = [
  ["state_icon", "agent", "state_text", "$mail", { token = "$attention", bold = true, rules = [
    { equals = "failure", fg = "#f38ba8" },
    { equals = "exited", fg = "#f38ba8" },
    { equals = "input", fg = "#f9e2af" },
    { equals = "asking", fg = "#f9e2af" },
    { equals = "quota", fg = "#fab387" },
    { equals = "silent", fg = "#fab387" },
    { equals = "completed", fg = "#a6e3a1" },
    { equals = "decided", fg = "#a6e3a1" },
    { equals = "parked", fg = "#9399b2" },
    { equals = "waiting", fg = "#9399b2" },
  ] }],
  ["terminal_title_stripped"],
  ["workspace", "tab", "$parent"],
]
```

## Notifications

herdr keeps one toast and no history, so vega keeps its own (vega-pqr). The model is macOS: the
herdr toast is the banner, `vega board`'s Inbox (`N`) the list, the UAC pane's name the badge.

- Writer: `vega_herdr::notes::notify` shows the toast and writes a note to this host's store
  (`$XDG_STATE_HOME/vega/notes/<id>.json`, id = Unix ms; 7 days, at most 200). vegad uses it for
  draft and lost-draft notices; other events can follow.
- Store: local state is the truth, as for agents. vegad reconciles the notes into the registry as
  `<host>.note.<id>` and deletes pruned ones; no auth change, each host writes only its keys. An
  inbox-only host (deimos) has no registry rights: its notes stay in the local store, which
  `vega post notes` reads the same way.
- Read state: per viewing host, a watermark per source host (last id read, last id cleared) in
  `notes/seen.json`, published as `<viewer>.notes.seen.<source>`. `vega post notes read|clear`
  moves it; the board runs `read` for what it showed when the list opens, `clear` on `c`.
- Badge: vegad counts this host's unread notes plus the other hosts' unread ones (the registry read
  fetches only the bodies of notes above the viewer's watermark) and shows them as the email badge on
  the UAC pane (see Badges). vegad redraws at once when the notes directory changes, so opening the
  list clears the badge within a second.
- Project: a note may carry `project` (the project key); the badge of that project's UAC counts it.
  Notes without one (quarantine, drafts, notes from before the field) and notes of a project without a
  UAC on this host count only on the vega UAC: the project of the checkout in `install.json`. Without
  that record or a UAC for it, every UAC counts them.
- Self-resolving: a note may carry `cause`; the source host's vegad sets `resolved` (Unix s) when the
  cause clears, and a resolved note is read for every viewer (still listed, `unread: resolved`). Causes:
  `bd-sync:<DB>`, cleared when the project's beads sync completes after the note.
- Not macOS Notification Center: with a shared store every machine would show each note again.

## Badges

The UAC pane's sidebar name (`--display-agent`) is `<atom> UAC` plus a badge per non-zero count, in
this order, one space after every symbol, e.g. `\u{F0768} UAC \u{F01EE} 1 \u{F05D1} 2 \u{F256} 3`. The
symbols come from `[symbols]` in `~/.config/vega/theme.toml` ([theme-symbols.md](theme-symbols.md):
set `nerd` by default or `emoji`, `☢️ UAC 🔔 1 🗳️ 2 ✋ 3`, plus per-topic overrides).
`vega_herdr::theme` reads them, `vega_herdr::badges` formats the label; the board uses both. vegad
re-reads the file on every label refresh and logs its mistakes once per change. With the label it
reports the sidebar tokens `$uac`, `$notes`, `$decisions` and `$waiting` (a zero badge cleared), which
a herdr `[ui.sidebar.agents]` row colours from `[symbols.colors]` ([theme-symbols.md](theme-symbols.md),
Colours and herdr sidebar tokens).

| Badge | nerd | emoji | Count |
|---|---|---|---|
| UAC marker | md atom U+F0768 | ☢️ U+2622 U+FE0F | - |
| notes | md email U+F01EE | 🔔 U+1F514 | unread, unresolved notes of every host for the project; host-level ones on the vega UAC (Notifications) |
| decisions | md scale_balance U+F05D1 | 🗳️ U+1F5F3 U+FE0F | open `slayer-decision` children of the project's one open `uac-tracker` epic, as `vega board`'s decisions view counts them |
| waiting | fa hand_paper_o U+F256 | ✋ U+270B | this host's headed sentinels of the project whose attention is asking, silent or input: the rows `vega uac` marks `for the user` |

- Decisions: one `bd list --label-any uac-tracker,slayer-decision --json` per project, with the bd and
  checkout `vega bd init` provisioned (else the project dir and a provisioned project's bd). Cached;
  rerun when the project's Dolt journal changes, at most every 60 s otherwise. A failed run keeps the
  last count.
- Waiting: local panes only (vegad knows remote panes' mode and attention only through their host).
- vegad saves each project's counts to `$XDG_STATE_HOME/vega/badges/v2/<hex(ProjectId)>.json` every 15 s;
  `vega board` reads them for its top bar (stale after 60 s: no badges while vegad is down).
- Without a UAC pane on the host, the open board panes (terminal title `vega-board-*`) get
  `vega board` plus the badges of their project.

## CI watch

vegad also polls GitHub CI for the PRs and branches agents register with `vega ci watch`, with conditional
requests, and mails each watcher on a new verdict: one poller per host instead of a `gh run watch` loop per
sentinel. See [ci-watch.md](ci-watch.md).

## PagerDuty

A separate thread on the configured polling host (default proteus) runs pdc every 15 min.
It reads committed service mappings from locally known projects, resolves their UACs through
the registry on any host, and sends incident/status mail plus four-hour reminders. Unmapped
services, duplicate ownership or no live UAC produce durable board inbox notes. Failures log
once without stopping vegad; state survives restarts and delivery is at-least-once. See
[pagerduty.md](pagerduty.md) for host selection, config and agent rights.

## Beads sync

vegad also keeps the beads of every project `vega bd init` provisioned on this host in sync with the
configured Dolt remote: a pull every 60 s, a push within seconds of a local write, a stop plus an
attention for the UAC on a conflict. See [beads-sync.md](beads-sync.md).

## Git sync

vegad also fetches the git repos of the same projects (and of every UAC claim) every 5 min,
fast-forwards a clean main checkout at most every 15 min, and records how far each worktree is
behind; the bd shim tells the agents. See [git-sync.md](git-sync.md).

## lanista catalog

vegad on the builder host (ganymede) runs `lanista fetch` daily and puts the index into the broker's object
store; every other vegad copies a newer one by direct gets, no consumer. See
[lanista-catalog.md](lanista-catalog.md).

## Theme

vegad also follows the OS dark/light mode: a thread applies each change to every TUI's config and publishes
`<host>.theme`; headless hosts follow their control host's entry; idle Claude panes get `/config theme=<mode>`
typed once. See [theme.md](theme.md).

## Upstream check

vegad also checks, at most once per 24 h and off the relay loop, whether the vega checkout this host was
installed from is behind its upstream, and mails one vega session once per new upstream state. See
[upgrade.md](upgrade.md).

## Hygiene

Badges pile up when nobody settles them (vega-lhc.126, `plugins/post/src/hygiene.rs`). On the metadata
clock vegad mails each project's live UAC pane on this host, as `tool vega-hygiene`:

- Waiting pane: a headed pane in the Waiting badge for 24 h with the same attention, once per such state.
  The UAC records the question as a `slayer-decision` bead and closes the pane.
- Hygiene list: the project's stale items, at most once per 24 h and only when the list changed since the
  last one: open decisions at least 24 h old (`created_at`), panes waiting over 24 h, panes with mail unread
  over 24 h (the UAC's own excepted) and unread notes older than 24 h. The UAC asks the Slayer once,
  batched (close / keep?).

The memory (when each pane started waiting, whether its UAC heard, each UAC's last list) lives in
`$XDG_STATE_HOME/vega/hygiene.json`, so a restart neither repeats a mail nor resets a clock. vegad only
reports: it never closes a bead or a pane. Unread mail ages count from vegad's start (Sidebar, `mail_since`).

## Open questions

- Urgent delivery: is `--now` enough, or should `blocked` peers get priority?
- A herdr upgrade that changes the socket schema while vegad runs: vegad checks the schema once at
  start and polls if it is unknown; it does not re-check after a live handoff.


Project records use normalized fetch-remote IDs; filesystem execution retains
owner-local locations. The startup journal applies plans before workers start,
preserving per-clone stopped state, timestamps and notification causes.
Interrupted startup replays the saved plan; rollback restores original bytes
while writers stay stopped. Historical bd backups remain untouched. The UAC
coordinates preview, audit, deploy, verify and rollback: see
[the operator runbook](project-registry-cutover.md#operator-runbook-146).
