# Pane control: steering TUI panes through herdr

Date: 2026-10-09. Scout for vega-lhc.36; feeds vega-lhc.37 (dialog kind in `uac attention`) and vega-lhc.38
(`vega mux answer`). Report only: no product code.

Versions: herdr 0.9.3 (protocol 22, schema_version 1), btop 1.4.6, Claude Code 2.1.295, codex-cli 0.162.0.
Method: a key-echo program (raw tty, logs every `read()` as hex) in a split pane, then btop, then throwaway
Claude (haiku) and Codex panes in `mktemp -d` dirs. Every pane opened was closed; no trust or permission prompt was
answered.

## Summary

| # | Finding | Impact |
|---|---------|--------|
| K1 | Letters, ctrl, alt, arrows, F1–F12, tab/shift+tab, esc, enter, backspace arrive correct | Steering most TUIs works |
| K2 | `home end pageup pagedown insert delete` are rejected (`invalid_key`) under every spelling | Use `send-text` with raw escapes |
| K3 | herdr follows the app's keyboard mode: kitty protocol (`CSI >1u`) changes `esc`, `ctrl+a`, `shift+enter` | Do not hard-code byte tables |
| K4 | `send-text` writes raw bytes, no bracketed paste; socket `pane.send_input` honours bracketed paste | Text for agents: `agent prompt` or `send_input` |
| K5 | btop drops keys after the first when several letters arrive in one `send-keys` call | One key per call for TUIs |
| R1 | `read --source visible` returns the composited screen, overlays included, alt screen included | btop menus read back fine |
| R2 | `recent-unwrapped --lines N` counts physical rows, so the first logical line can be cut | Ask for more lines than needed |
| R3 | `--format ansi` keeps SGR (256 and truecolor); text mode keeps the `❯`/`›` selection markers | Selection is readable without ansi |
| R4 | `revision` in `pane.read` stayed 0; it is no change counter | Poll text, not revision |
| L1 | `send-keys` CLI call ~30 ms; bytes reach the app in ~7 ms; next read already shows the echo | No sleep needed for fast apps |
| D1 | herdr marks every dialog `blocked` except Codex hooks review (`unknown`); it does not say which kind | vega must classify by text |
| D2 | Codex trust preselects "Trust and continue"; Codex permission takes `y`/`p` accelerators; Claude digits pick | A stray key can approve |
| A1 | `agent wait` right after a pane starts: `agent_not_found`; right after Enter: stale `blocked` | Poll `pane get` until the state moves |

## Inputs

`herdr pane send-keys <pane> <key>...` validates all keys first, then writes. Bytes received in a plain raw tty:

| Key | Bytes | Key | Bytes |
|-----|-------|-----|-------|
| `enter`, `return`, `ctrl+m` | `0d` | `up down right left` | `1b5b41..44` |
| `tab`, `ctrl+i` | `09` | `shift+up` | `1b5b313b3241` |
| `shift+tab` | `1b5b5a` | `ctrl+up`, `ctrl+left` | `1b5b313b35 41/44` |
| `esc`, `escape`, `ctrl+[` | `1b` | `alt+left` | `1b5b313b3344` |
| `backspace`, `bs` | `7f` | `f1`–`f4` | `1b4f50..53` (SS3) |
| `ctrl+backspace`, `ctrl+h` | `08` | `f5 f6 f10 f11 f12` | `1b5b 15~ 17~ 21~ 23~ 24~` |
| `alt+backspace` | `1b7f` | `ctrl+f1`, `shift+f1`, `alt+f1` | `1b5b313b 35/32/33 50` |
| `ctrl+a` … `ctrl+z` | `01` … `1a` | `ctrl+space`, `ctrl+@` | `00` |
| `ctrl+\ ctrl+] ctrl+/` | `1c 1d 1f` | `alt+a`, `meta+a`, `ALT+a` | `1b61` |
| `alt+enter` | `1b0d` | `ctrl+alt+a` | `1b01` |
| `alt+A`, `alt+shift+a` | `1b41` | `ctrl+shift+a`, `ctrl+A` | `1b5b39373b3675` (CSI u) |
| `super+a`, `cmd+a` | `1b5b39373b3975` (CSI u) | `ctrl+shift+tab` | `1b5b32373b363b397e` |
| `a`, `A`, `ä` | `61`, `41`, `c3a4` | `ctrl+alt+shift+x` | `1b5b3132303b3875` |

Silent or lossy (no error, no or wrong bytes): `shift+enter` and `ctrl+enter` send plain `0d`; `shift+space` is
`20`; `ctrl+1` is `31`; `shift+1` is `31` (not `!`); `f13`, `f20`, `hyper+a` send nothing.

Rejected with `invalid_key`: `home end pageup pagedown insert delete` and every variant tried (`Home`, `pgup`,
`page_up`, `PageUp`, `del`, `Delete`, `ins`), `backtab`, `btab`, `cr`, `kp_enter`, tmux-style `C-a`, `M-a`, `^a`.
Workaround: `herdr pane send-text <pane> $'\e[H'` (Home), `\e[F` (End), `\e[5~`/`\e[6~` (PgUp/PgDn), `\e[3~`
(Delete), `\e[2~` (Insert). These arrive byte-exact.

Keyboard modes. With the app in kitty keyboard mode (`CSI >1u`) and bracketed paste (`CSI ?2004h`), herdr
re-encodes: `esc` → `CSI 27u`, `ctrl+a` → `CSI 97;5u`, `alt+a` → `CSI 97;3u`, `shift+enter` → `CSI 13;2u`,
`ctrl+enter` → `CSI 13;5u`, `f1` → `CSI P`. Arrows and `enter` stay legacy. Consequence: a key such as `ctrl+]`
meant as a raw `1d` no longer is one; use `send-text $'\x1d'` for raw bytes.

Text. `send-text` writes the bytes as given: no bracketed paste even when the app asked for it; `\n` stays `0a`;
UTF-8 is kept; 3000 bytes arrived intact in three reads (1022 + 1022 + 956). The socket method `pane.send_input`
(`{"pane_id", "text", "keys": [...]}`, not exposed in the CLI) sends text then keys in one call and wraps the text
in `CSI 200~ … CSI 201~` when the app enabled bracketed paste. `herdr agent prompt` does the same for agents.

Multiple keys per call. `send-keys a b enter up` arrives as separate reads in the echo program, and btop took
`left left` correctly, but btop's filter input kept only `b` of `send-keys b t o p`. One key per CLI call (about
30 ms apart) worked every time. Rule for TUIs: one key per call, or a short gap between keys.

## Reading state

Sources (`herdr pane read <pane> --source …`):

- `visible`: the rendered viewport. Works for alternate-screen apps; btop's full layout, popups (menu, options,
  help) and their overlap with the boxes underneath read back as rendered.
- `recent`: viewport plus host scrollback, soft-wrapped rows as they appear. Alternate-screen frames never enter
  scrollback: after quitting btop, 300 recent lines held no btop frame.
- `recent-unwrapped`: soft wraps joined. `--lines` counts physical rows before joining: with a 200-char line wrapped
  over 3 rows, `--lines 6` returned the last 30 chars, `--lines 10` the full line.
- `detection`: herdr's bottom-buffer snapshot used for agent detection.

Formats: text strips SGR and keeps `\r`-free lines; `--format ansi` keeps `38;5;N`, `48;5;N`, `38;2;r;g;b`, bold.
`--raw` keeps a trailing `\r` per row. Selection in Claude, Codex and btop is also marked in plain text (`❯`, `›`,
`← value →`), so ansi is only needed where colour alone carries state.

The CLI prints the text; the socket's `pane.read` returns JSON with `revision` and `truncated`. `revision` stayed 0
across changes for a plain shell pane, so it cannot detect change. `pane layout` without `--pane` reports the
focused client's tab, not the caller's; its `rect` (60×40) did not match the pane's pty (85 columns, 76 rows).

Latency (15 runs, key-echo app): `send-keys` call 25–41 ms, delivery to the app ~7 ms after the call starts, and the
very next `read` showed the echo every time (15/15). 20 `read --source visible` calls took 0.14 s (7 ms each).
btop: 7 sort changes (`right`), each visible on the first read, ~50 ms after sending. Slower TUIs need a
`wait-output --source visible --regex … --timeout` instead of a fixed sleep; `wait-output` checks the current
snapshot first, so it can match stale text: wait for the new state, never for text that may already be there.

## btop transcript

btop in a split pane (`herdr pane split --direction right --no-focus`, then `herdr pane run <pane> btop`). Each step
sends keys, sleeps 0.3–0.8 s, greps `read --source visible`:

```text
sort right        (right)        → ← pid →
sort right        (right)        → ← name →
sort left x2      (left left)    → ← cpu lazy →
tree (e)                         → "Tree:" header, rows ├─11017, ├─39546 …
tree off (e)                     → tree rows gone
interval + (+)                   → 2100ms
interval - (-)                   → 2000ms
preset (p)                       → preset 0
preset back (P)                  → preset 3 (cycles backwards)
toggle cpu box (1) twice         → ¹cpu gone, then back
menu (m)                         → BTOP logo + Options/Help/Quit drawn over the proc box
options (o)                      → "Color theme 1/38 ← Default →", setting list with values
options down down, right         → "Truecolor ← False →" (changed a setting, see below)
help (f1)                        → "Key: Description" table
filter (f), then b,t,o,p one key per call, enter
                                 → ⁴proc┌┐f btop del, rows filtered to "60947 btop mhild"
q                                → shell prompt back
```

Values read back reliably: header (`- 2000ms +`, `BAT▼ 80%`, clock), per-core `C0 … 54%`, `Load avg: …`, memory
`Used:──53.7─GiB`, process rows `Pid Program User MemB Cpu%`.

Lessons from the run:

- A key aimed at a widget that is not on screen acts globally. In preset 3 the proc box is hidden, `f` did nothing,
  and the following `send-text btop` ran as commands (`p` switched presets, `o` …). Always read and confirm the
  target widget before typing into it.
- TUIs persist settings. btop rewrites `~/.config/btop/btop.conf` on quit. The scout backed it up before quitting,
  reverted Truecolor by hand, and the file was byte-identical afterwards (sha1 `45d24ee7…`). A vega driver for TUIs
  must treat settings changes as side effects.

## Dialogs

herdr already detects dialogs: `agent_status` becomes `blocked`, and `herdr agent explain <pane>` names the
manifest rule (`~/.local/state/herdr/agent-detection/remote/<agent>.toml`). It does not say which kind of dialog;
Claude's trust prompt and AskUserQuestion fire the same rule (`live_blocked_form`). vega classifies by text.

| Kind | Agent | herdr status / rule | Detect by (visible text) | Default selection |
|------|-------|---------------------|--------------------------|-------------------|
| trust | Claude | blocked / `live_blocked_form` | `Quick safety check: Is this a project you created or one you trust?` + `Yes, I trust this folder` | `❯ No, exit` |
| trust | Codex (git dir only) | blocked / `trust_directory` | `Trust this folder?` + `1. Trust and continue` + `enter continue · esc quit` | `› 1. Trust and continue` |
| hooks trust | Codex | **unknown** / none | `Hooks need review` + `2. Trust all and continue` | `› 1. Review hooks` |
| permission | Claude | blocked / `bash_permission_prompt` | `Do you want to proceed?` + `Esc to cancel · Tab to amend`; tool name above (`Bash command`) | `❯ 1. Yes` |
| permission | Codex | blocked / `osc_title_blocked` | `Would you like to run the following command?` + `Press enter to confirm or esc to cancel` | `› 1. Yes, proceed (y)` |
| question | Claude AskUserQuestion | blocked / `live_blocked_form` | tab bar `←  ☐ Colour  ☐ Pets  ✔ Submit  →` + `Enter to select · Tab/Arrow keys to navigate · Esc to cancel` | `❯ 1. <first option>` |
| question | Codex request_user_input (Plan mode) | blocked / `osc_title_blocked` | `Question 1/1 (1 unanswered)` + `tab to add notes \| enter to submit answer` | `› 1. <first option>` |

Not provoked: Claude MCP elicitation (`MCP server "…" requests your input`), Claude network permission (`Do you want
to allow this connection?`), plan approval; herdr's manifest has rules for them. Codex in a non-git temp dir showed
no trust prompt, only the inline "Set up security for Daybreak mode" nudge while herdr reported `idle`
(battle test F8).

Captured screens (visible text, chrome trimmed):

```text
Claude trust                                  Codex trust
 Accessing workspace:                           Folder access
 /private/var/folders/…/vega-pane-scout.jOj4…   /private/var/folders/…/vega-pane-scout-cxg.pfCS…
 Quick safety check: Is this a project you      Trust this folder? Codex can read, edit, and run
 created or one you trust? …                    files here, … Your trust decision will be saved.
 ❯ No, exit                                     › 1. Trust and continue
   Yes, I trust this folder                       2. Quit
 Enter to confirm · Esc to cancel               enter continue · esc quit

Claude permission                             Codex permission
 Bash command                                   Would you like to run the following command?
 Fetch HTTP headers from example.com            Environment: local
 curl -sI https://example.com                   Reason: Allow running touch probe.txt outside the sandbox?
 This command requires approval                 $ touch probe.txt
 Do you want to proceed?                        › 1. Yes, proceed (y)
 ❯ 1. Yes                                         2. Yes, and don't ask again for commands that start with … (p)
   2. Yes, and don't ask again for: curl *        3. No, and tell Codex what to do differently (esc)
   3. Yes, and switch to auto mode …            Press enter to confirm or esc to cancel
   4. No
 Esc to cancel · Tab to amend

Claude AskUserQuestion (multi-question)       Codex request_user_input
←  ☐ Colour  ☐ Pets  ✔ Submit  →                Question 1/1 (1 unanswered)
Which colour do you prefer?                     Which colour do you prefer?
❯ 1. Red                                        › 1. Red                Choose Red.
     Red                                          2. Blue               Choose Blue.
  2. Blue                                         3. None of the above  Optionally, add details in notes (tab)
  3. Type something.                            tab to add notes | enter to submit answer | esc to interrupt
  4. Chat about this
Enter to select · Tab/Arrow keys to navigate · Esc to cancel
```

Answering question dialogs (done end to end; both agents echoed the answer):

- Claude: `down` moves `❯`; `enter` picks and advances to the next tab; in a multiSelect tab digits toggle
  (`1`, `3` → `[✔] Cats`, `[✔] Fish`); `right` opens "Review your answers" (`→ Blue`, `→ Cats, Fish`) with
  `❯ 1. Submit answers`; `enter` submits. Transcript: `User answered Claude's questions: · Which colour … → Blue`.
- Codex: `down` then `enter` submits. Transcript: `• Questions 1/1 answered … answer: Blue`.
- The review screen is the safe checkpoint: read it and compare with the intended answers before `enter`.

Hazards for answering:

- Codex trust and Claude permission preselect the approving option: any stray `enter` approves. Codex permission
  also approves on a bare `y` or `p`; Claude dialogs pick on a bare digit. Text sent to a blocked agent with
  `send-text` is therefore never safe. `herdr agent prompt` refuses blocked agents (`agent_blocked`); raw
  `pane send-keys`/`send-text` do not.
- Codex hooks review is `unknown`, not `blocked`, so "is it blocked?" misses it; `agent prompt` would not refuse it.
- After `enter`, `agent wait` returned the stale `blocked` at once; the state moved to `done` ~2 s later.
- `agent wait`/`agent prompt` on a pane started a moment ago fail with `agent_not_found` until herdr detects the
  agent (~2–4 s for Claude haiku). Poll `pane get` until `agent` is set and `agent_status` is `idle`.

### Questions follow the child's mode (vega-lhc.41, vega-lhc.165)

Headed spawn and adopt footers tell the agent to ask the user in its pane. With `--headless`,
questions and decisions go to the parent by `vega post send`. An inbox-only host cannot deliver mail
to the parent; that target's footer instead tells the child to ask the user in the pane, even with
`--headless`. A PreToolUse hook
enforces it: `vega-mux hook ask-parent` denies AskUserQuestion (Claude) when the headless pane has a `parents` record for the
agent that runs there, and the deny reason names `vega post send [--host <h>] <parent> "…"` plus the questions it
tried to ask. Without a record it prints nothing, so sessions the user drives keep their dialogs. Any error also
prints nothing (fail open).

Headed panes (vega-lhc.61): a `modes` record (`{"name", "mode"}`, same agent-name check as `parents`) marks a child
headless (an automated sentinel) or headed (the owner works in it). The hook denies only headless children with a
parent; no record, another agent's record or an unknown mode count as headless (the old behaviour). `vega mux spawn`
and `vega uac adopt` write headed unless `--headless`. Spawn records the choice before startup, even without a
parent or when a dialog stops startup; `spawn --resume` preserves it. Missing records keep older children headless.
`vega mux mode <pane> [headed|headless]` flips or prints it; `vega mux show` and `vega uac` show it.
Headed asking, silent and input attentions are for the user and do not wake the parent;
exited, failure, quota, completed and decided still do.

`vega config apply hooks` (run by `scripts/install-cli.sh`; `vega mux hook install` is an alias) adds the entry to
`~/.claude/settings.json` and `~/.codex/hooks.json` (matcher `request_user_input|request_user_input_async`), replaces an older entry of its
own, keeps all other hooks and key order, and copies the file to `<file>.vega-bak` before a change
([config.md](config.md), units `claude.hooks`, `codex.hooks`); see Limits for codex.

Tested 2026-10-09 with Claude Code 2.1.295 (haiku): a spawned sentinel got `PreToolUse:AskUserQuestion hook error:
You were spawned by …` and sent the question by `vega post send`; a session without a record showed the dialog.

Limits:

- Parent records exist only on the spawning machine: a child spawned with `--machine` (or adopted across
  machines) has no record on its own host, so the hook does not fire there; the footer text still applies.
- Codex: a new or changed entry in `~/.codex/hooks.json` makes every new codex session stop at "Hooks need
  review" until the user trusts it (trust hashes the command and the matcher, so again after either changes);
  `vega mux spawn` then fails with `hooks_review_required`, in every project on the host. Codex rust-v0.163.0
  source shows PreToolUse runs for both question tools through the generic function-tool path (see "Codex async
  questions"); the deny and the copy are tested with hook payloads, not yet end to end in a codex session. To
  check: `vega config apply codex.hooks`, trust it once, spawn a codex child and ask it to use
  request_user_input_async; headless, it gets the deny and mails the parent; headed, the bar stays and the parent
  gets the copy.

### Codex async questions (vega-lhc.190)

Codex ≥ 0.160 has `request_user_input_async` (`questions: [{title, options?: [string]}]`), offered when the model
catalog lists it, in any mode. Read from codex rust-v0.163.0:

- The handler (`core/src/tools/handlers/request_user_input_async.rs`) emits an `AgentMessage` with `delivery:
  async` and the questions, returns `{"accepted":true}` and the turn goes on. It runs through the generic
  function-tool path (`core/src/tools/registry.rs`), so PreToolUse sees it: `tool_name` is the plain name,
  `tool_input` the parsed arguments, and a deny reaches the model as the tool's error text, like any tool.
- Matchers made of `[A-Za-z0-9_|]` match exact names split on `|` (`hooks/src/engine/matcher.rs`), so the
  sync matcher `request_user_input` does not catch the async tool. The trust hash covers the matcher
  (`hooks/src/engine/discovery.rs` `hook_hash`): changing it brings "Hooks need review" back once per host.
- The TUI queues the questions in a bar ("N questions, shift+← to answer"). While one is unanswered the
  terminal title shows action required, which herdr reports as `blocked` (vega: `input`), for the whole running
  turn. A question resolves when the user answers in the bar, when a committed user message is exactly the
  reply envelope `<send_user_message_question_reply>[{"questionItemId": "[\"request_user_input_async\",
  \"<call id>\",<index>]", …}]</send_user_message_question_reply>`, or when the turn ends (completed, interrupted
  or failed: `take_question_drafts` clears all pending questions and moves typed drafts into the composer).
- While `blocked`, `herdr agent prompt` refuses (`agent_blocked`) and post mail waits, so a `vega mux send` answer
  reaches the model only after the turn; the bar would need raw `send-text`, which this doc forbids for blocked
  panes. Hence the bead's "stays input after answering".

Route (simplest that keeps the rules above):

- Codex's ask-parent matcher becomes `request_user_input|request_user_input_async`; the hook reads both
  question shapes (`question` + `{label}` options, `title` + string options).
- Headless child with a parent: deny, as for the sync tool. The reason names `vega post send <parent> "…"` and
  the questions, and says to go on with work that does not depend on the answer (the tool is non-blocking). No
  question is queued, so the pane never turns `blocked`; the parent answers with the mail reply command, which
  arrives as a new prompt.
- Headed child with a parent: allow (the user sees the bar), and the hook mails the parent a copy by
  `vega post send` from the child: the questions, that the user may answer in the pane, and the reply command
  `vega post send <child> "<answer>"`, delivered when the child's turn ends. Its `blocked` stays a headed
  `input` (owner attention, the parent is not woken) and clears at turn end, as Codex clears the bar. The copy
  is sent in the background so the tool is not delayed; any failure is silent (fail open).
- No parent: nothing, as before.

### Folder trust on spawn (vega-nic.8, vega-lhc.55)

A trust dialog is never answered by vega; spawn avoids it instead. Trust is per agent kind, so a repo the user
trusted in codex still stops a claude spawn (rally-ng, 2026-10-10). Before a local claude or codex agent starts,
`vega mux spawn` (`plugins/mux/src/trust.rs`) gives it the trust its repo already has, from either kind, and prints
a `claude trust:` or `codex trust:` line. Sources: the spawn folder, its git toplevel, the repo root (parent of the
git common dir) and the main checkout (on `main`/`master`). A repo no kind trusts still asks; codex's explicit
`untrusted` entries are kept. `--machine` spawns are skipped (the files are local).

How the agents look it up, read from Claude Code 2.1.296's bundle and tested with throwaway repos:

| Agent | Store | Lookup | vega writes |
|-------|-------|--------|-------------|
| Claude | `~/.claude.json` `projects[<path>].hasTrustDialogAccepted` | the config's project path, else cwd up to the git toplevel, never above it (a trusted `~/src/durandom` did not cover `~/src/durandom/racing/rally-ng`) | the toplevel; a minimal `{hasTrustDialogAccepted: true}` if absent |
| Codex | `~/.codex/config.toml` `[projects."<path>"] trust_level` | the repo root when it has a `.git`, else the worktree (bare `repo/.bare` layout); no parent walk (a trusted `$HOME` did not cover a repo under it) | that path, plus the main checkout's hook approvals for a bare-repo worktree |

Both files are rewritten by running agents. Claude saves under `~/.claude.json.lock` (proper-lockfile: an atomic
`mkdir`, stale after 10 s) and re-reads under it; vega takes the same lock, re-reads, sets only the flag, writes a
temp file in the same dir and renames it, then reads it back. Codex's file gets a vega lock (`config.toml.vega.lock`)
and an append unless an entry exists. Tested 2026-10-10: claude in a codex-only repo and codex in a claude-only repo
started without a dialog; an untrusted repo still asked and got no entry; three parallel claude spawns into one
codex-only repo all started and left `~/.claude.json` valid.

## Limits

- No Home/End/PgUp/PgDn/Insert/Delete key names; `shift+enter`/`ctrl+enter` degrade to Enter in legacy mode.
- Multi-key calls can lose keys in some TUIs (btop).
- No change counter on reads; state checks are text polling (7 ms per read, cheap).
- `pane.send_input` is socket-only; vega's herdr client (`crates/vega-herdr`) shells out to the CLI today.
- Dialog kind is not in herdr's output; text patterns drift with agent versions (herdr ships its manifests
  remotely, dated, e.g. `claude.toml 2026.09.11.1`, `codex.toml 2026.10.01.1`).
- Not tested: mouse input (no herdr API for it), resize races, remote machines (`--machine`), other agents.

## Proposed vega API

Keep generic TUI control thin over herdr and put the policy (what may be answered) in vega.

```text
vega mux keys <target> <key>...         one key per herdr call, 30 ms apart; extra names home end pageup
                                         pagedown insert delete mapped to raw escapes; refuses blocked
                                         agent panes unless --force (never for trust/permission kinds)
vega mux screen <target> [--ansi] [--lines N] [--since-key]
                                         visible screen; trimmed of blank rows; --lines on recent-unwrapped
                                         pads the request to avoid the cut first line
vega mux expect <target> --regex R [--timeout ms] [--changed]
                                         wait-output on visible; --changed first snapshots and only matches
                                         text that was not on screen before (avoids stale matches)
vega mux dialog <target> [--json]        kind (trust|hooks-trust|update|permission|question|none), agent, title line,
                                         options with index and selected flag, raw text; exit 0 when a
                                         dialog is present, 1 when none. Feeds uac attention (vega-lhc.37).
vega mux answer <target> --option N | --text "…" [--question K]
                                         only for kind=question (vega-lhc.38): checks the dialog again,
                                         moves with arrows to N (never digits or y/p), enter, verifies the
                                         review/next screen, refuses every other kind with exit 2.
```

Classifier for `dialog` (first match wins, on `read --source visible`, after herdr says `blocked` or `unknown`):

1. trust: `Yes, I trust this folder` | `Trust this folder?` | `Hooks need review`
2. permission: `Do you want to proceed?` | `Would you like to run the following command?` |
   `Do you want to allow this connection?` | `requests your input` (MCP elicitation)
3. question: tab bar `^←\s+[☐☒]` or `Enter to select · Tab/Arrow keys to navigate` (Claude);
   `^\s*Question \d+/\d+` (Codex)
4. otherwise `unknown` with the text, never answerable.

Options parse: lines matching `^\s*[❯›]?\s*(\d+)\.\s+(.*)`; the marked one is selected. Trust and permission go
first in the order so an ambiguous screen is never treated as a question.

Open questions for the UAC:

- Should `keys`/`screen` live in `vega mux` or a new plugin (e.g. `vega tui`) since they target non-agent panes?
- Use the socket (`pane.send_input`, JSON `pane.read`) or stay on the CLI? The CLI is enough for everything above.
- Who owns the dialog patterns: vega (own table, tests from the captured screens above) or a thin layer over
  herdr's manifests (rule name + text)?
