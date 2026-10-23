# AgentCraft: a proprietary RTS dashboard, not a vega building block

Status: research, 2026-10-15. Research only: AgentCraft was not installed, run or signed up for. The npm
tarball of `@idosal/agentcraft@0.5.1` was unpacked into a scratchpad and read as data (`LICENSE`,
`package.json`, hook scripts, minified server code); none of it was executed.

AgentCraft is a single-developer, **proprietary ("All Rights Reserved")** Node app that shows coding agents
(Claude Code, Codex, OpenCode, Cursor, OpenClaw) as Warcraft-style heroes on a 3D map in the browser. A local
server on port 2468 receives events from hooks it writes into each repo, spawns agents in node-pty terminals,
and adds a planner ("Campaigns"), loops, containers, a mobile PWA, Telegram/Discord DMs and a multiplayer room
relayed through a Cloudflare Worker. The docs call it "open-source", but there is no public repo and the shipped
`LICENSE` forbids copying, modification and use without written permission. It overlaps with vega in
supervision (who is working, waiting, asking) and in the coordinator role, but it is a GUI for one human,
not an agent-to-agent message bus. **Recommendation: do not adopt or integrate. Take three ideas (file-intent
warnings, hook-driven status events, a phone approval channel) and build them in vega.**

## 1. What it is, who makes it, licence, source

| Item | Finding | Source |
| --- | --- | --- |
| Pitch | "RTS interface for agent orchestration"; each agent is "a hero unit on a 3D map" | [home](https://www.getagentcraft.com/), [docs](https://www.getagentcraft.com/docs) |
| Maker | Ido Salomon (npm author and sole maintainer, GitHub `idosal`); no company named | [npm](https://registry.npmjs.org/@idosal%2fagentcraft), [GitNation talk](https://gitnation.com/contents/agentcraft-putting-the-orc-in-agent-orchestration) |
| Licence | `"license": "SEE LICENSE IN LICENSE"`; the file: "Copyright (c) 2025 Ido Salomon. All Rights Reserved. … Unauthorized copying, modification, distribution, or use … is strictly prohibited without the express written permission" | npm tarball `LICENSE` |
| "Open source" claim | The docs say "an open-source tool". This contradicts the licence; treat it as **not open source** | [docs](https://www.getagentcraft.com/docs) |
| Source | **No.** No repo linked from the site or npm (`repository` field empty); `gh search repos agentcraft --owner idosal` finds nothing public. The npm package ships built JS (server minified, 115 MB with 3D assets), readable but not licensed for reuse | [npm](https://registry.npmjs.org/@idosal%2fagentcraft) |
| Language | TypeScript/JavaScript (Node ≥ 18, ES modules), React/3D frontend | npm `package.json`, [getting started](https://www.getagentcraft.com/docs/getting-started) |
| Activity | 40 npm versions, first 2026-02-11, latest **0.5.1 on 2026-08-21** (0.5.0 added Campaigns the same day); no release since | [npm](https://registry.npmjs.org/@idosal%2fagentcraft) |
| Price | None published; `/pricing` and `/terms` return 404. Free via `npx` today | site |
| Hosted parts | `app.agentcraft.gg` ("Launch App"), Supabase project (sign-in), PostHog EU (analytics), Cloudflare Worker `agentcraft-warroom.idosalomon.workers.dev` (Hall relay) | npm `server/dist/build-env.json`, `index.js` |

## 2. How it works

```
 browser UI (3D map, PWA) ◄─ws─► local Node server :2468 (~/.agentcraft/)
                                   ▲  ▲            │
   repo hooks: .claude/settings.local.json,        ├─ node-pty: claude / codex / opencode / cursor
   .codex/hooks.json, .cursor/hooks.json,          ├─ cloudflared quick tunnel (Remote Access, signed in)
   .opencode/plugins/agentcraft.js ── HTTP POST ───┘  ├─ CF Worker relay (Alliance Hall, signed in)
                                                      └─ Telegram / Discord bots (Channels)
```

| Aspect | Finding | Source |
| --- | --- | --- |
| Install / run | `npx @idosal/agentcraft` (`start [-d] [-p] [--all-projects]`, `stop`, `status`, `logs`, `open`, `install`, `uninstall [--purge]`, `restore`, `logout`, `doctor`) | [CLI](https://www.getagentcraft.com/docs/cli) |
| State | JSON files in `~/.agentcraft/`: `config.json` (port, **API keys**), `auth.json` (0600), `settings.json`, `mission-history.json`, `task-queue.json`, `plan-memory/`, `task-memory/`, `channels/state.json` (bot tokens) | [CLI](https://www.getagentcraft.com/docs/cli), [settings](https://www.getagentcraft.com/docs/getting-started/settings), [channels](https://www.getagentcraft.com/docs/features/channels) |
| Agents | Claude Code (primary), Codex, OpenCode, Cursor; OpenClaw passive/experimental. **No Pi, no Gemini/agy** | [docs](https://www.getagentcraft.com/docs) |
| Hooking | Writes hooks into each repo's `.claude/settings.local.json`, `.codex/hooks.json`, `.cursor/hooks.json`, `.opencode/plugins/agentcraft.js`; also ships a Claude Code plugin (`plugin/hooks/hooks.json`). Hooks are Node/bash scripts that POST events to `localhost:2468`. Claude events: SessionStart, UserPromptSubmit, Stop, PreToolUse (Task/Agent, AskUserQuestion, ExitPlanMode, Read/Write/Edit), SubagentStop, PermissionRequest | [Claude Code](https://www.getagentcraft.com/docs/integrations/claude-code), npm `plugin/hooks/` |
| Approvals | For agents it spawned, the PermissionRequest/ExitPlanMode hook **blocks** and polls `~/.agentcraft/approval-responses/` every 100 ms until the UI answers; external sessions pass through. The hook logs the full hook input to `~/.agentcraft/permission-request-debug.log` | npm `plugin/hooks/agentcraft-permission-request.sh` |
| Spawning | Own heroes run the provider CLI in node-pty; OpenCode via `opencode run --format json`; Codex with `CODEX_HOME=~/.agentcraft/codex-home`; Agent Teams set `CLAUDE_CODE_SPAWN_BACKEND=tmux` and poll `~/.claude/teams/` every 15 s | [opencode](https://www.getagentcraft.com/docs/integrations/opencode), [codex](https://www.getagentcraft.com/docs/integrations/codex), [agent teams](https://www.getagentcraft.com/docs/features/agent-teams) |
| Planning | Campaigns: a planner agent splits a spec into dependency-aware tasks in worktrees, collects verification evidence, then "Approve & Merge" / "Create PR". Two MCP servers ship (`agentcraft-campaign`, `agentcraft-verify`) | [campaigns](https://www.getagentcraft.com/docs/features/campaigns), npm `plugin/mcp/` |
| Multi-machine | Alliance Hall: each dev runs a local server; a Cloudflare Durable Object relays hero state, missions and file activity; Notice Board; soft "file intent" warnings when two heroes touch one file | [hall](https://www.getagentcraft.com/docs/features/hall) |
| Remote / mobile | cloudflared tunnel URL with per-session token, TTL 15 min–8 h; PWA with push and quick-reply; Telegram/Discord DMs can prompt, approve plans and grant permissions (pairing code) | [remote access](https://www.getagentcraft.com/docs/features/remote-access), [channels](https://www.getagentcraft.com/docs/features/channels) |
| Other | Loops (1 min–1 h), Docker/Apple containers, PTY terminal, file explorer, git stash/branch UI, skills.sh installer, achievements, music, race skins | [docs](https://www.getagentcraft.com/docs) |

### What leaves the machine

| Flow | Default | Evidence |
| --- | --- | --- |
| Prompts and code to the agent providers | always (inherent) | [privacy](https://www.getagentcraft.com/privacy) |
| PostHog analytics (event counts, hashed IDs, account ID when signed in) | off; code default `analyticsEnabled: false` plus an opt-in migration. When on, also a ping to `getagentcraft.com/api/health` | [privacy](https://www.getagentcraft.com/privacy), npm `server/dist/index.js` |
| Alliance Hall: prompts, replies, streaming output, mission summaries to all room members via the Worker | **sharing on by default** per connection; "cannot remove content already received" | [privacy](https://www.getagentcraft.com/privacy), [hall](https://www.getagentcraft.com/docs/features/hall) |
| Remote Access: full local API via a Cloudflare tunnel | only when started | [remote access](https://www.getagentcraft.com/docs/features/remote-access) |
| Remote viewer IPs to `http://ip-api.com` (plain HTTP) for city/country | when a viewer connects | npm `server/dist/remoteAccessManager.js` (`resolveGeo`) |
| npm version check | on start | npm `server/dist/versionCheck.js` |
| Account | Supabase sign-in; the code gates `/remote-access/*` and `/warroom/*` (Hall) behind `requireRealAuth`. Local core runs without an account | npm `server/dist/index.js`, `authManager.js` |

⚠️ The privacy page says remote viewer IPs are "kept locally and not geolocated"; the shipped code sends them to
ip-api.com over plain HTTP. The docs' "open-source" also contradicts the licence. Trust the code, not the docs.

## 3. Overlap with vega

| Concern | AgentCraft | vega | Verdict |
| --- | --- | --- | --- |
| Where agents run | Own node-pty terminals in a browser HUD; tmux for Agent Teams | herdr panes, any terminal; `vega mux spawn/adopt` | Same job, different host; vega keeps the user's terminal |
| Status / supervision | Hook events → hero states (idle, working, awaiting input, permission glow) | vegad watches herdr agent status; `silent`/`quota`/`asking`; wakes the parent ([vegad.md](vegad.md)) | Equivalent; AgentCraft's per-tool hook events are finer (file read/edit, subagent spawn) |
| Agent-to-agent messaging | None between agents; human → agent via UI, DMs, Notice Board; Agent Teams inbox is Claude's | `vega mux send`, `vega post` mail with correlation ids, delivery when free ([vega-mux-mvp.md](vega-mux-mvp.md)) | **vega only** |
| Coordinator | Campaign planner agent; human approves plan and merge in the UI | UAC role, sentinels, `slayer-decision` beads ([uac-role.md](uac-role.md)) | Similar intent; vega's is agent-driven, AgentCraft's UI-driven |
| Task store | `task-queue.json`, `plan-memory/` (local JSON) | beads on Dolt, replicated across hosts ([beads-sync.md](beads-sync.md)) | vega's is durable, shared, CLI-queryable |
| Dashboard | 3D RTS map, side panel, PWA | `vega board` TUI with Inbox (decisions, waiting panes) ([board-ui.md](board-ui.md)) | Same need; AgentCraft is richer visually, vega is terminal-native |
| Approvals | Blocks Claude/Codex permission requests until UI/phone/DM approves | UAC never approves gates; the user answers in the pane ([uac-role.md](uac-role.md) Hard limits) | Deliberately different; AgentCraft's is a remote-approval feature |
| Multi-machine | Hall via a hosted Cloudflare Worker, account needed | NATS on titan, per-host vegad, tailnet, no vendor | Same need; vega self-hosted |
| Mobile / remote | Tunnel + PWA + Telegram/Discord | none (herdr client over tailnet) | **AgentCraft only** |
| Isolation | Docker / Apple Containers per agent | worktrees only | **AgentCraft only** |
| Scheduled loops | 1 min–1 h prompt loops | none in vega (Claude's own `/loop`) | **AgentCraft only** |
| Live browser review | none | `vega review` ([live-review.md](live-review.md)) | **vega only** |
| CI watch, secrets, theme, herdr config, deploy, beads sync | none | `ci`, `secrets`, `theme`, `deploy`, `bd` plugins | **vega only** |
| Agents | Claude, Codex, OpenCode, Cursor | Claude, Codex, Pi, agy | Partly disjoint: no Pi in AgentCraft |
| Agent-facing CLI | Human-facing; agents see hooks only | AXI CLIs and `--skill` guides for agents | **vega only** |

### Ideas worth taking

| Idea | Why | Lands in |
| --- | --- | --- |
| **File-intent warnings**: soft warning when two agents on the same repo touch one file | Sentinels in different worktrees still collide at merge; vegad could flag overlap early | vegad reconcile + `vega uac status` / board Inbox `N` row ([vegad.md](vegad.md), [board-ui.md](board-ui.md)); input from `git diff --name-only` per worktree, no hooks needed |
| **Hook-driven fine status** (subagent spawned, plan waiting, AskUserQuestion) | herdr says only `blocked`; vega classifies dialogs by screen text ([pane-control.md](pane-control.md) D1) | Optional Claude/Codex hook that runs `vega post` to vegad; must stay optional (vegad.md non-goal: harness plugins as the main path) |
| **Phone approval / reply channel** (PWA push, Telegram) | The Slayer away from the desk cannot answer `slayer-decision` beads | `vega uac answer` over a push channel (e.g. ntfy) from vegad; read-only plus answers, never permission gates |
| Mission summaries (3-word / 1-sentence per finished task) | Cheap scannable history | `vega post done` one-liners already do this; reuse in board, nothing new |
| Per-agent containers | Safer `--dangerously-skip-permissions` sentinels | `vega mux spawn --kind … --container`; separate bead if wanted |

## 4. Use or interoperate?

**No usable integration surface.** There is no documented HTTP API apart from a few admin routes
(`/admin/refresh-worktrees`, `/event` for OpenClaw), no stable file format beyond private JSON in
`~/.agentcraft/`, and the licence forbids reuse of its code. In theory vega agents could POST to `/event` to
appear on the map, or AgentCraft's hooks could coexist with vega's (it keeps user hooks), but that buys a
second dashboard for the same agents.

Risks if anyone runs it next to vega:

| Risk | Detail |
| --- | --- |
| Licence / lock-in | Proprietary, single maintainer, no repo, no published price or terms; can change or vanish |
| Repo pollution | `install` writes hook files into every discovered repo (`.claude/settings.local.json`, `.codex/hooks.json`, …), up to three levels deep |
| Data exposure | Hall shares prompts and output by default; tunnels expose the full local API; bot tokens and API keys in plain JSON; debug log of every permission-request input |
| Gate conflict | Its approval hook blocks Claude's PermissionRequest for its own heroes; a vega sentinel adopted into it could hang waiting on a UI the UAC never answers |
| Docs vs code | Two contradictions found (open-source claim, IP geolocation) |
| Account | Supabase account needed for Hall and Remote Access |

## Sources

[home](https://www.getagentcraft.com/) ·
[docs](https://www.getagentcraft.com/docs) ·
[getting started](https://www.getagentcraft.com/docs/getting-started) ·
[first hero](https://www.getagentcraft.com/docs/getting-started/first-hero) ·
[settings](https://www.getagentcraft.com/docs/getting-started/settings) ·
[CLI](https://www.getagentcraft.com/docs/cli) ·
[Claude Code](https://www.getagentcraft.com/docs/integrations/claude-code) ·
[Codex](https://www.getagentcraft.com/docs/integrations/codex) ·
[OpenCode](https://www.getagentcraft.com/docs/integrations/opencode) ·
[Cursor](https://www.getagentcraft.com/docs/integrations/cursor) ·
[OpenClaw](https://www.getagentcraft.com/docs/integrations/openclaw) ·
[remote access](https://www.getagentcraft.com/docs/features/remote-access) ·
[hall](https://www.getagentcraft.com/docs/features/hall) ·
[channels](https://www.getagentcraft.com/docs/features/channels) ·
[campaigns](https://www.getagentcraft.com/docs/features/campaigns) ·
[agent teams](https://www.getagentcraft.com/docs/features/agent-teams) ·
[containers](https://www.getagentcraft.com/docs/features/containers) ·
[scheduled tasks](https://www.getagentcraft.com/docs/features/scheduled-tasks) ·
[terminal](https://www.getagentcraft.com/docs/features/terminal) ·
[worktrees](https://www.getagentcraft.com/docs/features/worktrees) ·
[git management](https://www.getagentcraft.com/docs/features/git-management) ·
[missions](https://www.getagentcraft.com/docs/features/missions) ·
[skill scrolls](https://www.getagentcraft.com/docs/features/skill-scrolls) ·
[side panel](https://www.getagentcraft.com/docs/features/side-panel) ·
[troubleshooting](https://www.getagentcraft.com/docs/troubleshooting) ·
[privacy](https://www.getagentcraft.com/privacy) ·
[npm registry](https://registry.npmjs.org/@idosal%2fagentcraft) ·
[GitNation talk](https://gitnation.com/contents/agentcraft-putting-the-orc-in-agent-orchestration).
Not read: the docs pages for side features (keyboard shortcuts, voice input, achievements,
file explorer, fog of war, music, race skins); `/pricing`, `/terms`, `/docs/changelog` return 404.
