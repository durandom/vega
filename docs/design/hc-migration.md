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
