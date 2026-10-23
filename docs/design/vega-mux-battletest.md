# vega mux battle test: Codex and pi

Date: 2026-10-06. Tester: claude agent in `w3V:pJ`, requested by `w3V:p1`.

## Setup

- Two panes split from `w3V:pJ` with `herdr pane split --no-focus`, cwd `~/src/durandom/vega`.
- `herdr agent start bt-codex --kind codex --pane w3V:pP` (codex-cli 0.160.1, GPT-6.1-Sol medium).
- `herdr agent start bt-pi --kind pi --pane w3V:pQ` (pi, grok-4.7 via xai).
- Later: `herdr agent start bt-codex2 --kind codex --pane w3V:pP -- --no-daemon`.

Startup notes (not vega bugs):

- First codex start showed the "Sign in with ChatGPT" screen and `herdr agent start` timed out,
  although `codex login status` said `Logged in using ChatGPT`. The user logged in by hand.
- The user ran `pi update` in the pi pane between attempts; panes were recreated afterwards.

## Summary

| # | Finding | Severity |
|---|---------|----------|
| F1 | Codex in default (daemon) mode runs commands with stale `HERDR_*` env from the pane where the shared app-server daemon started | Critical |
| F2 | Codex sandbox blocks the herdr socket; every reply needs a manual approval | High |
| F3 | Sandbox error is a raw Rust `Os {...}` debug string with a useless hint | Medium |
| F4 | `vega mux send <target> ""` sends a footer-only message | Medium |
| F5 | `show` does not strip Codex or pi chrome | Medium |
| F6 | Title column shows terminal titles for codex/pi (`vega`, `π - vega`, `[ ! ] Action Required \| vega`) | Low |
| F7 | Empty target is `agent_not_found` (exit 1), not a usage error (exit 2) | Low |
| F8 | Codex security popup stays on screen while herdr reports `idle` | Low (external) |
| F9 | pi's `glla` extension left an untracked `.pi-glla/` dir in the repo | Low (external) |

What works: delivery by name and by pane id, special characters, multi-line text, send to a working agent,
rejection of blocked/unknown/self targets, footer acceptance by both agents, and loop safety.

## F1 Critical: Codex daemon mode uses stale herdr env

Codex without `--no-daemon` runs shell commands through a shared, long-lived daemon
(`~/.codex/packages/app-server-daemon/.../codex app-server --listen unix:// --manage`).
That daemon keeps the environment of the pane it started in.

Evidence. Codex in `w3V:pP` ran `env | grep ^HERDR`:

```
HERDR_PANE_ID=w3E:p1
HERDR_TAB_ID=w3E:t1
HERDR_WORKSPACE_ID=w3E
```

The codex TUI process itself has the right env (`ps eww`):

```
92256  HERDR_PANE_ID=w3V:pP  HERDR_WORKSPACE_ID=w3V
```

The daemon processes have the stale env:

```
46085  started Mon Oct 5 21:19:38  codex app-server --listen unix:// --manage   HERDR_PANE_ID=w3E:p1
50396  started Mon Sep 28 16:56:29 codex app-server daemon pid-update-loop      HERDR_PANE_ID=w3E:p1
99425  started Tue Oct 6 17:49:51  codex-code-mode-host                         HERDR_PANE_ID=w3E:p1
```

Workspace `w3E` no longer exists (`herdr workspace list` shows only `w3V`, `w3W`).
The codex shell snapshot `~/.codex/shell_snapshots/01a111e7-db92-...sh` (written 17:49:30) also holds `w3E:p1`.

Effect. The approved reply `vega mux send w3V:pJ "codex; exact model ID unavailable"` failed:

```
error{code,message,hint}:
  pane_not_found,pane not found,run `vega mux` to check the current state
```

If it had succeeded, the footer would name `w3E:p1` as sender, so replies would go to the wrong (or a dead) pane.
Any herdr-aware tool run from such a codex session has the same problem.

With `--no-daemon` the env is correct (`HERDR_PANE_ID=w3V:pP`) and the reply arrived with the right footer:

```
w3V:pP
---
[vega-mux] from w3V:pP (codex in ~/src/durandom/vega)
```

Status 2026-10-06: fixed. `vega mux spawn` starts codex with `--no-daemon`, and `vega mux` identifies its pane by process ancestry via `herdr pane process-info`; a stale `HERDR_PANE_ID` now gives `not_in_herdr`.

Suggested fixes (pick one or more):

- `vega mux` resolves "me" from herdr (for example by process ancestry or `herdr pane current`) instead of trusting `HERDR_PANE_ID`; or verify that `HERDR_PANE_ID` hosts an agent whose pid is an ancestor, and error out otherwise.
- Detect the mismatch (`HERDR_WORKSPACE_ID` not in `herdr workspace list`) and print an error with the hint "codex daemon mode: start codex with --no-daemon".
- Document `--no-daemon` for codex in the vega skill, and pass it from the future `vega mux` spawn command.

## F2 High: Codex sandbox blocks the herdr socket

Codex has no `sandbox_mode` in `~/.codex/config.toml`, so it uses the default sandbox.
The socket is `/Users/mhild/.config/herdr/herdr.sock`, outside the writable roots.

Sandboxed run (`env | grep ^HERDR; vega mux; herdr pane get w3V:pJ`):

```
error{code,message,hint}:
  herdr_failed,"Error: Os { code: 1, kind: PermissionDenied, message: \"Operation not permitted\" }",run `vega mux` to check the current state
Error: Os { code: 1, kind: PermissionDenied, message: "Operation not permitted" }
```

Codex then asks for escalation on each reply:

```
Reason: Allow sending the pane ID to w3V:pJ? The sandbox denied access to the mux.
$ vega mux send w3V:pJ "w3V:pP"
› 1. Yes, proceed (y)
  2. Yes, and don't ask again for commands that start with `vega mux send w3V:pJ w3V:pP` (p)
```

The "don't ask again" option is scoped to the exact message text, so it never helps for the next reply.
Result: every codex reply blocks on the human. The sender sees no reply and no error.

Suggested fixes: document a codex rule (`~/.codex/rules/` prefix rule for `vega mux send`), or a
`[sandbox_workspace_write]` / permission profile that allows the herdr socket. Decide which is safer;
allowing the socket lets a sandboxed codex drive any pane.

## F3 Medium: raw sandbox error

See F2 output. Issues:

- The message is a Rust `Debug` string (`Os { code: 1, kind: PermissionDenied, ... }`).
- The hint `run \`vega mux\` to check the current state` points to a command that fails the same way.
- Better: code `socket_denied`, message "cannot connect to herdr socket <path>: permission denied", hint "a sandbox blocks the socket; run with escalation or allow the socket".

## F4 Medium: empty text is accepted

```
$ vega mux send bt-pi ""
sent: bt-pi
help: the reply arrives as a new prompt in this session; do not poll
exit=0
```

pi got a footer-only prompt and reasoned about it ("The message is a vega-mux footer with no body ... DONE").
It did not reply, but it spent a turn. Expected: usage error, exit 2.

## F5 Medium: `show` keeps Codex and pi chrome

`vega mux show bt-codex` (8 of 8 lines are chrome):

```
recent[8]:
    Set up security for Daybreak mode
    Set up Advanced Account Security with a hardware security key. You can keep using
    Codex while you finish setup.
  › 1. Set up security
    Press a number to choose · esc to dismiss · type to continue
  › Ask Codex to do anything
    GPT-6.1-Sol medium · vega · main · Ready · Context 0% used
    ← for agents · ? for shortcuts
```

`vega mux show bt-pi` (tail):

```
  ──────────────────────────────────────────────────────────────────────────────────────
  ──────────────────────────────────────────────────────────────────────────────────────
  ~/src/durandom/vega (main)
  $0.000 (sub) 0.0%/500k (auto)                                  (xai) grok-4.7 • medium
  ◆ 1 checkpoint
```

On a fresh pi, all 24 lines were the extension list, the "Update Available" box and package update notices.

Strip candidates:

- Codex: `› Ask Codex to do anything`, the status line (`<model> · <dir> · <branch> · Ready|Working ...`), `← for agents · ? for shortcuts`, `? for shortcuts`, `Tip: ...`, popups such as "Set up security".
- pi: runs of `─` lines, the `<cwd> (<branch>)` line, the cost/context line (`$0.000 (sub) ...`), `◆ N checkpoint(s)`, update boxes.

## F6 Low: titles for codex and pi are terminal titles

```
w3V:pP,idle,~/src/durandom/vega,vega
w3V:pQ,idle,~/src/durandom/vega,π - vega
w3V:pP,blocked,~/src/durandom/vega,[ ! ] Action Required | vega
```

They say nothing about the task. Options: strip the `π - ` prefix and `[ ! ] Action Required | `, and show `-` when the
title equals the cwd basename.

## F7 Low: empty target

```
$ vega mux send "" "x"
error{code,message,hint}:
  agent_not_found,agent target  not found,run `vega mux` to list agents; the target pane must host a running agent
exit=1
```

Expected: usage error, exit 2.

## F8 Low (external): codex popup while idle

The "Set up security for Daybreak mode" popup stays after every turn while herdr reports `idle`.
Typed text dismisses it ("type to continue"), so delivery worked in all tests.
A message that starts with a digit could select `1. Set up security`. Not tested.

## F9 Low (external): `.pi-glla/` in the repo

After the pi session, `git status` shows `?? .pi-glla/`. It comes from pi's `glla` extension. Not deleted; add to
`.gitignore` or remove it.

## Passing tests

### List and show

```
$ vega mux
agents[4]{pane,status,cwd,title}:
  w3V:p1,idle,~/src/durandom/vega,Eigene Prinzipien für agendischer Workflow
  w3V:p6,idle,~/src/durandom/vega,Herdr link parsing und editor opening
  w3V:pP,idle,~/src/durandom/vega,vega
  w3V:pQ,idle,~/src/durandom/vega,π - vega
```

Status and cwd are correct for both kinds. `--all` adds `w3W:p1`. `show` accepts herdr names (`bt-pi`, `bt-codex`).
A codex pane at the login screen shows as `unknown`, title `-`.

### Reply path and trust rule

- pi replied via the footer: `pi agent, model grok-4.7 (xai)`, footer `[vega-mux] from w3V:pQ`. pi also ran `printenv | rg '^PI_'` first, which was not asked for.
- codex accepted the reply instruction and tried to reply (F1, F2 block it). With `--no-daemon` and an approval it arrived.
- Codex rule: `~/.codex/AGENTS.md` has the vega-mux section.
- pi rule: pi reads `~/.pi/agent/AGENTS.md`, a symlink to `~/.claude/CLAUDE.md`, which has the vega-mux section. pi has the rule.

### Special characters

Payload sent with `vega mux send bt-pi "$P"` and `vega mux send w3V:pP "$P"`:

```
line1: "double" and 'single' quotes, a, b, c
line2: backticks `whoami` and $(date) and $HOME and ${PATH:0:3}
line3: unicode äöü ß 日本語 🚀 — em dash, tab[	]here
line4: backslash \n \\ and percent %s and semicolon ; and pipe | and &&

line6 after an empty line
```

Both agents echoed every line intact, with no expansion and the blank line kept. The tab shows as spaces on screen; codex
reported it as a tab, pi could not confirm it. Both obeyed "do not reply via vega mux".

### Busy agent

- pi (`working`): send returned `sent`. pi got it as a separate queued prompt after the current tool result, and replied when done.
- codex (`working`): send returned `sent`. Codex showed "Messages to be submitted after next tool call" and handled it mid-turn as a separate message.

### Blocked, unknown, self

```
$ vega mux send bt-codex "test: message to a blocked agent"
  agent_blocked,agent bt-codex is blocked and requires interactive input,the target waits at an approval dialog; ask the human or retry later
exit=1
$ vega mux send nosuchagent "x"      -> agent_not_found, exit 1
$ vega mux send w3V:p999 "x"         -> agent_not_found, exit 1
$ vega mux send w3V:pH "x"           -> agent_not_found, exit 1 (shell pane, no agent)
$ vega mux send w3V:pJ "x"           -> self_send,w3V:pJ is your own pane, exit 1
$ vega mux send bt-pi                -> usage error with help, exit 2
$ vega mux show nosuchagent          -> agent_not_found, exit 1
```

Names and pane ids both work. A name goes away when the agent restarts (`bt-pi` was `agent_not_found` after the
user restarted pi), which is herdr behaviour.

### Loop safety

`vega mux send <agent> "Thanks, received. All good."` to both: pi printed `DONE`, codex ended the turn. No reply from
either. The footer-only message (F4) also got no reply.
