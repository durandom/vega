# herdr -> hc / herdr-app inventory (vega)

Date: 2026-11-06, host proteus. Input to [hc-migration](hc-migration.md) (vega-0h4c.2); a snapshot, not kept in sync with the code.
Tools checked: `hc --help`, `hc <cmd> --help` (all commands), `hc --skill`, `hc schema` (API methods),
`herdr-app --skill`, `herdr-app commands`, `herdr-app status`, `~/.config/herdr-app/AGENTS.md`, `herdr.d.ts`.

## What hc / herdr-app give us (facts used below)

- Identity: `HC_TERMINAL_ID` (e.g. `t2`), `HC_TERMINAL_SOCKET`, `HC_SCOPE`. Addresses: `t7`, a name, `machine/t7`.
- `hc run [--bg] [--name] [--cwd] [--on m] [--label k=v]... [--parent|--no-parent] [--no-notify] [--outlive-parent] [--rm] [--keep] <cmd|template> [task]`.
  Template launch waits up to 60 s for readiness and reports `first_message.sent`. No `--env` on the CLI; API `terminal.run` has `env`.
  Default: a child ends with its parent unless `--outlive-parent`; hc types notes into the parent (blocked, error, exit) unless `--no-notify` or config `notes = false`.
- `hc ls [--all] [--label]`, `hc get <t>` (JSON: `address, name, status{app,state}, turn, labels, parent, pid, foreground{argv,cwd,pid}, records, first_message, keep, state`).
  No harness session id seen (`records` is keyed by an opaque id such as `tjgs2v9fj`; meaning unknown).
- `hc read <t> [--lines N]` (text; ANSI format option: none seen), `hc send <t> <text> [--if idle|working] [--force] [--wait]` (always presses Enter; refuses `blocked` unless `--force`; waits ~3 s for a person to pause),
  `hc keys <t> <keys...>`, `hc interrupt`, `hc wait <t> [--output X] | --children any|all`, `hc label <t> k=v|k=`, `hc kill`, `hc rm`, `hc keep`, `hc put`.
- `hc exec [--on m]`, `hc file read|write|edit [--on m]`, `hc machine add|ls|login|update|rename|rm` (`hc machine ls` is empty on proteus today).
- API (`hc schema`): `terminal.*`, `events.subscribe` (snapshot, then created/lost/restarted/keep/finished/removed/status/turn/title/labels/size/doc; heartbeat 15 s; `missed` + fresh snapshot), `doc.get/doc.set` (versioned opaque JSON per machine, emits `doc` events), `machine.*`, `hello`, `rpc.discover`. No focus event.
- Status words: `idle, working, done, blocked, error` (OSC 7501). App names: `claude-code`, `codex`, `pi` (herdr TUI says `claude`).
- herdr-app: `herdr-app ls|status|commands|do <cmd>|shot|check`. Relevant `do` commands: `space.new --name --folder --machine --argv --task`, `tab.new --name --space --folder --argv --task --mod --props`, `pane.split --direction --pane --argv --mod`, `pane.close`, `pane.zoom`, `pane.focus --terminal`, `agent.focus --terminal`, `terminal.open|keep|park|pin|kill|peek|show`, `tab.close|rename|settle`, `mod.open`, `overlay.close`.
  An hc terminal shows in the window only with labels `herdr.space=<id>` (+ optional `herdr.tab=<id>`).
  Toasts (`notify()`), sounds, focus (`focused()`), labels of every terminal (`terminals()`), `exec`, `hc(args)` exist only inside mods (JS, `~/.config/herdr-app/mods/<name>/`). No CLI toast, no startup hooks, no plugin manifest/actions/pane-entrypoints, no sidebar token API, no TTL metadata.
- `hc templates` is empty on proteus (`~/.config/herdr-core/templates/` does not exist).

Verdict legend: `hc native` (vega code can go), `adapt` (keep vega logic, swap backend), `keep` (vega-only), `gap` (missing; described).


## Part A: herdr inventory: mux, uac, orchestration, uber, janitor

All herdr CLI calls go through `crates/vega-herdr/src/lib.rs` `herdr()/herdr_local()/herdr_text()/herdr_on()`
(`herdr [--machine m] <args>`, JSON parsed from `result`). Paths are relative to `plugins/` unless they start with `crates/`.
"hc" means herdr-core CLI, "app" means `herdr-app do ...`.

Legend: **hc native** = vega code can go; **adapt** = keep the logic, swap the backend; **keep** = vega-only; **gap** = hc/app lacks it.

### Shared building blocks used by these plugins

| file:line | what it does for vega | world | hc / herdr-app equivalent | verdict |
|---|---|---|---|---|
| crates/vega-herdr/src/lib.rs:131-255 `herdr*()` runner, `--machine` prefix, error mapping, `socket_denied` | one place that shells out to `herdr` | machine-wide | `hc ... --json` (errors `{error:{kind,message,hint}}` on stderr, exit codes 3/7/9/10/11); `--on <m>` per command instead of a global `--machine` | adapt (second backend; map `kind` -> vega codes, `agent_not_found` -> exit 3 `not_found`) |
| crates/vega-herdr/src/lib.rs:337-470 `me()`: `pane list` + `pane process-info` per pane, process-tree walk, podman/conmon matching, `HERDR_PANE_ID` hint | finds the pane that hosts this process | own pane | `$HC_TERMINAL_ID` (+ `hc get $HC_TERMINAL_ID`). Unknown whether it is still correct inside `podman exec`/toolbox (env inherited, so probably yes) | hc native (drop process-tree walk in hc world; keep for TUI) |
| crates/vega-herdr/src/prompt.rs:29-60 `confirm_taken`: `agent wait --until working --timeout 3000`, `pane read --source visible`, `agent send-keys enter` x3 | makes sure a pasted brief was submitted (Claude staging, codex dropping Enter) | other agent's pane | `hc send <t> <text>` (waits for ~3 s typing pause, presses Enter) + `hc send --wait`; `hc keys <t> enter` for retries; `hc read <t>` | adapt (probably mostly hc native; unknown whether hc handles the Claude staged-paste and codex Starting cases; keep the check until proven) |
| crates/vega-herdr/src/dialog.rs (whole) `classify(screen, kind, status)` | conservative Claude/Codex dialog classifier on screen text | other agent's pane | hc status `blocked` says "a person is needed", no dialog kind/title/options | keep (classifier is screen-text only; feed it `hc read <t>`) |
| crates/vega-herdr/src/state.rs:12-40 records under `$XDG_STATE_HOME/vega/<kind>/<machine>_<pane>` | tasks, parents, modes, briefs, parked, done, silent per pane id | machine-wide | hc has `parent` (+ `--no-parent`, `--outlive-parent`), labels, `doc.get/doc.set` (opaque JSON docs, versioned, per machine) | keep (vega lineage/supervision); parent could become hc-native; key format must take `t7` / `machine/t7` |
| crates/vega-shim/src/lib.rs:120 `is_pane` = `<alnum>:<alnum>` | validates pane ids (used by orchestration) | — | hc addresses are `t7`, `t1b`, names, `machine/t7` | adapt (rejects every hc address today) |

### mux

| file:line | what it does for vega | world | hc / herdr-app equivalent | verdict |
|---|---|---|---|---|
| mux/src/main.rs:510-600 `home()`: `me()`, `agent list`, `workspace list`, `pane list`, `pane process-info` per pane | `vega mux` overview: agents, workspaces, panes, own pane | machine-wide + own | `hc ls --json` (address, agent, status, turn, where, cwd), `hc get`, `herdr-app ls` for spaces/tabs | adapt |
| mux/src/main.rs:625-680 `close()`: `agent get`, `workspace get`, `agent list`, `workspace close` / `pane close` | closes a spawned agent, or its worktree workspace if it is the last agent | other agent's pane | `hc kill <t>` + `hc rm <t>`; layout close: `herdr-app do tab.close --id` / `space.close --id --confirmed true` / `pane.close --pane` | adapt (hc native for the kill; workspace-is-worktree mapping is vega's) |
| mux/src/main.rs:682-700 `set_mode()` `agent get` | headed/headless mode record | other | `hc get` | keep (mode is vega state) |
| mux/src/main.rs:740-800 `show()`: `agent get`, `agent read --source <s> --lines n` | prints an agent's screen/history | other | `hc read <t> --lines N` (history reads limited to screen while working/blocked/error) | hc native |
| mux/src/main.rs:988-1050 `send()` / `send_post()`: `agent get` to resolve name->pane, then `vega post send` or direct paste | agent-to-agent message (mail or `--now` paste) | other | `hc send <t> <text>` for the paste; name resolution via hc `--name` | adapt (mail path keep; paste hc native) |
| mux/src/main.rs:2033-2070 `deliver()`: `me()`, `agent prompt <t> <body+footer>`, `confirm_taken`, `supervise` | direct paste with `[vega-mux] from` footer | other | `hc send <t> <body>`; hc also tags with `<hc from=...>` unless `notes = false` | adapt (footer is vega's; decide whether hc's own tag replaces it) |
| mux/src/main.rs:2136-2150 `supervise()` `agent get` | settles parent/child supervision on reply | other | hc `parent` relationship + `hc wait --children` | adapt |
| mux/src/main.rs:1295-1320 `spawn_inner` name check `agent get <name>` | refuse a taken agent name | machine-wide | `hc run --name` fails on a duplicate live name (names unique among live terminals) | hc native |
| mux/src/main.rs:1340-1435 spawn placement: `tab create --workspace --cwd --label --no-focus --env`, `pane layout` + `pane split --direction --no-focus --env`, `workspace create --label --no-focus --env`, `tab rename` | where the new agent appears (tab / split / workspace / worktree) | own workspace (layout) | headless: `hc run --bg --name --cwd --label k=v`; visible: labels `herdr.space=<id>` + `herdr.tab=<id>` on `hc run`, or `herdr-app do tab.new --name --space --folder --argv --task`, `pane.split --direction --pane --argv`, `space.new --name --folder --machine`. No `--env` on hc run CLI (API `terminal.run.env` exists); herdr-app tab.new has no env arg | adapt; gap: per-terminal env on CLI `hc run` (use API `env`, or `env K=V cmd` argv) |
| mux/src/main.rs:1151-1170 `with_env()`: split a worktree root pane with `--env`, close the old pane | herdr worktree commands take no `--env` | own workspace | not needed: `hc run` starts the program directly with env | hc native |
| mux/src/main.rs:1178-1198 `lower_priority()`: `pane process-info` -> `shell_pid`, `renice 10` | agents run below vegad priority | other (local only) | `hc get` gives `pid` (holder/shell) and `foreground.pid`; or launch `nice -n 10 <agent>` as argv | adapt (simpler: `nice` in argv) |
| mux/src/main.rs:1436-1505 `agent start <name> --kind --pane --timeout 60000 -- <args>` + `start_spawn` retry on `agent_pane_busy` | starts the harness in the pane, waits for readiness | other | `hc run --bg --name <n> --cwd <d> [--label ...] <kind> <args> "<task>"` or `hc run <template> "<task>"` (readiness up to 60 s, `first_message.sent`) | hc native (start + readiness + first message) |
| mux/src/main.rs:1459-1470 + 1085-1146 `spawn_args` codex `-c shell_environment_policy.set.HERDR_PANE_ID/HERDR_SOCKET_PATH/SSH_AUTH_SOCK` | codex tool shells keep the pane identity that ci watch, gate, orchestration read | other (child) | needs `HC_TERMINAL_ID`, `HC_TERMINAL_SOCKET`, `HC_SCOPE` passed the same way; hc says codex must run `--no-daemon` | adapt (swap var names; unknown whether codex's `inherit=core` drops HC_* too: assume yes) |
| mux/src/main.rs:1520-1600 failed spawn: `pane read --source recent --lines 40`, `pane close`, `pane read --source visible`, trust/hooks-review/dialog detection | reports why a spawn stopped; never confirms trust | other | `hc get <t>` `first_message:{sent:false,reason}`, `state finished` + `exit_code`, `hc read --lines 40`, `hc kill/rm`; app toasts a nonzero exit within 2 s | adapt (hc gives sent/reason; dialog text detection stays vega) |
| mux/src/main.rs:1662-1790 `deliver_brief` / `resume_spawn`: `agent get`, `dialog::read`, `same_agent` | `vega mux spawn --resume <pane>` delivers a kept brief after a dialog | other | `hc get` (`first_message`), `hc send` | adapt |
| mux/src/main.rs:2153-2200 `wait_started`: polls `pane read --source visible` 120x250 ms for codex ` · Starting · ` | codex drops Enter during startup | other | hc template launch "retrying startup settle failures"; `hc wait <t>`; `hc send` startup age limit | adapt (likely hc native for template launches; unknown for plain `hc run` + later send; keep until verified) |
| mux/src/main.rs:1993-1996 `no_agent()` `pane get` -> `agent` null | did the agent ever start | other | `hc get` `status`/`reported`/`state` | hc native |
| mux/src/main.rs:1855-1895 `ssh_target` / `remote_home`: `machine list --json` | `--machine` label -> ssh target, remote `$HOME` | machine-wide | `hc machine ls --json`; `hc exec --on m printenv HOME` | adapt |
| mux/src/main.rs:46-60 + crates/vega-herdr/src/lib.rs:59-78 global `--machine`, `qualify()` -> `io:w1:p2` | cross-machine targeting, user-facing ids | other machine | `--on <m>` on `run`/`exec`/`file`; every other command takes `machine/t7` addresses | adapt (address form changes from `m:w1:p2` to `m/t7`) |
| mux/src/dialog.rs:5-14, 95-122 `dialog::read`/`LivePane`: `agent get`, `pane read --source visible`, `pane send-keys`, `pane send-text` | `vega mux dialog` / `answer`: inspect and answer a dialog after re-checking the same question | other | `hc read <t>`, `hc keys <t> <keys...>`, `hc send --force <t> <text>` (send refuses blocked without `--force`) | adapt (answer logic keep; backend hc) |
| mux/src/hook.rs:47-60 `me()` + `agent get <me>` in the PreToolUse hook | finds own agent name to look up parent/mode, denies in-pane questions for headless children | own pane | `$HC_TERMINAL_ID` + `hc get` (`name`, `parent`) | adapt |
| mux/src/reopen.rs:40, 128-150 `pane get <record.pane>`, `agent get <name>`, `agent get <pane>` comparing `agent_session.value` | refuse reopen while the session still runs; detect pane-id reuse | other | `hc get <name>`; hc terminal addresses: unknown whether they are reused; session id: unknown (hc `get` shows `records` keyed by an id like `tjgs2v9fj`, not the harness session id) | gap (harness session id not exposed by hc, as far as seen) |
| mux/src/transcript.rs:70-100 `agent get` -> `agent`, `agent_session`, `cwd` | locates the session log for `vega mux transcript` | other | `hc get` gives `status.app` (claude-code/codex/pi) and `foreground.cwd`; session id unknown | gap (session id; falls back to "cwd's only log" today) |
| mux/src/next.rs:20-60 `agent list`, `machine list --json`, `pane::focus` (crates/vega-herdr/src/pane.rs: socket `pane.focus`, `tab focus`, `agent focus`) | `vega mux next`: focus the longest-waiting agent, any host | machine-wide + other machine | `herdr-app do agent.focus --terminal <t>` (sub-agents too), `pane.focus --terminal`; app shows remote spaces (`machine/` ids), so cross-machine focus likely works | adapt (cross-machine focus: likely better than TUI; unknown until tried) |
| mux/src/next.rs:146-152 `notification show <title> --position top-right [--body]` | toast naming the machine to switch to | machine-wide (this client) | none from CLI; `notify({title, body, level, action})` only from a mod | gap (needs a vega mod that receives toasts, e.g. via `doc` events or a file watch) |
| mux/src/worktree.rs:170-240 `open_workspace`: `worktree list --cwd --json`, `workspace create --label`, `worktree open --path --label --no-focus`, `tab create` | opens a git worktree as a herdr worktree workspace under the repo's root workspace | own workspace | `herdr-app do space.new --name <branch> --folder <path>`; the app's `spaces` mod nests worktree spaces under their repo; `worktree` mod exists | adapt (wt stays the worktree creator; app does the space) |
| mux/src/worktree.rs:245-275 `herdr_create`: `worktree create --cwd --branch --label` (no wt) | fallback worktree creation via herdr | machine-wide | none in hc; app has a `worktree` mod (unknown if scriptable) | adapt (drop herdr fallback; require wt or plain `git worktree add`) |
| mux/src/trust.rs:435 | git worktree list (not herdr) | — | — | keep |
| mux/src/tier.rs | no herdr | — | — | keep |
| mux tests: examples/reopen_herdr.rs, main.rs:2860 fake `herdr` on PATH | fakes the herdr CLI | — | an `hc` fake (now a Rust example), or `HC_SESSION` + scratch XDG for a real test core (hc skill) | adapt |

### uac

| file:line | what it does for vega | world | hc / herdr-app equivalent | verdict |
|---|---|---|---|---|
| uac/src/main.rs:175-205 `snapshot()`: `agent list`, `workspace list`, `tab list` per machine; `machines()` `machine list --json` | facility console over all herdr machines | machine-wide, every machine | `hc ls --json` (local) + per machine: `hc ls` shows only local? remote terminals are addressed `machine/t7`; unknown whether `hc ls` lists other machines' terminals. `herdr-app ls` for spaces/tabs | adapt (unknown: cross-machine listing in one call) |
| uac/src/main.rs:230-235 `me()` | the UAC's own pane | own pane | `$HC_TERMINAL_ID` | hc native |
| uac/src/main.rs:395-415 `pane read --source visible` + `dialog::classify` for blocked/unknown agents | show which sentinel is asking what | other | `hc read <t>`; `hc get` status `blocked` | adapt |
| uac/src/main.rs:755-840 `claim` (uses `me()`; registry record `pane`) | registers this session as the project's UAC | own pane | `$HC_TERMINAL_ID`; could also `hc label $HC_TERMINAL_ID vega.role=uac` | keep (role is vega's), adapt the id |
| uac/src/main.rs:943-965 `release`: `pane report-metadata <me> --source vega:vegad --clear-display-agent --clear-token role` | clears the sidebar UAC name/role token vegad set | own pane | `hc label <t> vega.role=` (removes) + a herdr-app mod that renders `vega.*` labels; or `terminal.pin` | gap (no sidebar metadata API; needs labels + a vega mod) |
| uac/src/main.rs:980-1050 `adopt`: `agent get` (local or `--machine`) | gives a foreign session the UAC as parent | other / other machine | `hc get <t>` / `machine/t7`; hc `parent` cannot be changed after start (no API seen) | adapt (vega parent record stays; hc parent: gap) |
| uac/src/main.rs:1055-1110 `park`/`unpark`: `agent get` | sentinel waits on a decision bead | other | `hc get` | keep (vega state), adapt lookup |
| uac/src/main.rs:1111-1135 `answer_here`: `HERDR_PANE_ID`, `agent list` | answer a decision from the UAC's pane | own pane | `HC_TERMINAL_ID`, `hc ls` | adapt |
| uac/src/main.rs:1700-1745 `qualify(m, pane)` | user-facing ids `m:w1:p2` | other machine | `machine/t7` | adapt |
| uac/src/land/live.rs:13, 33, 229 `pane list`, `agent list`: `pane_absent`, `agents`, `unoccupied` (occupants by cwd) | `vega uac land`: is the sentinel's pane gone, is the worktree still used by another pane | machine-wide | `hc ls --json` (`cwd` = launch folder; `hc get` `foreground.cwd` for current); `hc ls --all` for finished | adapt (current cwd needs one `hc get` per terminal: unknown whether `ls --json` has foreground cwd) |
| uac/src/land/prune.rs:17 `pane list` + `live::agents()` -> `check_paneless` | prune only worktrees no pane works in | machine-wide | as above | adapt |
| uac/src/land.rs, land/prune/tests.rs | git worktree (not herdr) | — | — | keep |
| uac/src/group.rs | project resolve only (no herdr CLI) | — | — | keep |

### orchestration

| file:line | what it does for vega | world | hc / herdr-app equivalent | verdict |
|---|---|---|---|---|
| orchestration/src/update.rs:21-25 `agent get <pane>` (2 s) | proves the local endpoint (pane id, name) of a wait | own pane | `hc get $HC_TERMINAL_ID` | adapt |
| orchestration/src/update.rs:40-55 `caller()`: `HERDR_PANE_ID` must pass `is_pane` + harness session env | wait identity: "a herdr agent pane is required" | own pane | `HC_TERMINAL_ID` (`t7`) | adapt (also `is_pane` must accept `t\w+` and `machine/t7`) |
| orchestration/src/update.rs:56-70, 319, 341 `pane@host` addresses validated by `is_pane` | coordinator address format | other / other machine | `t7@host` or `host/t7` | adapt |
| orchestration/src/update.rs:105-116, 170 `machine list --json` -> `hosts::registry_for_machine` | maps herdr machine labels to registry hosts to prove child-of-coordinator | machine-wide | `hc machine ls --json` (names; ssh host) | adapt |
| orchestration/src/home.rs:301 `machine list --json` (2 s) | question rows: which host is which machine | machine-wide | `hc machine ls --json` | adapt |
| orchestration/src/runtime.rs, main.rs | waits/gather ledgers, no herdr | — | — | keep |

### uber

| file:line | what it does for vega | world | hc / herdr-app equivalent | verdict |
|---|---|---|---|---|
| uber/src/main.rs:150-165 `here()`: `me()`, `pane list` | this host's panes replace the registry's copy for the Uber view | own + machine-wide | `$HC_TERMINAL_ID`, `hc ls --json` | adapt |
| uber/src/answer.rs:170-200 `session_log()`: `pane current --current` -> `agent == claude`, `agent_session.value`, `cwd` | reads the Uber-UAC's own Claude session log for `--capture` | own pane | own session: `$CLAUDE_CODE_SESSION_ID` env is enough (no herdr needed); `hc get` `status.app == claude-code` | adapt (use env; hc has no session id) |
| uber/src/answer.rs:202, capture_main.rs:51 `me()` | the Uber claim's pane must be the caller | own pane | `$HC_TERMINAL_ID` | adapt |
| uber/src/ask.rs, brief.rs | `output_within` for other commands, registry | — | — | keep |

### janitor

| file:line | what it does for vega | world | hc / herdr-app equivalent | verdict |
|---|---|---|---|---|
| janitor/src/live.rs:11-27 `pane list` -> (`foreground_cwd` or `cwd`, `pane_id`) | which pane works in a checkout (view only, empty without herdr) | machine-wide | `hc ls --json` + `hc get` `foreground.cwd` (or `where` label) | adapt |
| janitor/src/main.rs:221, 341 `daemon_health::status()` | vegad loop budget, not herdr CLI | — | — | keep |

### Parallel-world breakage (TUI and hc on one machine)

- `me()` (lib.rs:350) only knows herdr TUI panes: every command that calls it (`vega mux`, `mux send --now`, `mux spawn --at split|tab`, `uac` status/claim, `uber answer`, the PreToolUse hook) fails with `not_in_herdr` in an hc terminal. The hook fails silently (`let Ok(me) = me() else return`), so a headless hc child's questions are NOT redirected to its parent.
- Targets are resolved with `herdr agent get <name|pane>`: an hc caller can still reach TUI agents (the `herdr` CLI talks to the TUI server's default socket), but nobody can reach an hc terminal by name or `t7`. `vega mux send`, `close`, `dialog`, `show`, `uac adopt/park` toward an hc agent all return `agent_not_found`.
- vegad's registry entries (`<host>.agent.<pane>`) come from herdr's `agent list` only and record `{pane, name, kind, status, cwd, repo_key}`: no backend field. hc agents are absent from the registry, so `vega post send` / `send --uac` to them reports `pane_gone`/unknown and mail pointers are never typed into them.
- Telling worlds apart: herdr ids are `w<id>:p<n>` (`is_pane` = `x:y`), hc ids `t<n>` / names / `machine/t7`. Syntactically distinct, but names overlap (both use `--name`/agent names): a bare name lookup can hit either world. The registry should store an explicit `backend: herdr|hc` (and `scope` = `HC_SCOPE`) rather than rely on the id shape.
- State files `<machine>_<pane>` assume herdr ids; an hc `t3` and a herdr pane cannot collide by shape, but hc addresses can be names, and `machine/t7` contains `/`, which breaks the file name (`state_file`) and registry `token()` unless encoded.
- `mux spawn` from an hc terminal: placement `--at tab|workspace|split` calls `herdr tab/workspace/pane create` with `me().workspace` -> fails; even if forced, the child lands in the TUI while the parent is in hc, so hc's parent/children (`hc wait --children`) and die-with-parent do not apply.
- Codex children get `HERDR_PANE_ID`/`HERDR_SOCKET_PATH` via `shell_environment_policy`; `HC_TERMINAL_ID`/`HC_TERMINAL_SOCKET`/`HC_SCOPE` are not forwarded, so ci watch / gate / orchestration in a codex tool shell under hc see no identity at all.
- `orchestration` waits refuse any hc caller (`HERDR_PANE_ID` missing or `t7` fails `is_pane`).
- `uac release` / vegad sidebar tokens (`report-metadata`) have no effect on hc terminals; the UAC badge just does not show there (harmless, but invisible).
- Cross-machine: `--machine <label>` names a herdr machine (`herdr machine list`); hc machines (`hc machine ls`) are a separate list. On proteus today `hc machine ls` is empty, so every `--machine` flow stays TUI-only until machines are added to hc.

## Part B: herdr inventory: plugins/post (vegad, vega post) and crates/vega-registry

Test fakes (not inventoried as call sites): `main.rs:6308`, `main.rs:6446` (fake `herdr` binary), `history.rs:485`
(`tests/fixtures/history-herdr.sh`), `notify_delivery.rs:462`, `agents.rs:326` (fake socket), `service.rs:1117`
(env file parse), `main.rs:6458` (env_remove HERDR_PANE_ID/HERDR_SOCKET_PATH), `sidebar.rs:252-357`.

| file:line | what it does for vega | world | hc / herdr-app equivalent | verdict |
|---|---|---|---|---|
| agents.rs:1-300 (`spawn`, `stream`, `snapshot`, `subscriptions`) | vegad agent watcher: herdr socket `session.snapshot` + `events.subscribe` (pane.agent_status_changed, closed, exited, created, agent_detected, focused); feeds delivery, supervision, waiting ledger, human-input clock | machine-wide | API `events.subscribe` (snapshot, then created/lost/finished/removed/status/turn/title/labels/doc; heartbeat; `missed` + fresh snapshot). Status words idle/working/done/blocked/error map 1:1 | adapt (second watcher for hc; hc is simpler: one subscription, no per-pane re-subscribe). Focus: gap (see human.rs) |
| agents.rs:115 `check_schema` | `herdr api schema --json`, refuse unknown schema_version | machine-wide | `hc schema` / API `rpc.discover`, `hello` (generation, features) | adapt |
| agents.rs:136 `socket_path` | `HERDR_SOCKET_PATH`, else herdr default | machine-wide | `HC_TERMINAL_SOCKET` (in hc terminals), default `~/.local/state/herdr-core/default/default.sock`; `HC_SESSION` selects another | adapt |
| agents.rs:297 `poll_once` | fallback: `herdr agent list` every 3 s | machine-wide | `hc ls --json` (status, turn, labels, parent) | adapt |
| human.rs (whole) | last human input = herdr `pane.focused` event; Uber-UAC engaged window, registry beat `human_at` | machine-wide | none in hc events (no focus fact). herdr-app knows focus (`herdr-app status` -> `agent`, `focused_pane`; mod `focused()`) but exposes no event stream to outside processes | gap: no focus/human-input event. Options: poll `herdr-app status` (window only), or a mod that writes focus to an hc doc (`doc.set`, emits `doc` event) |
| delivery.rs (whole, pure) | pointer policy: deliver when idle/done, "pointed" until working again | other agent's pane | statuses from hc `status`; `hc send --if idle` gives an atomic only-when-idle send | keep (policy); input source adapt |
| main.rs:4128 `notify` | types the mail pointer: `herdr agent prompt <pane> <line>` | other agent's pane | `hc send <t> <text>` (pastes, Enter, waits ~3 s of human quiet, refuses blocked unless `--force`; `--if idle`) | adapt (hc covers send+Enter; the "held while person types" behavior is native) |
| main.rs:3948 `holds` (Drafts), 4033 | stashes a typed draft: `herdr pane send-keys <pane> ctrl+c` / `ctrl+e ctrl+u` | other agent's pane | `hc keys <t> ctrl+c` / `hc keys <t> ctrl+e ctrl+u` | adapt; likely partly hc native: hc send holds a person's keys while pasting, so draft stash may be removable for live typing (unknown for an already-typed, unsent draft) |
| main.rs:4081 `restore`, 4085 | types the stashed draft back: `herdr pane send-text` (no Enter) | other agent's pane | none: `hc send` always presses Enter. `hc keys` takes key names only (unknown whether it accepts literal text) | gap: paste-without-Enter |
| main.rs:4118 `screen`, notify_delivery.rs:192 `screen` | screen as ANSI: `herdr pane read --source visible --format ansi` (draft detection; notification preview) | other agent's pane | `hc read <t>` (text, `--lines 0` = visible); `--json`. ANSI/format option: unknown (not in help) | adapt; gap if ANSI is required (draft detection relies on dim/placeholder styling: check) |
| main.rs:4560 `local_dialog`, 6006 `turn_screen` | visible screen text for dialog classify / quota abort detection | other agent's pane | `hc read <t>`; hc status `blocked` covers permission dialogs natively | adapt (dialog::classify stays as fallback for dialogs hc doesn't call blocked) |
| main.rs:6085 `provider_abort` | `herdr agent get` -> kind + session id -> transcript log for quota abort | other agent's pane | `hc get <t>`: `status.app` (claude-code, codex, pi), `foreground.argv/cwd`, `records`. Harness session id: unknown (no field seen) | adapt; gap: agent session id |
| main.rs:1362 `local_pane_open` | is a local pane still open (`herdr agent list`) | other agent's pane | `hc get <t>` (exit 3 not_found, `state` live/finished) | adapt |
| main.rs:3427 `local_agents` | local agent list (theme typing, attentions) | machine-wide | `hc ls --json` | adapt |
| main.rs:3434 theme + theme.rs:219 `claude_panes` | types `/config theme=<mode>` into idle Claude panes; filters `agent == "claude"` | other agent's pane | `hc ls` filter `status.app == "claude-code"`, `hc send --if idle <t> "/config theme=..."` | adapt (note app name differs: `claude` vs `claude-code`) |
| main.rs:2076 `inbox`, 2147 `inbox_show` | finds own parent via `herdr agent get <me.pane>` (after `vega_herdr::me()`) | own pane | `HC_TERMINAL_ID` = own address; `hc get $HC_TERMINAL_ID` -> `parent` | adapt; `me()` itself (vega-herdr) is hc native via env |
| main.rs:4163 `attentions`, 4186 | `herdr machine list --json` to name machines | machine-wide | `hc machine ls --json` | adapt |
| main.rs:4759 `Layout::read_herdr`, 4772-4774 | pane/workspace/tab lists for sidebar metadata and project resolution | machine-wide | `hc ls --json` (cwd, foreground.cwd, labels `herdr.space`/`herdr.tab`); spaces/tabs: `herdr-app ls` (window only) | adapt; partial gap: workspace worktree info (`repo_key`, `checkout_path`) has no hc field, derive from cwd via git |
| main.rs:4800-5250 (metadata builders), 5049 `report_quiesced`, sidebar.rs:1-233 | sidebar: `herdr pane|workspace report-metadata <id> --source vega:vegad --ttl-ms --token k=v --clear-token --display-agent --title`; tokens kind, label, project, host, badge, git, role, UAC badge; refreshed before TTL | other agents' panes (all local) | `hc label <t> k=v` / `key=` (no TTL, no source namespace); herdr-app shows labels only via a mod reading `terminals()` labels; `title` is program-set, no external set | gap: no built-in token rendering, no TTL, no display-agent/title override. Plan: vega labels (`vega.*`) + a vega herdr-app mod |
| upstream.rs:102 | upgrade check finds agent panes in the vega checkout (`herdr pane list`) to mail | machine-wide | `hc ls --json` (cwd) | adapt |
| upstream.rs:174-200 `set_tokens` | `$upgrade` workspace token (TTL) | machine-wide (workspace) | `hc label` on terminals; spaces have no labels (herdr-app space ids are app-side) | gap: no space/workspace-level metadata; mod could read an hc doc instead (`doc.set vega-upgrade`) |
| local_send.rs:42, 80 | inbox-only host: find local target via `herdr agent list` / `agent get` | other agent's pane | `hc ls`, `hc get <t>` | adapt |
| local_send.rs:131-145 | types report/correction: `herdr agent prompt` + `prompt::confirm_taken` | other agent's pane | `hc send <t> <text>` (`--wait` confirms the turn started; `not_started` result) | adapt; confirm_taken likely hc native via `send --wait` |
| supervise.rs (whole) | children recorded by `vega mux spawn`, silent/done from status transitions | other agent's pane (children) | hc records `parent` natively; `hc wait --children any|all`; hc types notes into parent when a child blocks/errors/exits (`notify`) | adapt: keep vega's done/silent ledger (mail-based); parent tracking and exit/blocked notes are hc native (beware double notes: `--no-notify` or config `notes=false`) |
| notify_delivery.rs:125 | reads waiting ledger focus to suppress notifications | machine-wide | depends on focus (human.rs) | gap (focus) |
| identity.rs:1239-1240 | cutover: `herdr agent list` + `workspace list` -> `vega_registry::local_entries` | machine-wide | `hc ls --json` | adapt |
| pagerduty.rs:422-427 | PagerDuty ownership: local agents + workspaces -> project | machine-wide | `hc ls --json` + cwd->project | adapt |
| history.rs:539 | history snapshot tolerates failed `agent`/`workspace` lists | machine-wide | `hc ls` | adapt |
| reach.rs (whole) | registry reachability (`beat`, `agent`, `uac`, `uber` keys); pane_gone | other agent's pane, cross-host | none (vega registry) | keep; needs the address format to include hc terminals |
| main.rs:6162 `valid_pane_id` | accepts only `<alnum>:<alnum>` (herdr `w1:p2`) before any herdr call | other agent's pane | hc addresses `t7`, `t1b`, names, `machine/t7` | adapt: rejects every hc address today |
| service.rs:5, 538, 798-922 (`manifest`, `link_herdr_plugin`, `relay install/uninstall`) | vegad startup: writes `~/.local/share/vega/herdr-plugin/herdr-plugin.toml` (`[[startup]] vega-post relay start`), `herdr plugin list/link/unlink` | machine-wide | none in hc/herdr-app (no startup hooks seen; herdr-app mods could `exec` it but only while the window runs) | gap: use launchd/systemd user unit (vegad is per machine, independent of any UI) |
| service.rs:35-43, 509 | env vegad must not inherit: HERDR_SOCKET_PATH, HERDR_PANE_ID, HERDR_TAB_ID, HERDR_WORKSPACE_ID, HERDR_ENV, HERDR_PLUGIN_* | own pane (starter) | add HC_TERMINAL_ID, HC_TERMINAL_SOCKET, HC_SCOPE (else vegad started from an hc terminal looks like that terminal, and dies with it if it is a child: use `--outlive-parent`/no hc) | adapt |
| service.rs:443-503, 775 `daemon_path` | pins herdr's dir (HERDR_BIN_PATH) on vegad PATH; warns if herdr missing | machine-wide | pin `hc` dir (`~/.local/bin`) | adapt |
| main.rs:2925 `serve` (registry reconcile, 5721 `reconcile`, 5732-5740) | each round: `herdr agent list` + `workspace list` -> `local_entries` -> KV | machine-wide | `hc ls --json` | adapt |
| crates/vega-registry/src/lib.rs:186-300 `local_entries` | builds `<host>.agent.<pane>` = {pane: `a.pane_id`, name, kind: `a.agent`, status: `a.agent_status`, turns: `a.completion_seq`, cwd, repo_key: workspace `worktree.repo_key`, project, waiting}; `<host>.link.<child host>.<pane>` from state records | machine-wide | hc fields: `address`, `name`, `status.app`, `status.state`, `turn`, `foreground.cwd`; no repo_key | adapt; add a backend field |

### What the registry records

- Key `<host>.agent.<token(pane)>`, value `{pane, name, kind, status, turns, cwd, repo_key, project?, waiting?}`.
- Pane is the herdr id `w1:p2`; hc addresses are `t7` (local) or `machine/t7`. They are syntactically distinct
  (colon vs none), but no field says which backend; `kind` uses herdr names (`claude`), hc uses `claude-code`.
- Host comes from the key; there is no backend/world field, no socket, no HC_SCOPE.
- Links and UAC claims (`uac.v2` `pane`, `uber.v1` `pane`) store bare pane ids too; state files are
  `<kind>/<machine>_<pane>` (vega-herdr state.rs).

### Parallel-world breakage (TUI and hc on one machine)

- vegad sees only herdr TUI panes: hc agents never enter the registry, so `vega post send` to them reports
  unknown/pane_gone, and `send --uac` cannot resolve a UAC that runs in hc.
- Mail pointers are typed only via herdr; `valid_pane_id` rejects `t7`, so an hc recipient gets mail in the stream
  but never a pointer (silent loss of wake-up).
- An agent in hc calling `vega post inbox`/`send`: `vega_herdr::me()` fails (`not_in_herdr`, no HERDR_PANE_ID), so
  it cannot identify itself; reply footers and parent lookup break.
- Supervision: a child spawned in one world and parent in the other has no status stream joining them; silent/done
  detection runs only on herdr events. hc's own child notes (typed into the parent) can double vega's done mail.
- Pane id reuse: herdr reuses `w1:p2`; hc ids are different strings, so no collision, but state files keyed by
  `<machine>_<pane>` must not assume `w:p` parsing.
- Sidebar tokens and the `$upgrade` workspace token are written only to TUI panes; hc terminals show nothing until
  a herdr-app mod renders vega labels.
- Human-input clock (`human_at`, Uber-UAC engaged) and the waiting ledger use herdr `pane.focused`; time spent in
  herdr-app counts as "not engaged", so the Uber-UAC may treat the Slayer as away.
- vegad started by the herdr plugin exists only while the TUI server runs; a machine on herdr-app alone has no
  vegad unless started another way (launchd/systemd).
- Theme typing filters `kind == claude`; hc reports `claude-code`, so hc Claude panes never get `/config theme`.
- Registry readers cannot tell which backend to call for a pane without parsing the id; add `backend: herdr|hc`
  (or store hc addresses with a prefix) before mixing.

## Part C: herdr inventory: board, open, gate, review, config, small plugins, vega-herdr helpers, shims, scripts, docs

Scope: everything except mux, uac, post, orchestration, janitor, uber, and the vega-herdr dialog/state modules, which are covered elsewhere.
Verdicts: `hc native` (vega code can go), `adapt` (keep the vega logic, swap the backend), `keep` (vega-only), `gap` (missing in hc or herdr-app).
Facts checked against hc 2026-11 help and schema, and against `herdr-app commands` and AGENTS.md:
- `hc run` has no `--env` flag. The API `terminal.run` has `env`.
- `hc get` shows `status.app`, `foreground{argv,cwd,pid}`, `pid`, `labels` and `records{<id>:{app,state,title}}`. It shows no harness session id.
- herdr-app has no CLI notification command. `notify()` exists only inside mods.
- Mods can read terminal `labels` through `terminals()`.

### crates/vega-herdr (shared helpers)

| file:line | what it does for vega | world | hc / herdr-app equivalent | verdict |
|---|---|---|---|---|
| lib.rs:131-250 `herdr`/`herdr_local`/`herdr_text`/`herdr_on`/`run_herdr_on` | Runs the `herdr [--machine m] …` CLI with a deadline (daemon mode) and maps `{error{code,message}}`, sandbox `socket_denied` | all | `hc … --json`: errors are `{error:{kind,message,hint}}` on stderr, exit codes 3/7/9/10/11; `--on <machine>` per command instead of a global `--machine` | adapt: needs a second runner `hc(args)` with the error mapping `kind`→`code`. The bounded-timeout wrapper `output_within` stays (keep) |
| lib.rs:337-415 `me()` + 418-640 (process-tree walk, `pane list`, `pane process-info`, podman `container_pane`) | Finds the pane that hosts this process. HERDR_PANE_ID can be stale, so it verifies by pid ancestry | own pane | `$HC_TERMINAL_ID` + `hc get $HC_TERMINAL_ID` (`pid`, `foreground.pid`, `foreground.cwd`, `status.app`). The ancestry check can reuse `pid`/`foreground.pid` | adapt: hc's env is set per terminal, but the same staleness risk exists for long-lived daemons, so keep the ancestry check against `hc get`'s pids. The container path: unknown whether HC_TERMINAL_ID survives `podman exec`/toolbox |
| lib.rs:59-79 `machine()`/`mflag`/`qualify` (`io:w1:p2`) | Machine-qualified pane ids | other pane | hc address `machine/t7` | adapt: the id grammar changes (`/` instead of `:`, `tN` instead of `wX:pY`) |
| lib.rs:694-745 `status_rank`/`status_counts`/`agent_name`/`me_line` | Rendering of herdr agent JSON | — | hc status words `idle/working/done/blocked/error` (adds `error`) | adapt: field names `agent_status`→`status.state`, `agent`→`status.app` |
| socket.rs | Bounded JSON-line socket client | machine | `$HC_TERMINAL_SOCKET` (JSON-RPC, `hc schema`) | keep: generic. Callers adapt |
| pane.rs:28-120 `peel`/`is_pane_id`/`parse` | Recognizes `w1:p2`, `io:w1:p2`, `w1:p2 on io` in selected text | other pane | hc addresses `t7`, `t1b`, `vps/t7`, names | adapt: add the hc grammar. Both grammars must coexist during the transition |
| pane.rs:185 `machines()` (`herdr machine list --json`) | Lists the herdr machines | machine-wide | `hc machine ls --json` | adapt: the machine sets differ (`hc machine ls` is empty on this host today) |
| pane.rs:197 `exists()` (`pane get`) | Is the pane alive | other pane | `hc get <t>` (exit 3 not_found, `state` live/finished) | hc native |
| pane.rs:203-252 `focus`/`focus_local` (socket `pane.focus`)/`focus_remote` (`agent focus`, `pane get`+`tab focus`) | Jumps the human's view to a pane (vega open, board) | other pane, UI | `herdr-app do agent.focus --terminal t7` or `do pane.focus --terminal t7`; a remote one is `--terminal vps/t7` (AGENTS.md: remote ids are `<machine>/` prefixed) | adapt: herdr-app can focus a remote machine's terminal directly, which removes vega-8m7's "switch machine yourself" caveat (verify) |
| prompt.rs:29-60 `confirm_taken` (`agent wait --until working`, `pane read --source visible`, `agent send-keys enter`) | Makes sure a pasted prompt was submitted (Claude staging, codex flicker) | other pane | `hc send --wait <t>` reports `not_started`. `hc send` waits for a typing pause and hc retries startup settles | hc native for the wait. Keep a narrow `hc read` + `hc keys <t> enter` fallback until it is proven that Claude never stages under hc |
| notes.rs:324-345 `toast`, 360-390 `notify_project` (`notification show … --sound request`) | Desktop toast for the human, plus a local note store | machine-wide UI | none on the CLI. A herdr-app mod has `notify({title,body,level,timeout,action})` and `playSound` | gap: no CLI or API to post a toast. Option: a vega mod that watches an hc doc (`doc.set` name e.g. `vega-notes`, `events.subscribe` doc events) and calls `notify()`. The note store is keep |
| badges.rs:85-110 `uac_tokens`/`count_tokens` (args for `pane report-metadata --token/--clear-token`) | UAC pane sidebar badges (notes, decisions, waiting) | own/UAC pane UI | `hc label <t> vega.uac=… vega.notes=…` plus a mod that draws labels (`terminals()` has labels); agents/workspace-status mods are shipped mods (clone them with `mod.clone`) | gap: there are no sidebar tokens; a vega mod is needed. Labels have no TTL, so vega's TTL refresh logic simplifies |
| waiting.rs (fed by herdr status + `pane.focused` events) | Ledger of agents waiting on the Slayer, because herdr's `done` turns `idle` when looked at | machine-wide | hc `done` exists in AGENTS.md ("becomes idle once they look"). `events.subscribe` has no focus event; herdr-app knows focus (`status`, mods `focused()`) | adapt: the status source swaps to hc events. Focus is a gap in hc events (polling `herdr-app status` or a mod is the only focus source) |
| hosts.rs | Maps herdr machine labels (ssh target) to registry hosts | machine-wide | `hc machine ls --json` (fields unknown: target/ssh host?) | adapt; unknown whether hc exposes the ssh target |
| theme.rs:210-221 `agent_topic` (`claude`/`codex`/`pi`/`copilot`), 239 `CELL_BLANK` | Maps herdr agent kinds to symbols; works around herdr token whitespace trimming | — | hc `status.app` = `claude-code`, `pi`, `codex` (herdr.d.ts) | adapt: add the `claude-code` alias. CELL_BLANK becomes moot without herdr tokens |
| vegad_env.rs, service_config.rs, otlp.rs, history.rs, capacity.rs, resets.rs, lanes.rs, catalog.rs, cutover.rs, project.rs, mail*.rs, asks.rs, gather.rs | Named after the crate, but they do not talk to herdr | — | — | keep. The crate name is a misnomer: consider splitting `vega-herdr` into a `vega-core` part and a backend part |

### plugins/gate (reviewer path)

| file:line | what it does for vega | world | hc / herdr-app equivalent | verdict |
|---|---|---|---|---|
| start.rs:400, 478-483 | The author's identity is `HERDR_PANE_ID`. When it is empty the result is "no herdr, push not reviewed" (FailOpen, recorded as unreviewed) | own pane | `HC_TERMINAL_ID` (+ machine) | adapt: this is why gate says "no herdr" in herdr-app terminals today |
| start.rs:167 `skip` records `pane=HERDR_PANE_ID` | Audit metadata | own pane | `HC_TERMINAL_ID` | adapt |
| start.rs:846-854 `prompt(..., author_pane)`, 702 | The reviewer prompt tells it to `vega post send <author_pane>` | other pane | depends on `vega post send` reaching an hc terminal | adapt (blocked on the post slice; see the main inventory) |
| author.rs:45-70 `from_pane` (`herdr pane current --current`: `agent`, `agent_session.value`, `cwd`) + transcript lookup | Author kind and model, to exclude same-model reviewers | own pane | `hc get $HC_TERMINAL_ID`: `status.app`, `foreground.cwd`. Session id: none in hc, but `CLAUDE_CODE_SESSION_ID`/`CODEX_THREAD_ID` are in the caller's env | adapt: take the session from the env, not the multiplexer. `Kind::from_herdr` needs `claude-code` |
| start.rs:774-800 `spawn_args` → `vega mux spawn <tab> --kind k --at tab --cwd clone --prompt-file f --env VEGA_GATE_ROLE=reviewer --no-parent -- <model args>`; 884-905 parses `pane: ` from the output | Starts the reviewer agent in a new tab | other agent's pane (new) | `hc run --bg --no-parent --outlive-parent --name <n> --cwd <clone> --label vega.role=gate-reviewer --label vega.review=<id> -- env VEGA_GATE_ROLE=reviewer <kind> <args> "<prompt>"`. Optional visibility: `--label herdr.space=<id>` (+ `herdr.tab`) or `herdr-app do tab.new --name <tab> --folder <clone> --argv '[...]' --task …`. Or a template in `~/.config/herdr-core/templates/` | adapt: `hc run` has no `--env`, so wrap the program in `env K=V` (or use the API `env`). `--name` allows only `[a-z0-9-_]{1,32}`, so `tab_name(branch)` must be sanitized. Output parsing: JSON `address` instead of `pane: ` |
| start.rs:291-300 `pane_alive` (`herdr pane get`) | Running vs abandoned review | other pane | `hc get <t>` → `state == live` (exit 3/11 otherwise) | hc native |
| start.rs:302-318 `cleanup` (`herdr pane close`) | Closes an abandoned or retired reviewer | other pane | `hc kill <t>` then `hc rm <t>` (or `herdr-app do terminal.kill --terminal t`) | hc native |
| start.rs:320-345 `cleanup_detached` (nohup `sleep 5; rm …; herdr pane close <own>`) | The verdict closes the reviewer's own pane after a delay | own pane | `hc kill $HC_TERMINAL_ID; hc rm $HC_TERMINAL_ID` in the same nohup script. If the reviewer was started with `--rm`, kill alone is enough | adapt (one string) |
| start.rs:604-630 `reviewer_state` (`pane get`: `agent_status`, `agent`, `agent_session.value`, `cwd`) + `took_prompt` | Shows "stuck at dialog" / "never took prompt" | other pane | `hc get <t>`: `status.state` (`blocked` = dialog), `turn` (>0 = took a prompt), `first_message.sent` for template launches | adapt; hc simplifies it (the transcript lookup becomes unnecessary when `turn` advances) |
| start.rs:585-595 `is_sentinel` (`state::children()` by pane, machine None) | Next-step wording | other pane | — | adapt: the children records must store hc addresses |
| verdict.rs:64-110 `authorize` (HERDR_PANE_ID → state file; `pane get` `agent_session.value` == `CLAUDE_CODE_SESSION_ID`/`CODEX_THREAD_ID`) | Only the reviewer it spawned may give the verdict | own pane | `HC_TERMINAL_ID` → state file. hc exposes no session id (`records` keys like `tjgs2v9fj` are unknown and not proven to be a session id) | gap/unknown: no harness session id from hc. Replacement: verify that the caller's process descends from `hc get $HC_TERMINAL_ID`'s `pid`/`foreground.pid` (the same ancestry walk as `me()`), plus a label `vega.review=<id>` on the terminal set at spawn. Alternatively record the reviewer's `CLAUDE_CODE_SESSION_ID` from its first `vega gate` call |
| push.rs:249-252 | Refuses pushes from `VEGA_GATE_ROLE=reviewer` or from a gate-clone cwd | own pane | — | keep (env only; it works as long as the env reaches the tool shells, which the `env K=V` wrapper does) |
| lib.rs:27 `tool("herdr")` (`VEGA_GATE_HERDR`) | Test injection of a fake herdr | — | `tool("hc")` | adapt; tests/cli.rs fakes herdr (15 HERDR_ refs) |

### plugins/board

| file:line | what it does for vega | world | hc / herdr-app equivalent | verdict |
|---|---|---|---|---|
| herdr-plugin.toml (`[[panes]]` dock split + popup, `[[actions]]` toggle-dock/open-board/redeploy, `[[events]] tab.created` auto-dock), link-herdr.sh (`herdr plugin link`), mise.toml:44-54 | Installs the board as a herdr plugin | machine-wide UI | herdr-app: `herdr-app do pane.split --direction right --argv '["vega-board","--mode","dock"]'`, `do terminal.open --argv … --place right`, `do tab.new --argv`; key bindings in `keys.json`. A board as a native mod: `tab.new --mod` / `pane.split --mod` (JS, a rewrite) | gap: no plugin manifest, no plugin actions running shell scripts from keys (unknown whether `keys.json` can run an arbitrary command), no `tab.created` hook. Short term, run the TUI as a terminal via app commands |
| scripts/lib.sh:11-150 (`pane list` find-by-title, `plugin pane open`, `pane neighbor/swap/resize`, `tab list`), open-dock.sh:13, redeploy.sh:13 (`pane close`), auto-dock.sh (HERDR_PLUGIN_EVENT_JSON, HERDR_TAB_ID, `tab focus`), open-board.sh:14 (`plugin pane open` popup) | Dock layout management | UI | `herdr-app ls` (find panes), `do pane.split`, `do pane.close --pane`, `do pane.swap`, `do layout.resize` (layout nodes only, not panes: unknown), `do tab.select`; popup ≈ `terminal.peek` or a mod `openOverlay` | gap for pane resize and popup placement; adapt for the rest |
| main.rs:71-110 `resolve_repo_cwd` (HERDR_PANE_ID, HERDR_WORKSPACE_ID, `herdr pane list` focused pane cwd) | Scopes the board to the repo of the focused pane, since herdr starts it in the plugin dir | UI / other pane | `herdr-app status` (`agent`, `focused_pane`) + `hc get <agent>` `foreground.cwd`; or start the board with `--folder <repo>` | adapt (simpler: pass the cwd at launch) |
| app.rs:590-610 `toggle_zoom` (`herdr pane zoom --toggle --pane HERDR_PANE_ID`) | Docked ↔ fullscreen | own pane UI | `herdr-app do pane.zoom --pane <p>`; the pane id comes from `herdr-app ls` by terminal `$HC_TERMINAL_ID` | adapt |
| app.rs:1035-1060 auto-dock marker in HERDR_PLUGIN_CONFIG_DIR | Opt-in setting | — | `~/.config/vega/…` | keep (move off the herdr dir) |
| jump.rs (vega_herdr::pane resolve/focus) | Jumps to a pane id clicked in the detail view | other pane UI | see pane.rs focus | adapt via the shared crate |
| badges.rs, inbox.rs, ui/*, views/* (vega_herdr::theme, badges) | Symbols and counts | — | — | keep |
| SKILL.md:3-8 | "in a herdr pane", `vega.board.toggle-dock` | docs | — | adapt text |

### plugins/open

| file:line | what it does for vega | world | hc / herdr-app equivalent | verdict |
|---|---|---|---|---|
| herdr-plugin.toml (`[[actions]] selection`, `[[panes]] menu` popup 72x13), link-herdr.sh, mise.toml:50-52 | A key opens a menu for the selection | UI | a herdr-app mod (popup via `openOverlay({kind:"dialog"})`, run `vega-open` through `exec()`), bound in `keys.json` | gap: no plugin actions; it needs a mod |
| main.rs:268-290 `action` (HERDR_PLUGIN_CONTEXT_JSON: `focused_pane_cwd`, `focused_pane_id`, `selected_text`) | Collects the selection and cwd | UI | mod side: `focused()` (terminal, cwd); selection: unknown (no selection API seen in herdr.d.ts grep; `terminal.copy` + clipboard as a fallback, which the code already does) | gap (selection text) / adapt (cwd, terminal) |
| main.rs:293-311 `open_popup` (`plugin pane open --plugin vega.open --entrypoint menu --focus --env …`) | Shows the menu TUI | UI | mod `openOverlay`, or `herdr-app do terminal.open --argv '["vega-open","menu"]' --place down` (no float, no env) | gap (no popup terminal with env via CLI) |
| main.rs:694-705 `pane_screen` (`pane read --source visible --format text`) | Ranks candidates by the visible text | other pane | `hc read <t>` (`--lines 0` = screen) | hc native |
| main.rs:708-720 `notify` (`notification show`) | Shows an error when the popup failed | UI | none on the CLI (mod `notify`) | gap (see notes.rs) |
| main.rs:722 `herdr_bin` HERDR_BIN_PATH | Binary lookup | — | `hc` on PATH | adapt |

### Other plugins

| file:line | what it does for vega | world | hc / herdr-app equivalent | verdict |
|---|---|---|---|---|
| review/src/main.rs:253 `my_pane` = `me()`; 259 `herdr_local agent get` → agent_name for mode/parent lookup | Decides whether the owner works in this pane (where questions go) | own pane | `HC_TERMINAL_ID`; `hc get` `status.app` / `name` | adapt |
| ci/src/main.rs:234-252 `my_pane` (`me()`, `not_in_herdr` hint) | Which pane `vega ci watch` mails | own pane | `HC_TERMINAL_ID` | adapt; for slow waits, `hc run --bg … gh run watch` + `hc wait` is the hc-native pattern (the skill recommends it). ci's mail-on-done can stay (keep) |
| notify/src/main.rs:186, 235 `me()?.pane` | Context for `vega notify` asks | own pane | `HC_TERMINAL_ID` | adapt |
| notify/src/lib.rs:48 `notes::toast` | Toast channel | UI | none on the CLI | gap (mod) |
| deploy/src/main.rs:153-170 (`herdr machine list --json`), 321-326, 461 | Fleet = herdr machines | machine-wide | `hc machine ls --json`; remote build via `hc exec --on m` / `hc run --on m` | adapt: the machine lists must be in sync, or deploy must read the registry. hc `machine update` copies hc only, not vega |
| config/src/herdr.rs:1-222 (unit `herdr`: render `config/herdr/config.toml` (203 lines), `herdr config check` with HERDR_CONFIG_PATH, `herdr server reload-config`, HERDR_OWNED) + config/herdr/config.toml | Owns the TUI config: sidebar tokens/colours, keys | machine-wide | herdr-app.{keys,settings,themes} units already exist (herdr_app.rs: `herdr-app commands` to validate keys, `status` errors after a live reload) | keep until the TUI is gone, then delete. The sidebar-token colouring moves to a mod |
| config/src/herdr_app.rs:22-160 | herdr-app units (keys/settings/themes; validates against `herdr-app commands`, reads `status` errors) | machine-wide | — | keep (already the target). Note: `HERDR_APP_HOME` is not followed (line 22) |
| config/src/infra.rs:85-95, 403-405 (HERDR_BIN_PATH, `herdr` component row) | `vega config doctor` checks that herdr is installed | machine-wide | add `hc` and `herdr-app` rows | adapt |
| onboard/src/lib.rs:162-176 (`~/.local/share/vega/herdr-plugin/herdr-plugin.toml` = vegad startup hook), main.rs:202-266 | Onboarding status: vegad started by herdr, herdr config check | machine-wide | none: hc has no startup hooks. Use launchd/systemd for vegad | gap (startup hook; a vega-side fix belongs to the post slice) |
| theme/src/main.rs:51, 104 (`vega theme herdr` alias → config unit), 289 | Alias | — | — | keep / delete with the TUI |
| crates/vega-theme-sync/src/adapters.rs:27, 126-151, 446-460 | Reads the herdr `[theme]` (read-only) | machine-wide | herdr-app `app.theme` / themes unit | adapt (add a herdr-app adapter, or treat it as native: the app follows the OS?) — unknown |
| capacity, history, lanista, bd | Only use non-herdr helpers from the vega-herdr crate (`Failure`, `cell`, `relay_running`, `catalog`); lanista `DEFAULT_SEAT = "herdr-agents"` is a name | — | — | keep |

### Shims, scripts

| file:line | what it does for vega | world | hc / herdr-app equivalent | verdict |
|---|---|---|---|---|
| shims/herdr/src/main.rs:12 (`exec_real("herdr", HERDR_BIN_PATH)`), scripts/install-cli.sh:26 (`herdr-shim`) | A placeholder shim; it only resolves the real binary | — | an `hc` shim if hints are wanted | keep / delete with the TUI |
| crates/vega-shim/src/lib.rs:100-123 `session_id` (CLAUDE_CODE_SESSION_ID, CODEX_THREAD_ID, HERDR_PANE_ID), `pane_key` (HERDR_PANE_ID first), `is_pane` (`w3V:p17` grammar) | Per-session hint dedupe; the pane key for the gh/tea shims; uac/orchestration filter registry keys with `is_pane` | own pane | add `HC_TERMINAL_ID` (form `t7`, no colon) | adapt: `is_pane("t7")` is false today, so uac/orchestration would drop hc agents (`uac/src/main.rs:1304`, `orchestration/src/update.rs:43,59,319`) |
| shims/git/src/main.rs:39 | Hint dedupe via `session_id` | own | — | keep (works through the harness env) |
| scripts/vegad-refresh.sh, others | No herdr calls found | — | — | — |
| mise.toml:22, 44-54 | `cargo check -p vega-herdr`; install links the herdr plugins | — | — | adapt (drop the plugin links later) |

### Docs that instruct agents to use herdr

- skills/vega/SKILL.md:18: "not `herdr machine list`".
- plugins/mux/SKILL.md:28 (`herdr machine list`), plugins/mux/help/spawn.md:30-63 (env "through herdr", HERDR_PANE_ID/HERDR_SOCKET_PATH for codex, herdr worktree workspaces), help/reopen.md.
- plugins/post/SKILL.md:14, plugins/deploy/SKILL.md:8 (`herdr machine list`), plugins/board/SKILL.md:3-8, plugins/open/SKILL.md, plugins/config/SKILL.md, plugins/theme/SKILL.md, plugins/onboard/SKILL.md, plugins/ci/help/watch.md, plugins/uac/SKILL.md, plugins/uber/SKILL.md, plugins/orchestration/help/waiting.md, plugins/notify/help/test.md, plugins/lanista/help/pin.md, plugins/janitor/SKILL.md (1-6 mentions each).
- docs/: 37 files. The biggest are design/vegad.md (69), theme-symbols.md (54), vega-mux-mvp.md (39), vega-mux-battletest.md (33), pane-control.md (28), herdr-attention-ui.md (27), config.md (24), mux-next.md (20), agent-tooling-principles.md (19), concepts.md (15), gate.md (8). They are design records, not runtime guides, except concepts.md and agent-tooling-principles.md.
- Codex in hc: hc's skill says codex must run with `--no-daemon`. mux already does this for herdr (`default_args`).

### Gate slice (concrete changes so gate works inside an hc terminal)

1. Identity: one helper `own_terminal()` in vega-herdr returns `Backend::Hc(HC_TERMINAL_ID)` or `Backend::Herdr(HERDR_PANE_ID)`, and gate reads it everywhere it reads HERDR_PANE_ID (start.rs:167, 400, 448, 846; verdict.rs:70; author.rs:48). The "no herdr" fail-open then triggers only when neither is set (reword it to "no terminal").
2. Store the backend in `Review` (state file): `pane` = `t7` (or `machine/t7`) plus `backend: "hc"`. Then `pane_alive`, `cleanup`, `reviewer_state` and `cleanup_detached` dispatch on it, so in-flight herdr reviews keep working during the transition.
3. Spawn: for an hc author, start the reviewer with `hc run --bg --json --no-parent --outlive-parent --name gate-<sanitized> --cwd <clone> --label vega.role=gate-reviewer --label vega.review=<review id> -- env VEGA_GATE_ROLE=reviewer <kind> <default args> <model args> "<prompt text or 'read <prompt.md>'>"`. Read `address` from the JSON instead of `pane: `. Visibility for the Slayer is optional: `herdr-app do tab.new --name <tab> --folder <clone> --argv '[…]' --task …`, or add `--label herdr.space=<space>` (the author's `herdr.space` label from `hc get $HC_TERMINAL_ID`). Codex gets `--no-daemon`. Whether this goes through `vega mux spawn --backend hc` or directly through hc is the mux slice's choice; gate needs only argv → address.
4. pane_alive: `hc get <t> --json` → `state=="live"`. cleanup: `hc kill <t>; hc rm <t>`. cleanup_detached: `sleep 5; rm …; hc kill <own>; hc rm <own>` (or spawn with `--rm`).
5. reviewer_state: `hc get` → `status.state` (`blocked` = at a dialog; `turn==0` = never took its prompt; `first_message.sent=false` if started from a template). This drops the transcript-based `took_prompt` for hc.
6. author kind and model: `hc get $HC_TERMINAL_ID` → `status.app` (map `claude-code`→claude in `vega_transcript::Kind::from_herdr`), `foreground.cwd`. Session: `CLAUDE_CODE_SESSION_ID`/`CODEX_THREAD_ID` from the env.
7. verdict authorize: hc has no session id (gap). Replace the check with: the state file for `HC_TERMINAL_ID` exists, AND `hc get $HC_TERMINAL_ID` carries `labels."vega.review"` == the review id, AND this process descends from that terminal's `pid` (ancestry walk). Optionally pin the harness session id on the reviewer's first `vega gate` call and compare it later.
8. Verdict mail: `vega post send <author t7>` must reach an hc terminal (post slice). Until then, a fallback for hc authors is `hc send <author> "<verdict line>"` (hc refuses while the author is blocked, retries via `--if idle` are possible).
9. Tests: `tool("hc")` fake next to the herdr fake in tests/cli.rs.

## Answers

### 1. Parallel operation: what breaks when caller and target live in different worlds

Current state: no Rust code reads `HC_*` at all. Everything resolves identity through `vega_herdr::me()` (`herdr pane list` + process ancestry) or `HERDR_PANE_ID`, and targets through `herdr agent get`.

| caller -> target | what happens today |
|---|---|
| TUI -> TUI | works (unchanged). |
| hc -> TUI | `me()` fails `not_in_herdr` (gate fails open "no herdr", `post inbox` parent lookup, `ci watch`, `notify`, `review`, `uac claim/status`, `uber answer`, orchestration waits, `mux spawn --at tab/split`). Commands that do not need `me()` still reach TUI panes, since the `herdr` CLI talks to the TUI's default socket (e.g. `vega post send w1:p2` works). The mux PreToolUse hook fails silently, so a headless hc child's in-pane questions are not redirected. |
| TUI -> hc | the target is invisible: vegad lists only herdr agents, so hc terminals are not in the registry; `post send` reports unknown/`pane_gone`; `valid_pane_id` (post main.rs:6162) and `vega_shim::is_pane` reject `t7`; no mail pointer is typed; `mux send/show/close/dialog`, `uac adopt/park` return `agent_not_found`. |
| hc -> hc | both of the above: no identity and no target. Only raw `hc send` between agents works. |
| supervision across worlds | children are tracked from herdr status events only; an hc child of a TUI parent (or the reverse) is never marked done/silent. hc's own parent notes can duplicate vega's done mail when both run. |
| UI side effects | sidebar tokens, UAC badge, `$upgrade` token, toasts, `vega mux next`/`vega open` focus, the human-input clock (`pane.focused`) and the waiting ledger see only the TUI. Time spent in herdr-app counts as "Slayer away" for the Uber-UAC. |
| vegad lifetime | vegad is started by the herdr plugin `[[startup]]` hook: no TUI server, no vegad. |
| theme typing | filters `kind == "claude"`; hc reports `claude-code`. |

Does the registry record enough? No.
- `<host>.agent.<pane>` holds `{pane, name, kind, status, turns, cwd, repo_key, project?, waiting?}`, built only from `herdr agent list` + `workspace list`. hc terminals are absent.
- No backend/world field, no `HC_SCOPE`/socket. The id shape tells them apart (`w1:p2` has a colon; `t7` and `machine/t7` do not), but hc names (`--name review`) collide with herdr agent names, and `kind` values differ (`claude` vs `claude-code`).
- `link`, `uac.v2`, `uber.v1` store bare pane ids; local state files are `<kind>/<machine>_<pane>`: a `machine/t7` address puts a `/` into the file name and the KV token unless encoded.
- Recommendation: add `backend: "herdr" | "hc"` to every entry and state record that holds a pane, plus a canonical address (`hc:t7`, `hc:vps/t7`, `herdr:w1:p2`), before mixing worlds.

Detection caveat: env alone can mislead. If the herdr TUI server is started from inside an hc terminal (e.g. the TUI run in a herdr-app tab), every TUI pane inherits that terminal's `HC_TERMINAL_ID`/`HC_TERMINAL_SOCKET` and also gets `HERDR_PANE_ID`. Rule: `HERDR_PANE_ID` set -> TUI world (it is the more specific one); else `HC_TERMINAL_ID` -> hc; and verify (herdr: existing ancestry check; hc: `hc get $HC_TERMINAL_ID` pids against our ancestry), as `me()` already does for stale `HERDR_PANE_ID`. Long-lived daemons (vegad) must strip `HC_*` like they strip `HERDR_*` (post service.rs:35-43).

### 2. vega mux features: covered by hc / herdr-app or not

| mux feature | hc / herdr-app | status |
|---|---|---|
| start agent + wait for readiness + first message (`agent start --timeout 60000`, `wait_started`, `confirm_taken`) | `hc run --bg --name <n> --cwd <d> <kind> <args> "<task>"`, or `hc run <template> "<task>"` (`first_message.sent`/`reason`); `hc send --wait` reports `not_started` | covered (keep a narrow Claude-staging/codex-Enter fallback until verified) |
| spawn templates (kinds, default args, tiers, model pins) | `~/.config/herdr-core/templates/<name>.md` (`command` incl. model, body = instructions); none installed | partly: the mechanism exists; vega's tier/lanista/capacity choice stays vega (could generate templates) |
| per-spawn env (`--env`, `BEADS_ACTOR`, `VEGA_GATE_ROLE`, `SSH_AUTH_SOCK`, codex `shell_environment_policy`) | API `terminal.run.env`; no CLI flag; herdr-app `tab.new` has no env | gap on CLI (use `env K=V cmd` argv or the API). Codex must forward `HC_*` instead of `HERDR_*` and run `--no-daemon` |
| placement tab/split/workspace/worktree | headless by default; visible via labels `herdr.space`/`herdr.tab` or `herdr-app do tab.new|pane.split|space.new` | covered for tab/split/space; worktree creation stays wt/git; app nests worktree spaces itself |
| unique agent names | `--name` unique among live terminals (`[a-z0-9_-]{1,32}`) | covered (sanitize names) |
| parent/children, supervision notes | `parent`, `--no-parent`, `hc wait --children any|all`, notes into parent on blocked/error/exit | partly: lineage + exit/blocked notes native; vega's done/silent/briefs/mail ledger stays. Decide `--outlive-parent` (default: children die with parent, which is wrong for UAC sentinels that must survive a UAC restart) and `notes=false`/`--no-notify` to avoid double reports |
| sidebar status / tokens (`report-metadata`, TTL, display-agent, title, UAC badge) | `hc label` (no TTL, no title override); rendering only via a herdr-app mod reading `terminals()` labels | gap: needs a vega mod; vegad writes `vega.*` labels (and an hc doc for space-level items like `$upgrade`) |
| pane metadata (kind, status, cwd, turns) | `hc get/ls`: `status.app/state`, `turn`, `foreground.cwd`, `labels` | covered; repo_key/checkout path: derive from cwd via git |
| harness session id (`agent_session.value`) for reopen, transcript, gate verdict | not exposed (unknown if `records` ids relate) | gap: take it from the agent's own env (`CLAUDE_CODE_SESSION_ID`, `CODEX_THREAD_ID`) and record it in vega state at first contact |
| reopen (`vega mux reopen`) | `hc keep` recovers a shell on loss but never reruns the command | gap for harness resume: keep vega's reopen, needs the session id above |
| dialog inspect/answer (`mux dialog`, `answer`) | `blocked` status, `hc read`, `hc keys`, `hc send --force` | partly: detection native (`blocked`), classification (which dialog, options) stays vega's `dialog::classify` on `hc read` text |
| show screen/history | `hc read --lines N` | covered (history reads return screen only while working/blocked/error) |
| direct paste with footer (`mux send --now`) | `hc send`; hc tags `<hc from="t3 name">` itself unless `notes=false` | covered; footer becomes optional |
| draft stash/restore around a pointer | `hc send` holds a person's keystrokes while it pastes | partly: restore needs paste-without-Enter, which hc lacks (`send` always submits) |
| close (agent, last-agent workspace) | `hc kill` + `hc rm`; `herdr-app do tab.close|space.close --confirmed true` | covered |
| renice | `nice -n 10` in argv | covered (simpler) |
| cross-machine spawn / send / read | `hc run --on m`, every command takes `machine/t7`, `hc exec --on`, `hc file --on`; app `space.new --machine` | covered by design, but `hc machine ls` is empty: machines must be added (`hc machine add`) and kept in sync with `herdr machine list`, `vega deploy` fleet and registry hosts (`hosts.rs`) |
| `mux next` focus across machines | `herdr-app do agent.focus --terminal vps/t7` | likely better than TUI (vega-8m7); unknown until tried |
| toasts (`notification show`) | none on CLI; mod `notify()` | gap: vega mod fed by an hc doc or file |
| focus / human input events | none in `hc events`; app-side only | gap: a mod that writes focus into an hc doc (`doc.set`, emits `doc` events) |
| vegad startup hook, board/open plugin panes and actions | none | gap: launchd/systemd for vegad; board as a terminal via `herdr-app do pane.split --argv` (later a mod); open as a mod |
| status stream for vegad | `events.subscribe` (one subscription, status/turn/labels/doc, `missed` resnapshot) | covered (simpler than herdr's per-pane subscriptions) |

### 3. Suggested slice order (proteus keeps working after each slice)

Principle: the backend follows the world of the agent concerned (caller for own identity, recorded backend for a target). The TUI path is untouched until the last slice; every slice is additive and testable with an `hc` fake plus `HC_SESSION` + scratch XDG for a real test core (never the user's core).

0. **Foundation, no behavior change.** In vega-herdr: `World` detection (rule above), `own_terminal()` returning `Herdr(w1:p2)` or `Hc(t7)` with ancestry verification, an `hc` runner next to `herdr()` (error `kind` -> vega codes, `--on` instead of `--machine`), an address grammar accepting both (`pane.rs` `parse`, `vega_shim::is_pane`, post `valid_pane_id`), safe encoding of `machine/t7` in state file names and KV tokens, `claude-code` alias in `theme::agent_topic` and `vega_transcript::Kind::from_herdr`. Strip `HC_*` from vegad's env. TUI unchanged.
1. **gate reviewer (first user-visible slice).** As in "Gate slice" above: identity from `own_terminal()`; `Review` stores `backend`; for an hc author spawn the reviewer with `hc run --bg --json --no-parent --outlive-parent --name gate-<id> --cwd <clone> --label vega.role=gate-reviewer --label vega.review=<id> -- env VEGA_GATE_ROLE=reviewer <kind> ... "<prompt>"` (codex `--no-daemon`); `pane_alive`/`cleanup`/`reviewer_state`/`cleanup_detached` dispatch on backend; verdict authorization by state file + `vega.review` label + ancestry (session id gap); author model from env session id. Verdict mail: `hc send <author>` fallback until slice 3. TUI authors keep the herdr path, so in-flight reviews survive.
2. **Own identity everywhere.** Switch the `me()` callers that only need "who am I" to `own_terminal()`: `post inbox`/`inbox_show` parent lookup (`hc get` `parent` or vega state), `ci watch`, `notify`, `review`, orchestration `caller()`, mux PreToolUse hook, `uber answer` (session from env). After this an hc agent can read mail and run tools; it still cannot be reached.
3. **vegad sees hc.** Second watcher on `events.subscribe` (poll `hc ls --json` fallback); registry entries for hc terminals with `backend`; delivery types pointers with `hc send --if idle`; local_send/theme/pagerduty/upstream/identity read `hc ls`; supervision consumes hc status; vegad installed as a launchd/systemd user unit (independent of the TUI plugin, which stays as a no-op-if-running starter). Now mail works in all four directions.
4. **mux on hc.** `vega mux spawn` picks the caller's world (override `--backend`): `hc run` with name/labels/env wrapper, visible via `herdr.space`/`herdr.tab` labels or `herdr-app do tab.new`; decide `--outlive-parent` and `notes=false`. Then `send --now`, `show`, `close`, `dialog`/`answer`, `next` (`herdr-app do agent.focus`), `reopen`/`transcript` (session id recorded by vega). Add hc machines (`hc machine add`) and reconcile machine lists before enabling `--machine` for hc.
5. **UAC and UI surfaces.** `uac status/claim/release/adopt/park/land/prune` over both backends; a vega herdr-app mod that renders `vega.*` labels (role, badges, project, git), shows toasts and the `$upgrade` item from an hc doc, and writes focus into an hc doc for `human.rs`/`waiting.rs`; board as a terminal pane via `herdr-app do pane.split --argv`, open as a mod; `config` doctor rows for `hc`/`herdr-app`; update skills/help texts (`skills/vega/SKILL.md`, `plugins/*/SKILL.md`, `plugins/mux/help/spawn.md`).
6. **Retire the TUI.** Once no TUI agents remain on any host: delete the herdr backend, `config/herdr` unit, herdr plugins (`vega.vegad`, `vega.board`, `vega.open`), `shims/herdr`, `link-herdr.sh`, mise plugin links; consider renaming `vega-herdr` (most modules there are terminal-independent).

Risks to settle early:
- ⚠️ Default hc lineage: children die with their parent. A UAC restart would kill its sentinels unless spawned `--outlive-parent`.
- ⚠️ No harness session id from hc: gate verdict, reopen and transcript need a vega-side record.
- ⚠️ `hc machine ls` is empty on proteus: any cross-machine hc slice needs machines added and a single source of truth for the fleet.
- Unknown: whether `hc ls` lists other machines' terminals in one call; whether `HC_TERMINAL_ID` survives toolbox/podman exec; whether `hc keys` accepts literal text; whether Claude staged pastes need `confirm_taken` under hc.
