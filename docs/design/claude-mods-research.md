# Claude Code mods and hooks instead of screen scraping

Status: research note (scout, no code changed). Build examined: Claude Code 2.1.322 (the mod types shipped
with this build, `plugin-authoring` skill) plus the public docs. Everything marked "not verified" was not run;
no mod was built or loaded.

## Summary and recommendation

vegad reads Claude panes as text because it has no other channel into the process. Claude Code has two:
settings hooks (stable, documented, one shell command per event) and mods (in-process JS/TS, early access,
API "moves between releases"). Neither is used by vega for state today; the existing `vega-hooks@vega-local`
plugin only carries the AskUserQuestion PreToolUse hook (plugins/mux/src/hook.rs).

Ranked recommendation:

1. **Ship a `vega-state` settings hook set first** (stable, no mod). Hooks `SessionStart`, `UserPromptSubmit`,
   `Stop`, `Notification`, `PermissionRequest`, `PreToolUse(AskUserQuestion)`, `SessionEnd` each write one line
   to a vega-owned state file or socket (pane, event, ts, `last_assistant_message`). Gives vegad a second,
   Claude-native source for turn state and pending question, independent of herdr-core. Replaces
   `read_turn_end` question parsing. Fits the existing `claude.hooks` unit.
2. **Deliver mail without typing via hooks**: `UserPromptSubmit`/`PostToolUse` `additionalContext` ("3 new
   messages, run `vega post inbox`") and `Stop` with `decision: block` + `reason` to continue a finished turn
   with the mail. `asyncRewake` wakes an idle session. This removes `hc send` for Claude and the slash-menu
   problem, and is the answer to the polling cost (push, not poll).
3. **Then a small mod `vega-claude`** for what hooks cannot do: `prompt.suggest` hook (deny the engine's own
   dim suggestions at the source), `$.prompt.read()` (true draft text), `$.prompt.submit` (a turn from a
   plugin, no typing), a status-line/band showing inbox count. Highest payoff for the input-box pains, but
   early-access API and a bigger trust/maintenance surface. Build only after 1 and 2 prove the state channel.
4. Keep screen parsing as the fallback for Codex, pi, startup dialogs (trust/update) and hosts without the
   hooks, but stop adding Claude-specific parsing.

Cheapest immediate fix for pain 1, no mod at all: `CLAUDE_CODE_ENABLE_PROMPT_SUGGESTION=false` (or
`promptSuggestionEnabled`) for every vega-launched Claude. `plugins/mux/src/main.rs:1186,2471` already sets it
for spawned agents; adopted/UAC panes are not covered (to check, not verified).

## Capabilities

### Hooks (docs: https://code.claude.com/docs/en/hooks.md)

- 33 events, incl. SessionStart (`source`), UserPromptSubmit (`prompt`), Stop (`last_assistant_message`,
  `stop_hook_active`, `background_tasks`), Notification (`message`, `notification_type`), PreToolUse /
  PostToolUse (`tool_name`, `tool_input`), PermissionRequest, SubagentStart/Stop, PreCompact, SessionEnd.
- Output: `additionalContext` (UserPromptSubmit, SessionStart, PostToolUse, Stop, SubagentStop; 10,000 chars max);
  Stop `decision: "block"` + `reason` continues the turn with `reason` as instruction (max 8 in a row,
  `CLAUDE_CODE_STOP_HOOK_BLOCK_CAP`, check `stop_hook_active`); exit 2 blocks where supported.
- `asyncRewake: true` + exit 2 wakes an idle session; stderr arrives as a system reminder (docs, hooks page).
- No hook event for "idle" (only `TeammateIdle` for agent teams), none for the input box or a prompt suggestion.
  A `Notification` `idle_prompt` matcher was seen in an excerpt but is not confirmed.
- Channels (MCP push into a running session, https://code.claude.com/docs/en/channels.md): research preview,
  needs claude.ai/Console auth, `--channels` flag, org opt-in on Team/Enterprise. Too restrictive; rejected.

### Mods (docs: https://code.claude.com/docs/en/plugins/mods/overview.md; types in the build)

Hooks run in-process as `($, e, next)`. From the 2.1.322 types (`claude-code.d.ts`):

- `prompt.suggest` event: input has `origin: {kind:'suggestion'}` for the engine's own guess; a hook answering
  `{ isShown: false }` without `next` suppresses it (line ~10050-10085). `$.prompt.suggest()` can also set one.
- `$.prompt.read()` → `{ text, cursor }`, the real draft (line 3131). `''` when headless. Events
  `prompt.change`/`prompt.edit` fire on edits. (The public docs agent found no getter; the build's types have it.)
- `$.prompt.submit({text})`: "a turn of its own, once the session is idle" (line 3108); `$.prompt.fill` writes
  into the box (modes replace/append/insert; `isFilled:false` under a dialog).
- `prompt.submit` hook can add `context` to the user's next prompt (no extra turn).
- `turn.start/step/complete`, `session.start/end/receive`, `tool.call/check`, `command.run` (slash commands via
  `$.command.register`), `config.set` (key hookable, e.g. `theme`), `classic.<Event>` for every settings hook.
- Output: `ui.render` on `AbovePrompt` (band), `Pane`, `ToolUse`; `$.ui.status(text)` status line; `$.ui.toast`.
- I/O: `$.fs` read/write, `$.process.run`/`spawn` (argv, no shell), `$.http.fetch(url, {socketPath})` over a
  Unix socket, `$.clock.every`, `$.store` (across sessions), `$.state`. `$.tool.register` lets the model call a
  tool, e.g. a `vega_inbox` tool.
- Limits: a hook has a time budget per dispatch; long work starts from `session.start` with `$.clock`. No DOM/Node.
  `import()` is refused. Hot reload only for the session mods folder; an installed plugin loads at start.

## Findings per pain

### 1. Input box empty or suggestion shown (delivery.rs `draft()` :663, `input_box` :692, theme.rs)

- Mechanism A (hook set): none. No settings hook sees the box. Disable suggestions by env/setting (see above).
- Mechanism B (mod): `prompt.change` + `$.prompt.read()` publish `{text, cursor, ts}` on every change to a file
  or Unix socket; the dim suggestion is not part of `text` (it is not a draft), so the suggestion/draft ambiguity
  disappears. `prompt.suggest` hook can also just suppress suggestions in all vega panes, mod-wide.
- Failure modes: mod not loaded (old build, hooks disabled by `disableAllHooks`, org policy) → vegad must know
  and fall back to the screen; stale state if the process dies (needs a heartbeat/`session.end`); `read()`
  returns `''` headless.
- Confidence: high that the API exists on 2.1.322 (types); medium that it is stable; draft state with a
  *dialog open* (box unmounted) not verified.

### 2. Turn state missing for a pane (ganymede/t1z, 30-60 min wait)

- Mechanism: own hooks `SessionStart` → idle, `UserPromptSubmit` → working, `Stop`/`StopFailure` → idle,
  `Notification`/`PermissionRequest` → asking, `SessionEnd` → gone. Written by vega, so vega controls the
  failure handling and can add a "no event for N min while the process lives" alarm without parsing the screen.
- Why the pane never reported is unknown (I did not find the cause). A second source helps only if the cause was
  herdr-core's hook plumbing and not Claude's hook execution; hooks held back by an unaccepted workspace trust
  dialog (docs: interactive sessions hold all hooks until trust is accepted) would silence both.
- Mod variant: `turn.start`/`turn.complete` give the same, plus `reason` (answer/aborted/refusal/error).
- Confidence: high for hooks; the t1z cause: I don't know.

### 3. Question and dialog detection (supervise.rs `read_turn_end` :253, dialog.rs `classify` :44)

- Question asked by the model in text: `Stop` hook input carries `last_assistant_message`; vega applies its
  question heuristic to that string instead of the screen (no redraw race, no `✻`/`◐` footer filtering).
- AskUserQuestion: already a PreToolUse hook (mux/src/hook.rs); extend it to write a record instead of only
  denying.
- Permission dialogs: `PermissionRequest` hook (input has `permission_suggestions`) / `Notification`
  (`notification_type`); mod `tool.check`. Gives kind and tool, not the on-screen option indexes.
- Not covered: trust dialog, update dialog, "Hooks need review" (they appear before any hook runs). Those stay
  screen-parsed (dialog.rs kinds Trust, HooksTrust, Update).
- Confidence: medium-high; the exact `notification_type` values were not enumerated.

### 4. Delivery without typing (hc send; theme `/config theme=<mode>` needs 2nd Enter)

- Mail pointer: three options, best first.
  1. Hook `additionalContext` on the agent's next `UserPromptSubmit`/`PostToolUse`: zero typing, free if the
     agent is already working. Cannot reach an idle agent.
  2. `Stop` hook returning `decision: block, reason: "Mail: N new. Run vega post inbox"` when mail is pending:
     the turn continues with the reason as instruction. Fires only at a turn end, capped 8 in a row.
  3. Idle agent: `asyncRewake` hook (a background command exiting 2 when mail arrives) or mod
     `$.prompt.submit` from a `$.clock.every`/socket watcher. Both start a turn without touching the box.
- Open: does `$.prompt.submit` run while the user has a draft in the box? Types say "once the session is idle",
  nothing on drafts. Must be tested; vega must never clobber a user draft.
- Theme: a mod cannot be assumed to set the CLI theme from outside. `$.theme.set` styles the UI (backdrop,
  attributes) and the `config.set` event is hookable (`key: "theme"`); I did not find a documented setter for
  the `/config` theme value. I don't know. Keep the typed command but only when the pane is idle (state from
  pain 2), which already avoids the completion-menu problem.
- Confidence: medium; needs a spike for `asyncRewake` on an idle pane and for `$.prompt.submit` with a draft.

### 5. Token cost of polling (spellkave ledger/designs/agent-token-efficiency-2026-11.md)

The audit: waiting/polling is 12-30% of long sessions; a no-op inbox check costs a full turn ($0.03-$0.20).
Hooks turn the poll into a push: an agent with nothing to read gets no prompt; with mail it gets one line via
`additionalContext`. Caveat: this removes the *empty* checks only if agents are told not to poll; the hook does
not stop an agent that decides to poll by itself, so the vega skill must say "mail arrives by itself".

## Showing vega state inside Claude (question 3)

- Statusline (settings `statusLine`, docs statusline.md): command gets JSON (session, cwd, model, context,
  rate limits); can print anything from a shell (`vega post inbox --count`), runs after each assistant message
  and every `refreshInterval` s (min 1). No input-box state. Stable, no mod.
- Mod: `$.ui.status`, `AbovePrompt` band, `Pane` (live panel) driven by `$.clock.every` over a socket. Richer
  (inbox count, waiting children, capacity), but only a human looks at it; agents read `additionalContext`.
- Recommendation: statusline for humans now; context line for agents via hooks.

## Shipping and updating

- Today: `claude.hooks` unit installs the local marketplace `~/.local/share/vega/claude-marketplace/`
  (plugin `vega-hooks@vega-local`) through `claude plugin install/update` (docs/design/config.md:36,196-201;
  plugins/config/src/claude_plugin.rs:6). Hooks and a mod are both plugin contents, so one plugin can carry
  `hooks/hooks.json` with command hooks plus `modules` (the mod format uses the same `hooks/hooks.json`).
  Whether an *installed* marketplace plugin may carry a mod module: the skill says folders "not hot-reloaded"
  are still loaded, I did not verify installed-plugin mods. Test first.
- Rollout: `vega deploy apply <host>` already runs `claude.hooks`; add a version-pinned `vega-hooks` bump and
  a per-host check (`claude plugin list --json`, as config.md:208 does). vegad must read a per-pane
  "state source: hooks|mod|screen" so it can fall back.
- Harness versions: mods need Claude Code ≥ 2.1.287 (docs: on by default from there; the build here is
  2.1.322). Hooks work on all current builds; fields are added over time (docs give per-field version gates).
  Gate on `claude --version` in the unit.
- Codex: has hooks (vega already installs `codex.hooks`; "Hooks need review" trust per command). No known mod or
  input-box API; I don't know whether Codex hooks include a Stop/UserPromptSubmit with context injection. Check.
- pi: I don't know. pi-tui's editor exists (delivery.rs:672 comment); an extension API was not researched.

## Risks

- **Trust prompts**: interactive sessions hold *all* hooks until the workspace trust dialog is accepted; a
  hooks-trust dialog may also appear for changed hooks (as for Codex). A new pane in a new directory has no
  state until trust is accepted: same blind spot as screen reading for dialogs, so keep dialog parsing.
- **Security**: state files must carry no secrets and no prompt text beyond what vega needs. The Stop hook sees
  `last_assistant_message` (may contain secrets): store only the question flag/first line, 0600, local only.
  Never write into the box while `$.prompt.read().text` is non-empty. `$.http.fetch` goes "through the host"
  and can be blocked by org web-fetch policy; prefer file or `$.process.run`.
- **Hook loop risk**: `Stop` block must check `stop_hook_active` and cap per mail, else a runaway turn.
- **Claude updates**: hook JSON is versioned per field, with no general stability promise (docs); mod API is
  declared early access. Mitigation: hooks first, mod optional; a startup self-test reports "hooks alive" to
  vegad; version the state records; fail open (no output = old behavior, as hook.rs does).
- **Managed environments**: `disableAllHooks`, `allowManagedHooksOnly` switch vega hooks off silently; vegad
  must treat "no events" as unknown, not idle.

## Minimal first step (one bead, small)

`vega-hooks` gains a `vega mux hook state` subcommand (same binary and crate as hook.rs) bound to
SessionStart, UserPromptSubmit, Stop, Notification, SessionEnd. It appends one JSON line per event to
`$XDG_STATE_HOME/vega/pane-state/<pane>.jsonl` (pane from `own_terminal()`, as hook.rs does) and prints
nothing. vegad `supervise` reads the file next to the hc status and logs disagreements for a week (shadow mode),
no behavior change. Success: the t1z case (no status for 5 min) shows hook events or proves the hooks do not
run. Then step two: `Stop`/`PostToolUse` context line for pending mail, behind a flag per host.

## Open questions

1. Why did ganymede/t1z report no status: hooks not run (trust dialog?), or herdr-core plumbing? Needs the pane's
   debug log.
2. `$.prompt.submit` and `asyncRewake` against a non-empty box: clobber or queue? Needs a spike.
3. Can an installed marketplace plugin carry a mod module, and does it load without the "turn on mods" question?
4. Is there a documented way to set the `/config` theme from a mod or hook (replacing the typed command)?
5. Do prompt suggestions stay on in UAC/adopted panes today (env set only at spawn)?
6. Codex hook events and pi extension API: not researched in depth.
7. Is the mod API stable enough (early access)? Anthropic gives no promise; decide how much to bet.
