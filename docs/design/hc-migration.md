# Migration from the herdr TUI to hc and herdr-app

Status: draft for the Slayer (vega-0h4c.2, epic vega-0h4c). The call-site inventory behind it is
[hc-migration-inventory](hc-migration-inventory.md).

## Goal

vega stops driving the herdr TUI and runs on herdr-core (`hc`: terminals, agents, labels, send/read/wait,
remote machines) and herdr-app (the GUI on hc, `~/.config/herdr-app/`). The last TUI state is tagged in git
(vega-0h4c); vega config keeps owning all config (herdr-app units: vega-0h4c.1, [config](config.md)).

## Decisions

1. **Design B: vega gives up what hc does natively.** Starting agents, readiness and the first message,
   unique names, parent/children, read/send/keys/kill, cross-machine addressing and the status event stream
   are hc's. vega keeps what sits above the terminal: the UAC role and lineage ledger, `vega post` mail,
   board, gate, dialog classification, capacity and model choice.
2. **Parallel operation during a short transition.** Both worlds run on one machine until every host has
   moved. Every slice is additive: the TUI path stays untouched until the last slice.
3. **The backend follows the agent concerned.** For "who am I" the caller's world decides; for a target the
   world recorded with it decides.
4. **Sentinels outlive their parent (Slayer, 2026-11-06).** `vega mux spawn` runs `hc run` inside the
   spawning agent's terminal, so the UAC becomes the hc parent, and by default a child ends when its
   parent's program ends: a UAC handoff, `/exit`, crash or resume would end every sentinel (TUI panes
   never did). vega spawns with `--outlive-parent`: hc keeps the parent link (`hc wait --children`, notes)
   and vega keeps its own lineage ledger.
5. **The vega herdr-app mod starts vegad (Slayer, 2026-11-06).** herdr-app has no startup hooks; a mod has
   `init()` and `exec()`, so the vega mod runs the idempotent `vega-post relay start` when it loads, as the
   `vega.vegad` herdr plugin's `[[startup]]` hook does today. Amendment vega-0h4c.25:
   `relay install` also installs a login starter on macOS and Linux for hosts without
   either UI. It calls the same detached, idempotent starter; vegad retains its lock
   and self-exec lifecycle. No hc core job is installed or changed. See
   [vegad](vegad.md) "Lifecycle".
   Core ownership decision (Slayer, 2026-11-07): `hc machine`/SSH keeps starting
   remote cores, including on reconnect. Vega installs only vegad's login job;
   the operator removes the experimental `dev.vega.hc-core` job on ganymede.
   Amended (Slayer, 2026-11-06, vega-0h4c.13): slice 0 found that a mod no layout shows never runs
   `init()`. The vega mod is therefore a small, always visible vega status element. The Slayer chose
   a **vega-owned clone** of main: `config/herdr-app/layouts/vega.json`, unit `herdr-app.layouts`.
   Shipped main is restored at every start and rejects structural edits; a personal clone persists.
   First apply selects vega through `layout.switch`, which persists the choice in app-owned
   `shared.json` (not settings). The clone records its source app version; `vega config` reports
   version/hash drift. An explicit maintenance generator inserts the block at main's status anchor;
   its output is committed and reviewed. Apply never regenerates it. No automatic rebase.
   The element's `init()` starts vegad; it shows minimal relay state now, with labels, toasts and
   focus in slice 5. See [config](config.md) "Vega layout and startup mod".
6. **hc notes stay on; done goes through vega post (Slayer, 2026-11-06).** hc types a note into the parent
   only when a child cannot report itself (blocked, error, exit), never a done. A sentinel's done report
   stays a `vega post` mail: it crosses machines and survives a UAC handoff into a new terminal. vega's own
   silent detection can give way to hc's notes. No `--no-notify`, no `notes = false`.

7. **`hc machine` owns the fleet (Slayer, 2026-11-06).** It takes the role `herdr machine` has today:
   `vega deploy` reads `hc machine ls` instead of `herdr machine list`, and the registry stays the record of
   who is reachable, not an inventory. Moving it is one `hc machine add` (ssh) per saved herdr machine:
   ganymede, deimos, io (ssh host `io-wsl`), puck. Direction rule: a private host may add a corp host
   (proteus adds deimos, as with herdr), never the reverse: no corp host gets a private host as an hc
   machine, since that would let it run commands and drive agents there ([vegad](vegad.md), trust model).

8. **The vega mod is a config unit (Slayer, 2026-11-06).** Source `config/herdr-app/mods/vega/` in the
   repo; a new unit `herdr-app.mods` writes it whole to `~/.config/herdr-app/mods/vega/`, like the themes.
   Before writing, `herdr-app check` must pass on the new folder, else the change is rejected.

Verified in a scratch core (own `HC_SESSION`, scratch XDG, 2026-11-06): a child ends when its parent
terminal's root program ends; with `--outlive-parent` it stays `live` and keeps `parent`; a child started
by a command inside a shell terminal survives that command (the shell is the root program).

## World detection

- `HERDR_PANE_ID` set: TUI world (it is the more specific one: a TUI server started inside an hc terminal
  hands `HC_*` to every TUI pane).
- Else `HC_TERMINAL_ID` set (with `HC_TERMINAL_SOCKET`, `HC_SCOPE`): hc world.
- Either is verified against the process ancestry (herdr: the existing `me()` check; hc: the pids of
  `hc get $HC_TERMINAL_ID`), so a stale variable never claims a terminal.
- Neither: no terminal. Gate's fail-open message becomes "no terminal" instead of "no herdr".
- Long-lived daemons (vegad) strip `HC_*` as they strip `HERDR_*` today.

One helper in `crates/vega-herdr`, `own_terminal()`, returns `Herdr(w1:p2)` or `Hc(t7)`; every `me()` caller
that only needs its own identity moves to it.

## Addresses and the registry

Today no Rust code reads `HC_*`; `post` `valid_pane_id` and `vega_shim::is_pane` reject `t7`, and vegad
fills the registry from `herdr agent list` only, so an hc agent can neither be found nor reached.

- Canonical address: `herdr:w1:p2`, `hc:t7`, `hc:<machine>/t7`. Bare ids stay accepted where unambiguous
  (`w1:p2` has a colon, `t7` has none).
- Every registry entry and state record that holds a pane gains `backend: "herdr" | "hc"`
  (`<host>.agent.*`, `link`, `uac.v2`, `uber.v1`, `$XDG_STATE_HOME/vega/<kind>/<machine>_<pane>`).
- `machine/t7` is encoded in file names and KV tokens (no `/`).
- Agent kinds: hc says `claude-code`, the TUI `claude`; vega maps `claude-code` to `claude`
  (`theme::agent_topic`, `vega_transcript::Kind::from_herdr`).

## Gaps and how vega fills them

| Gap in hc / herdr-app | vega's answer |
|---|---|
| No harness session id (gate verdict, reopen, transcript) | The agent's own env (`CLAUDE_CODE_SESSION_ID`, `CODEX_THREAD_ID`), recorded in vega state at first contact |
| No env flag on `hc run` (API has `env`) | mux uses `terminal.run` over `hc connect`, with separate env and direct harness argv |
| No sidebar tokens, toasts, focus events | A vega herdr-app mod: renders `vega.*` labels, shows toasts and `$upgrade` from an hc doc, writes focus into an hc doc for `human.rs`/`waiting.rs` |
| No startup hook for vegad | The vega mod runs `vega-post relay start` in `init()` (decision 5); the TUI plugin stays a no-op-if-running starter until the TUI goes |
| No plugin panes or actions (board, open) | board as a terminal via `herdr-app do pane.split --argv`, open as a mod |
| `hc send` always presses Enter | Draft restore around a mail pointer is dropped for hc; `hc send --if idle` replaces the stash |
| Children end with their parent by default | `--outlive-parent` on every vega spawn (decision 4) |

## Slices

proteus keeps working after each one; each is testable with an `hc` fake, plus a scratch `HC_SESSION` and XDG
dirs for a real core (never the user's core).

0. **Foundation, no behaviour change.** World detection and `own_terminal()`, an `hc` runner next to
   `herdr()` (error `kind` to vega codes, `--on` instead of `--machine`), the address grammar in
   `pane.rs`/`vega_shim::is_pane`/post `valid_pane_id`, safe encoding, the `claude-code` alias, `HC_*`
   stripped from vegad.
1. **Gate reviewer.** Identity from `own_terminal()`; `Review` stores the backend; an hc author's reviewer
   starts with `hc run --bg --json --no-parent --outlive-parent --name gate-<id> --cwd <clone>
   --label vega.role=gate-reviewer --label vega.review=<id> -- env VEGA_GATE_ROLE=reviewer <kind> ...`;
   liveness, cleanup and reviewer state through `hc get/kill/rm`; verdict authorization by state file plus
   the `vega.review` label plus ancestry. Verdict mail uses `hc send <author>` directly, selected by the
   saved backend, until hc offers an atomic idle-or-done guard (vega-0h4c.17). Ends
   the `VEGA_GATE=skip` pushes from herdr-app (vega-yixb).
2. **Own identity everywhere.** `post inbox` parent lookup, `ci watch`, `notify`, `review`, orchestration
   `caller()`, the mux PreToolUse hook, `uber answer`. An hc agent can then read mail and run every tool.
3. **vegad sees hc.** A watcher on `events.subscribe`, registry entries with `backend`, mail pointers via
   `hc send --if idle`, supervision from hc status, a minimal vega mod that starts vegad. Mail then works
   in all four directions (TUI and hc, either way).
4. **mux on hc.** `vega mux spawn` in the caller's world (`--backend` overrides), `--outlive-parent`, visible through
   `herdr.space`/`herdr.tab` labels or `herdr-app do tab.new`; then `send --now`, `show`, `close`,
   `dialog`/`answer` (vega's classifier on `hc read`), `next` (`herdr-app do agent.focus`), `reopen` and
   `transcript` (session id from vega state). Machines are added to hc before `--machine` works for hc.
   Slice 4 fleet behavior: hc inventory falls back to the TUI list only when hc is
   absent or empty. Deploy displays via transports as unsupported and skips selected
   via targets during apply; unrelated SSH machines remain usable. Mux preserves
   qualified mail addresses; the machine-name/registry-host convention below now resolves them.
   hc explicit split/workspace placement is tracked in vega-0h4c.20; tab and wt-created
   spaces are implemented. Guides and README descriptions change with this slice.
5. **UAC and the UI.** UAC commands over both backends, the vega herdr-app mod, board and open, config
   doctor rows for `hc`/`herdr-app`, skills and help texts. Split: 5a `vega uac` over hc plus quieter
   attention (vega-0h4c.23, GitHub #8); 5b the UI, see [herdr-app UI](herdr-app-ui.md).
6. **Retire the TUI** (vega-0h4c.4): once no TUI agent runs on any host, delete the herdr backend, the
   `herdr` config unit, the herdr plugins, `shims/herdr` and `link-herdr.sh`.

Hosts move after slice 4 on proteus (vega-0h4c.3): ganymede, deimos, io, puck. Running TUI agents do not move live;
each restarts in herdr-app from a handoff with resume.

## Open questions for the Slayer

1. **board and open.** Still needed under herdr-app, or replaced by herdr-app's own panes and mods? To be
   looked at separately.

## Unknowns to settle in slice 0

- Where hc notes go once an `--outlive-parent` child's parent has ended, and whether notes reach a
  shell parent at all (an explicit `hc kill` typed none into a shell parent in the scratch test).
- Whether herdr-app tabs start the agent as the root program or inside a shell (decides when a child
  without `--outlive-parent` would end).
- Whether a mod loads (and runs `init()`) when no layout shows it, and with which env its `exec` runs.
- Whether `hc ls` lists other machines' terminals in one call.
- Whether `HC_TERMINAL_ID` survives toolbox and `podman exec`.
- Whether `hc keys` accepts literal text.
- Whether Claude's staged pastes still need `confirm_taken` under hc.

### Slice 0 findings (2026-11-06)

Tested on proteus/macOS with hc 0.1.18, herdr-app 0.1.13 and Claude Code 2.1.319.
All terminal mutations used scratch sessions `s0`/`s1`, runtime dirs `/tmp/hc-s0` and
`/tmp/hc-s1`, and XDG homes under this worktree's `target/hc-probes`. Each command
checked the scratch server PID against the recorded user-core PID. The second core
was connected as `remote` through a local `--via` script, without adding a fleet host.
The app used a separate `HERDR_APP_HOME` and the scratch core. Probe scripts and raw
results are local, uncommitted artifacts in `target/hc-probes`.

1. **Notes need a live agent parent.** A natural child exit (code 3) produced no
   input in a plain `/bin/sh` parent (`hc read` before/after). The same experiment
   with an idle Claude parent produced `<hc>... exited (exit 3). Read: hc read ...</hc>`
   in that parent's input. An `--outlive-parent` child survived killing its Claude
   parent, retained the same `parent` in `hc get`, and exited with code 4; no new
   note appeared in the finished parent's screen or the live shell. No retargeting
   was observed. Therefore notes cannot replace durable mail after a parent ends;
   decision 6's durable done mail remains necessary. Verification: `results2.log`,
   `notes.log`; natural exits were used as well as an explicit parent kill.
2. **App-launched programs have a shell root on this macOS build.** In a scratch
   app, `tab.new --argv '["sleep","120"]'` resulted in hc argv
   `["/bin/zsh","-lic","\"$@\"; exec \"$0\" -li","/bin/zsh","sleep","120"]`.
   The root PID belonged to zsh; `foreground.pid` belonged to sleep. The generated
   app guide also documents this wrapper for menu agents on macOS (and interactive
   shells on Linux). An agent finishing can leave the root shell alive; this does
   not justify dropping `--outlive-parent` for direct hc spawns. Verification:
   `app-results.log`, `app-terminals.log`, installed app `AGENTS.md`, “programs”.
3. **An unshown mod does not run `init()`.** A fresh scratch app contained a probe
   mod whose init used `exec` to write a marker. `status` showed no errors, but no
   marker existed until `pane.split --mod probe` displayed it. Its `exec` inherited
   the app's marker env, scratch XDG config and `HC_SESSION=s0`, with no terminal
   ID. The installed guide says copies start on first display and then stay alive
   while their block remains in the layout, even hidden. **This qualifies decision
   5:** a mod merely installed on disk does not guarantee vegad startup. Decision
   5 is unchanged here; `vega-0h4c.13` tracks choosing a guaranteed instantiated
   placement/startup mechanism. Verification: `app.py`, `app-results.log`,
   `mod-env.txt`, installed guide “A copy starts the first time it shows”.
4. **`hc ls` includes remote terminals in one call.** After adding the second
   scratch core with `machine add --name remote --via ... --session s1`, a terminal
   created with `run --on remote` appeared beside local terminals in one `ls` JSON
   result, addressed as `remote/t1`; `machines` reported the connection state.
   `get remote/remote-child` returned that same qualified address. Verification:
   `more.py`, `more.log`. `ls` has no `--on`; terminal operations use qualified
   addresses, whereas run/exec/file/doc operations accept `--on`.
5. **Plain `podman exec` drops hc identity; toolbox remains unverified.** A scratch
   hc terminal started an isolated Ubuntu container, then printed its own ID,
   the ID inside plain `podman exec`, and inside `podman exec -e HC_TERMINAL_ID`:
   `host=t14`, `plain-exec=`, `explicit-exec=t14`. The container was removed by a
   trap. Passing the variable alone does not establish ancestry or socket access.
   `command -v toolbox` found no toolbox on proteus or its Podman VM, so a real
   toolbox/ancestry check is still required on a Linux host (`vega-0h4c.12`). hc
   identity currently fails closed across an unverified process-tree boundary.
   Verification: `container.py`, `container2.log`; the Podman connection was
   explicitly selected because scratch XDG paths hide its normal connection config.
6. **`hc keys` is not arbitrary literal text.** `keys keyprobe 'hello world'`
   returned `invalid_params`/exit 6. `keys keyprobe a b space c enter` sent `ab c`
   to a `cat` terminal. Single-character literal keys work; multi-character strings
   are interpreted as key names. Verification: `more.log`, `keys --help`.
7. **The tested Claude multiline paste needs no extra `confirm_taken`.** In a
   scratch terminal with tools and hooks disabled, an idle Claude received a
   47-line prompt via one `hc send --wait --timeout 25`. It advanced its turn,
   replied `SLICE_ZERO_PASTE_OK`, and left an empty prompt, without another Enter
   or text retry. Verification: `claude.py`, `paste.py`, `paste.log` (status and
   screen). This confirms the idle delivery path for these versions, not every
   startup race: hc's guide explicitly requires checking `first_message.sent`
   and `not_started` before recovery. Keep the TUI `confirm_taken` unchanged;
   later hc delivery should use hc's own delivery/wait result.

Foundation storage uses `hc-` plus the hex bytes of the bare hc address for state
filenames and registry pane tokens: canonical `hc:io/t7` and bare `io/t7` share one
key, while `io.a/t7` and `io_a/t7` remain distinct. Legacy TUI state filenames and
registry keys retain their spelling. Parsing carries the backend explicitly;
opening/focusing hc addresses remains a later slice and fails explicitly for now.

### Slice 3 delivery contract (2026-11-07)

vegad consumes `hc events --json` (the `events.subscribe` snapshot/stream),
resubscribes on `missed`, and polls `terminal.list` through `hc connect` while
the stream is unavailable. All terminal inventories use the shared
`vega_herdr::hc::terminal_list` helper: `hc --json ls` is a display summary with
string status and no labels or foreground metadata. `terminal.list` and
`hc --json get` return full Terminal objects, including foreground cwd.
Snapshots, CI liveness, and waiting/supervision state are scoped by backend.
Only local hc terminals are published by this host; connected remote terminals'
paths are never resolved as local checkouts. Registry agent rows retain bare
`pane` IDs for existing readers and add `backend` and canonical `address` fields.
Canonical and bare targets share inbox subjects and consumers.

The parent confirmed parity with existing TUI delivery after a scratch hc
0.1.18 probe proved that `--if idle` rejects `done` (exit 10): fresh `idle` uses
`hc send --if idle`; fresh `done` uses plain `hc send`; parent/urgent mail may
also use plain send while `working`. No `--force`, and blocked/unknown states
wait. A failed send keeps the pointer pending. The get/send race can queue the
pointer if work starts meanwhile; hc still refuses blocked. An atomic hc
idle-or-done guard is follow-up vega-0h4c.17. hc owns its notes; vega keeps the
existing done/silent ledger. Native local hc parent links seed missing vega
parent records; an existing adoption wins, and discovery creates no new brief.
hc receives no draft-clear/restore keystrokes.

Cross-host post mail accepts `t7 --host <registry-host>` or a qualified hc address
under the convention below. Remote terminals never enter the local inbox.

### Machine names are registry hosts (Slayer, 2026-11-07, vega-0h4c.19)

An hc machine name must equal its vega registry host. `machine/t7` and
`hc:machine/t7` resolve directly to host `machine`, pane `t7`; no remote identity
query, alias mapping, SSH target matching, or hostname guessing is used. Post
checks that the host has a registry beat before enqueueing, including tool sends.
Unknown hosts fail with `machine_host_unknown`; unavailable registry checks fail
with `registry_unavailable`. A stale beat still identifies an existing host and
retains the normal reachability warning policy. A conflicting explicit `--host`
fails rather than remapping the machine-qualified address. Inbox-only senders
cannot verify remote registry names; they use an unqualified ID and explicit host
from a reply command. Local inbox-only delivery remains available.

`vega config infra --fleet` checks every configured hc machine name against the
registry and reports mismatches with `hc machine rename <old> <host>`. This is an
explicit network diagnostic; ordinary infra/onboarding checks stay local. The
diagnostic never renames machines or changes fleet membership. Missing hc or
registry access is an explicit failure, not a successful empty inventory.

### Remote spawn corrections (vega-0h4c.27)

Mux launches use JSON-lines `terminal.run` through `hc connect`, with direct harness
argv and a separate environment map. `env` and `nice` argv wrappers obscure the
program identity. hc overlays that map on the target core's base environment, so
PATH and HOME belong to the target; vega forwards terminal identity and explicit
spawn variables. Local hc spawns renice the returned PID after launch; TUI still
renices its shell before launch. The native parent and outlive-parent contract is
unchanged. A bounded RPC timeout reports uncertain completion and asks callers to
inspect terminals before retrying.

Before a remote launch, `machine.exec {on: <machine>, argv: [...]}` invokes the target's `vega-mux
prepare` helper. Existing trust and hook approval inheritance runs on that machine;
local paths/configs are not consulted for remote trust. Matching vega binaries must
be deployed to both ends. Unknown repositories and changed hooks still ask the user.

The live ganymede probe found Codex 0.160.0 reported no OSC 7501 status even with a
direct argv and a Ready screen. This is a harness deployment prerequisite, not a
reason to infer idle from screen text or bypass the slice 3 delivery contract. The
main session owns the harness upgrade and post-deploy idle-pointer verification.
Core startup/launchd remains vega-0h4c.25; macOS privacy prompts are documented in
[onboarding](onboarding.md#first-herdr-core-use-on-macos).

### Pinned hc wire contract (vega-0h4c.31)

`crates/vega-herdr/schema/hc-0.1.18.openrpc.json` is a byte-for-byte copy of
herdr-core's `schema/released.openrpc.json`, version 0.1.18 (also the local hc
version used for this migration). Upstream generates it from Rust types and
checks append-only compatibility within this protocol generation. We do not
depend on the unpublished herdr-core Rust crate.

Refresh from a released upstream checkout with:

```sh
python3 scripts/refresh-hc-schema.py repos/herdr-core
# In a worktree, pass the main checkout's herdr-core path if repos/ is absent.
nice -n 10 mise run check
```

When the version changes, update the include path in `hc_schema.rs`, remove the
previous snapshot and update this version note. Review the schema diff and any
new validation vocabulary. The small checker covers the generated vocabulary:
references, required/properties/additionalProperties, arrays, types, anyOf,
enums and numeric/array bounds; `format` is an annotation. Extra result fields
remain allowed for forward compatibility. Unsupported validation keywords fail
closed. A refresh must keep the checker vocabulary test passing.

`hc::rpc` checks every successful result against that method's result schema,
including Python subprocess fixtures and Rust-authored responses. Negative tests
exercise missing required fields, wrong types and an extra result envelope.
`hc_local`/`hc_on` retain their internal argv interface for existing callers but
translate the supported operations to RPC, rejecting unknown arguments. No hc
CLI display JSON is parsed. `machine.exec` still returns base64 stdout/stderr and
exit metadata; only the decoded program stdout is parsed as preparation JSON.
Caller identity, machine scope, send guards and explicit deadlines are retained.

Vegad uses a persistent plain-mode `events.subscribe` connection and checks its
initial result against the same schema. Notifications (`event`, `heartbeat`,
`missed`) are the exception to schema validation: OpenRPC describes method
results, while their shapes and stream semantics live in upstream's
`schema/README.md`. They remain handled by the stream parser and its tests.
Gate detached cleanup still invokes `hc kill`/`hc rm` with output discarded;
these are lifecycle commands without JSON reads. There are no remaining hc CLI
JSON exceptions. The independent herdr TUI
transport and its output shapes remain unchanged.
