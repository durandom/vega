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
   `vega.vegad` herdr plugin's `[[startup]]` hook does today. launchd/systemd stay gone
   ([vegad](vegad.md) "Lifecycle", vega-lhc.54); on a host without a window, `vega deploy` already starts a
   stopped vegad. The core outlives the app (macOS starts it as a one-shot launchd job), so vegad must
   detach; `relay start` already does (setsid).
   Amended (Slayer, 2026-11-06, vega-0h4c.13): slice 0 found that a mod no layout shows never runs
   `init()`. The vega mod is therefore a small, always visible vega status element in the main layout
   (it shows vega state in slice 5 anyway); being shown starts vegad. The layout file becomes a vega
   config unit too.
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
| No env flag on `hc run` (API has `env`) | `env K=V <cmd>` in argv |
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
   the `vega.review` label plus ancestry. Verdict mail falls back to `hc send <author>` until slice 3. Ends
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
5. **UAC and the UI.** UAC commands over both backends, the vega herdr-app mod, board and open, config
   doctor rows for `hc`/`herdr-app`, skills and help texts.
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
