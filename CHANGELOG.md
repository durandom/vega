# Changelog

What an occasional user of vega must know after an upgrade: new infra, config units, migrations and
notable commands. `vega onboard` shows the blocks newer than the one this host saw last; the rules are in
[docs/design/onboarding.md](docs/design/onboarding.md). Newest block first; never edit a block that is on
main, add a new one. Entry kinds: `required`, `optional`, `unit`, `migration`, `feature`.

## 206 · 2026-11-13

- feature: the project window's Decisions tab renders again: answer inputs are created outside render
  (herdr-app rejected `InputState.new` during render and left a window that could not close).

## 205 · 2026-11-13

- feature: live Claude theme sends retry with bounded backoff and resume after vegad restarts.
  `vega theme` shows each pane's last sent mode and pending or failed delivery status.
- feature: pi's native `system` and automatic light/dark pairs stay native; vega no longer tries
  to copy generated system themes or replace native pair settings.

## 204 · 2026-11-13

- feature: the vega tab bar and pane headers use stock 0.1.15 forks with static working dots.
- unit: apply `herdr-app.vega-tabs` and `herdr-app.vega-pane` before `herdr-app.layouts`
  (or apply the `herdr-app` group) to load the forks. `herdr-app.profiles` now skips hosts
  without a herdr-app home.

## 203 · 2026-11-13

- feature: working agents in the vega sidebar use a static orange status dot, including child and
  collapsed rail rows, avoiding continuous spinner animation.

## 202 · 2026-11-13

- feature: the vega sidebar reconciles waiting hands only when their state changes. The Needs you
  popover refreshes when displayed ages or badge freshness change, avoiding unchanged redraws every second.

## 201 · 2026-11-13

- feature: Promote in the status window's Config view saves straight into the vega checkout this
  vega was built from (`vega window promote <name>` without `--repo`); the folder picker only
  appears when no checkout is known.

## 200 · 2026-11-13

- feature: status windows use a fork of herdr-app's stock pill tab bar, with program icons, active-tab close, scrolling and window controls. Double-click a tab to rename inline; drag it onto another tab to persist its order, including when optional groups are unavailable.
- feature: the right-side Config button shows a colored, numbered diff from the applied repo profile to the effective local profile, with Promote and Reset actions.
- unit: after upgrading, run `vega config apply herdr-app.profiles`, then `vega window status` to load the updated bar and diff viewer.

## 199 · 2026-11-13

- feature: status tabs support + / leader-c machine selection, inline rename, drag reorder and close. Local edits persist in `~/.config/vega/windows/<profile>.toml`; closing hides a tab without killing its terminal.
- feature: Config shows the effective profile with promote/reset. `vega window promote <name> --repo <checkout>` writes an uncommitted repo profile for review; rebuild/install, apply it and reset local edits afterward.
- unit: run `vega config apply herdr-app.profiles` after upgrading, then `vega window status` to load the management bar and keys.

## 198 · 2026-11-13

- feature: the sidebar project badge opens a Needs you popover with waiting panes, decisions and notes, including Open and resolution actions. Waiting hands appear on the child's row and clear after viewing its turn and a later turn advance; Done dismisses them immediately. UAC rows no longer repeat project counts.

## 197 · 2026-11-13

- feature: `vega window status` gives proteus, ganymede, optional io btop, and ganymede tokens their own top tabs, preserving tab-switching keys and reusing live terminals. io is skipped with a warning when btop is unavailable.
- unit: run `vega config apply herdr-app.profiles` to update the status profile, layout, navigation mod and keys before `vega window status`. Profiles support `[[tab]]` groups with one or two `[[tab.terminals]]` entries; two terminals split horizontally. Existing flat profiles still work.

## 196 · 2026-11-13

- feature: `vega window status` opens a separate herdr-app home with proteus/ganymede btop and ganymede token-burn tui. Reopening reuses live terminals; the main sidebar hides profile terminals.
- unit: `herdr-app.profiles` writes TOML-defined profile homes, layouts, selected mods and settings. Run `vega config apply herdr-app.profiles` before opening a profile. Fresh homes skip welcome; existing shared state is preserved.

## 195 · 2026-11-13

- feature: the git shim hints at Worktrunk commands for branch deletion, worktree housekeeping and new-branch checkouts in wt-managed repos, preserving git's stdout and exit code.

## 194 · 2026-11-13

- feature: `scripts/check-mods.sh` checks every herdr-app mod and cleans its generated type files, including on failure.

- unit: dcg.config allows safe `git branch -d` / `--delete`; force deletion stays blocked.

- feature: `vega uac prune` delegates integrated branch deletion to Worktrunk, including squash merges. Unmerged work remains residue; fast-forward the local target before pruning.

## 193 · 2026-11-13

- feature: the sidebar shows a headless sentinel's unread done as a filled grey dot when its
  parent is live: done, handed to the UAC. Headed sessions and UACs keep the blue unread dot.

## 192 · 2026-11-12

- feature: the Uber-UAC has its own hc role label (`vega.role=uber`, plus `vega.engaged=yes`
  while its window is open), kept by vegad from the claim file like the UAC role. The sidebar
  lists it as one "Über" row at the top; its project label no longer creates a project group
  or a "no UAC" marker.

## 191 · 2026-11-12

- feature: the sidebar hides a project without a UAC when none of its agents or terminals
  render in its own group and it is not pinned (no more empty "no UAC" headers).

## 190 · 2026-11-12

- feature: the sidebar lists a sentinel from another project in that project's group only
  when the project has a UAC; otherwise it stays a branch under the agent that spawned it.

## 189 · 2026-11-12

- feature: agents lost in a reboot resume in place. hc restores kept terminals with a
  shell; once per boot vegad finds those whose spawn record names a session and, per
  `~/.config/vega/resume.toml` `mode = "offer" | "auto" | "off"` (default offer), leaves
  one note with `vega mux reopen <t>...` or types the resume command itself. claude,
  codex and pi are supported.
- required: run `vega uac claim` once in each running UAC so its terminal is recorded;
  agents spawned before this upgrade lack the launch argv and env and resume with the
  default args plus their recorded extra args.

## 188 · 2026-11-12

- feature: in the project sidebar a child whose own project differs from its parent's now shows in
  its own project group with a "↑ parent" hint, not as a branch of the foreign parent. Run
  `vega config apply herdr-app` to apply the mod.

## 187 · 2026-11-12

- feature: the project sidebar lists live terminals without an agent (token-burn tui, pdc, a dev
  server, a shell) under their project as dim `◇` rows. Active hides idle shells unless the
  project is pinned. Run `vega config apply herdr-app` to apply the mod.

## 186 · 2026-11-12

- feature: `vega mux spawn --worktree` leaves one terminal in the new herdr-app space, the agent.
  The shell herdr-app makes for the space is removed once the agent runs; no migration needed.

## 185 · 2026-11-12

- feature: after a reboot, hc terminal labels follow the UAC claims: a UAC terminal keeps its
  claimed project, and a bare login shell (hc keep's recovery shell) no longer takes a project
  from its space folder. Upgrade vegad; stale labels clear on the next refresh.

## 184 · 2026-11-11

- feature: the project sidebar keeps live agents visible after their UAC exits, with a
  "no UAC" marker. Project pins, filters and manual ordering still apply.

## 183 · 2026-11-11

- required: upgrade private hosts' vegad to reject command and quarantine replies outside
  the authenticated sender's inbox. This closes response permission laundering by granted
  inbox-only hosts; no broker authorization change is required.

## 182 · 2026-11-11

- feature: `vega post auth-config --retired-host <host>` (repeatable) keeps the admin/viewer
  read subjects for retired inbox archives without a host key or grants. Remove the host's
  active key entry and grants; use `--retired-host deimos` to preserve its archive access.

## 181 · 2026-11-11

- feature: tmux-style last navigation in the sidebar: `ctrl-s l` or `cmd-;` toggles
  the last agent, `ctrl-s shift-l` the last space, and `ctrl-s ;` the last pane in
  the current tab. Run `vega config apply herdr-app` to apply the mod and keys.

## 180 · 2026-11-10

- migration: deimos is retired and replaced by puck. The built-in inbox-only host default and
  current user-facing host examples now name puck.

## 179 · 2026-11-10

- optional: dcg is pinned to 0.16.0 via `mise run install-dcg` (verified release archive,
  previous binary backed up, strict doctor failure rolls back). Install it per host;
  vega deploy does not upgrade dcg automatically.
- unit: `pi.dcg` adds a bash guard at `~/.pi/agent/extensions/dcg-guard.ts` in the `hooks`
  group. Review/apply it and restart pi. `claude.settings` and `codex.hooks` now own dcg's
  shared `dcg/hook.json` PreToolUse entry; review their diff before applying.
- unit: guard-only `claude.dcg` preserves preferences. Fresh hosts use installer
  `--configure-hooks`; failed fresh installs remove managed guards after binary rollback.
- migration: review/apply `dcg.config` to disable hook self-repair; vega owns the hooks.
  `vega config check` reports dcg version >=0.16.0 and each installed agent's guard.
  Run `dcg doctor --strict` for Codex approval/reachability; approvals remain human-owned.

## 178 · 2026-11-10

- feature: post waits up to 60 s for slow broker operations. Send and done retry uncertain
  publishes with the same message ID, preventing duplicate mail after a lost acknowledgement;
  vegad tolerates short broker pauses before reporting an outage.

## 177 · 2026-11-10

- feature: inbox-only hosts deliver relay tool mail (hygiene, CI, beads sync and upgrade
  proposals) directly to free local panes, holding drafts as in local mode. This avoids denied
  broker publishes and endless retries; Uber asks and digests are skipped on these hosts.

## 176 · 2026-11-10

- optional: the sidebar shows done (blue) only while unread, like the tab bar (turn past
  `herdr.seen`); seen sessions read as idle and the Active filter hides them. Apply the updated
  vega-sidebar mod (`vega config apply herdr-app`).

## 175 · 2026-11-10

- feature: sidebar sub-agents now show unread done as a blue dot like tabs and remain in Active
  until read. Pin a project with its header needle to keep its UAC visible even when idle;
  pins respect label/machine views and persist via `vega project config <id> --pin|--unpin`.
  Apply the updated vega-sidebar mod (`vega config apply herdr-app`).

## 174 · 2026-11-10

- optional: the sidebar's Active filter now also hides finished sentinels: a sub-agent's done
  shows as a grey ring and counts as idle. Apply the updated vega-sidebar mod
  (`vega config apply herdr-app`).

## 173 · 2026-11-10

- migration: `vega board`, `vega open` and the herdr TUI config unit are removed.
  Install/deploy removes stale command binaries and links from `~/.local/bin` and
  the agent shim directory. On hosts that used the plugins, run `herdr plugin unlink vega.board`
  and `herdr plugin unlink vega.open`, then remove their keybindings from the host-owned
  `~/.config/herdr/config.toml`. Remove obsolete local `~/.config/vega/config/herdr/`
  overrides and leftover plugin-local `bin/` files. Keep the live herdr config while
  legacy panes still use it; mux's herdr backend and herdr-app mods remain supported.
  The prior state is tagged `herdr-tui-final` (dbd11a8); the last commit with them is a652b82.

## 172 · 2026-11-10

- feature: the herdr-app sidebar has an Active toggle beside the view selector. It hides idle
  and unknown sessions and empty groups, keeps working, blocked, done and focused agents,
  and keeps a UAC with active descendants. The setting survives restarts and combines with
  label/machine views; click the dim idle-hidden count to show everything in the view again.
  Apply the updated vega-sidebar mod (`vega config apply herdr-app`).

## 171 · 2026-11-10

- optional: the herdr-app vega status line and project Overview read beads, asks and CI in the
  project's registry checkout instead of the UAC terminal's shell folder, which could show another
  repo's counts. Apply the updated vega and vega-sidebar mods (`vega config apply herdr-app`).

## 170 · 2026-11-10

- feature: the herdr-app project/config window has a Models tab for registry defaults and all
  known hosts, including inbox-only hosts. Choose an allowed model or Reset a lane/tier pin;
  each cell shows the effective model, source and expiry, and CLI failures stay visible.
  Pi defaults are configured through zai. Apply the updated vega-sidebar mod to use it.

## 169 · 2026-11-10

- optional: codex `gpt-6.1-terra` is in the seats allowlist without a global tier. Hosts may pin it,
  e.g. `vega lanista pin codex --tier budget --host puck --set gpt-6.1-terra`. Run `vega lanista install`
  (or deploy) on the setting host first.

## 168 · 2026-11-10

- feature: private hosts can configure shared model defaults with
  `vega lanista pin <lane> --tier <tier> --default --set <model>` or `--clear`.
  Defaults live at `<control>.lanista.default` (theme control, normally proteus).
  Resolution is project > host > registry default > repo seats. Inbox-only hosts
  never read defaults and need no new grant; their own host pins stay independent.
  `hosts --json` identifies project, host, default, or global sources per cell.

## 167 · 2026-11-10

- required: orrery must regenerate titan auth-config with existing flags plus
  `--cmd-grant puck=vega.cmd.proteus.lanista.puck` and
  `--registry-read puck=puck.lanista.host`, keeping `--inbox-only puck`.
  Applying this requires Slayer approval. Puck gets no KV publish rights;
  proteus validates commands and writes only the derived host key.
- feature: lanista resolves project > host > global. `pin --host <host> --set`
  edits host tiers (default expiry never); inbox-only hosts relay their own edits
  and cache exact-key reads offline. `hosts --json` exposes models and allowlists.
  Pi allowlists Google Argon/Flash for host/project pins; its global fallback remains
  zai/GLM. Puck mappings are configured after deploy.

## 166 · 2026-11-10

- feature: `vega mux spawn --worktree` starts a tab in the caller's space when no local
  herdr-app is available, preserving the worktree cwd, brief and parent link. Remote
  `--machine` worktrees are created on the target and use the same tab fallback.

## 165 · 2026-11-10

- required: to show puck's keychain lock, regenerate titan's broker authorization with the existing
  arguments plus `--cmd-grant puck=vega.cmd.proteus.keychain.puck`, keeping puck under `--inbox-only`.
  Applying this is a separate Slayer step. Puck gets only this command; the private recipient gets
  the exact status-key write needed to relay it. Never grant inbox-only hosts KV publish rights:
  a `Nats-Rollup: all` header on even one allowed subject can erase the shared registry.
- migration: all hosts publish keychain state as `<host>.status.keychain`. Upgrade publishing vegad
  and the app host reader together. Inbox-only vegad sends bounded, validated status to its theme
  control host on change and every 60 seconds. Samples appear without a host beat and age out
  after 180 seconds.

## 164 · 2026-11-10

- feature: the keychain lock turns green within seconds of the password: while an unlock window
  is open, herdr-app probes that host directly instead of waiting for vegad's next round.

## 163 · 2026-11-10

- feature: the keychain unlock window explains the lock and shows the exact command, retries a
  failed unlock, and stays open after the unlock until you close it (the title turns green).

## 162 · 2026-11-10

- feature: the status line shows one keychain lock, the focused terminal's machine's, instead of
  one per Mac. Unlock windows for other machines still open when they lock. Apply with
  `vega config apply herdr-app`.

## 161 · 2026-11-10

- feature: vegad probes the login keychain on each macOS host every minute and shares its state
  through the registry. The herdr-app status line shows one green/red/dim lock per Mac. A new
  lock opens a dedicated unlock terminal window once; click a red lock to reopen it. Press Enter
  there to start the password prompt. Only the Slayer enters the password; vegad never unlocks.
  Deploy matching vegad binaries and apply the updated `herdr-app.mods` unit after landing.

## 160 · 2026-11-10

- unit: `claude.hooks` adds silent Claude lifecycle telemetry through `vega mux hook state`.
  Apply the unit after deployment and restart Claude sessions; the user handles any trust prompt.
  Metadata only (including a Stop question flag) stays in the latest 200 pane-state JSONL lines.
- feature: vegad logs disagreements between hc and hook state for supervised local Claude panes
  once per change. This is shadow mode: delivery and supervision behavior stay unchanged.

## 159 · 2026-11-10

- feature: `vega mux spawn` sets `GIT_SSH_COMMAND=ssh -o BatchMode=yes` for the agent: git over
  ssh fails at once instead of hanging on an invisible passphrase prompt (locked keychain on puck,
  vega-oiwq). `--env GIT_SSH_COMMAND=...` overrides it. Applies to new spawns.

## 158 · 2026-11-10

- feature: the theme switch in a working Claude pane now runs: the Enter after the typed
  `/config theme=<mode>` only closed Claude's completion menu, so vegad presses Enter once more
  when the input box still holds exactly that line.

## 157 · 2026-11-10

- feature: a light/dark switch reaches busy Claude panes at once: vegad types `/config theme=<mode>`
  also while the agent works (queued), not only when idle.
- feature: `vega mux spawn --kind claude` sets `CLAUDE_CODE_ENABLE_PROMPT_SUGGESTION=false`: the
  dim prompt suggestion read as a draft in hc screens and held every pointer 60 s (vega-lhc.257).
  Pass `--env CLAUDE_CODE_ENABLE_PROMPT_SUGGESTION=true` to keep it.

## 156 · 2026-11-10

- feature: mail no longer waits forever for a pane whose status hc never reported (a Claude
  pane listed as `-`): after 5 min vegad types the pointer when the pane's input box is empty
  (vega-lhc.257). Update with `vega deploy apply <host>`.

## 155 · 2026-11-10

- feature: a host without a vega UAC no longer counts notes without a project on every
  project badge (e.g. 3 notes on spellkave whose Notes tab was empty); they count on the vega
  project of the host that runs the vega UAC. Update with `vega deploy apply <host>`.

## 154 · 2026-11-10

- feature: sidebar views can match any condition: `vega project view <name> --match any` (or the
  "a label or a machine" toggle in the filter popup) shows projects with a chosen label OR on a
  chosen machine, e.g. everything on puck plus label vega. Default stays `all`.

## 153 · 2026-11-10

- feature: a headed child no longer shows the waiting hand for turns you typed into it after it
  answered its parent: silent now needs an unanswered brief. `vega mux dismiss <child>` (on the
  parent's host) drops a hand you handled; the project window Overview and the vega inbox offer
  it as Dismiss (vega-0h4c.56). Update vegad with `mise run install` or `vega deploy apply`, then
  `vega config apply herdr-app`.

## 152 · 2026-11-09

- feature: the herdr-app project window has an Asks tab: open `vega notify` asks of the project
  from every host, with Ack, Resolve and Go to pane. The Overview asks tile opens it
  (vega-0h4c.50.3). Apply with `vega config apply herdr-app`.
- feature: project labels as pills: the Config tab adds labels by Enter and removes them by
  their pill, offering labels other projects use; sidebar project headers show their labels,
  and a click filters the sidebar to that label. The filter popup lists All, each label and the
  saved views, and builds a view from label and machine toggles. Fixes inputs in the project
  window losing focus on click and the view choice never reaching the sidebar.

## 151 · 2026-11-09

- feature: herdr-app project window. Right-click, the settings icon or the summary badge on a
  UAC project in the sidebar opens a draggable window with Overview, Board, Decisions, Notes,
  UAC and Config tabs; pin it per project or let it follow the focused project. The status line
  shows the focused project's decisions, asks, notes, beads, CI, capacity and beads sync, each
  opening the window on its tab. Sidebar project headers show one summary badge instead of
  separate mail, decision and waiting icons (vega-0h4c.50). Apply with `vega config apply herdr-app`.

## 150 · 2026-11-09

- feature: `vega capacity` shows thin or exhausted windows as `on credits` when
  token-burn reports fresh spendable overage credits, suppressing the capacity
  warning on spawn. JSON adds account IDs, credit flags/balance and `on_credits`;
  missing or stale credits remain unknown and keep the warning. Balances retain
  their provider credit units (vega-lweq; token-burn 774f459 or newer).

## 149 · 2026-11-09

- feature: `vega notify [list]`, `vega ci`, `vega capacity`, `vega bd status`
  and `vega uac` accept opt-in `--json` for the project Overview. Responses
  are schema-1 objects with explicit empty lists and structured errors;
  notify, CI and Beads status support exact project filters. UAC JSON selects
  its project from cwd without a calling pane. TOON remains the default
  (vega-0h4c.50.4). On-credits awaits token-burn's CLI read contract.

## 148 · 2026-11-09

- feature: `vega project config` stores personal display names and labels by project ID;
  `vega project view` saves label/machine filters in `~/.config/vega/projects.toml`.
  Right-click a sidebar project or UAC row for Config. Priority and Grouped by space
  sort modes are removed; the header switches saved views. Project ordering is manual
  only. Non-UAC Spaces rows retain Rename (vega-0h4c.49).

## 147 · 2026-11-09

- feature: parent and peer mail reaches working agents behind the input draft guard;
  Codex hc agents with an empty prompt receive pointers immediately
  (vega-0h4c.52). Known Codex placeholder hints count as empty; only changes in the input box
  extend the 60 s draft hold. Output and footer activity no longer delay mail, and the relay
  logs a hold once instead of logging every draft check. Multiline Codex drafts hold even
  when the first prompt line is empty.

## 146 · 2026-11-09

- optional: `cargo-target` in `vega config infra` warns when this checkout's
  `target/debug/deps` exceeds 50,000 entries, with a capped scan and a pointer
  to `mise run target-sweep`. Dev/test builds pack debug info to stop macOS
  retaining loose objects while keeping backtrace source lines. Checks now
  evict seven-day-old deps/incremental cache families under Cargo's lock, with
  per-run scan/deletion limits. See [build-cache.md](docs/design/build-cache.md)
  for measurements and recovery (vega-htsq).

## 145 · 2026-11-09

- feature: right-click a space in the vega sidebar and choose Rename to edit its name.
  Enter saves local and remote space names; Escape cancels (vega-0h4c.48).

## 144 · 2026-11-09

- feature: vegad's draft guard covers hc terminals again (vega-0h4c.47): a pointer no longer joins
  text in a Claude, Codex or pi input box. It waits while the draft (for Codex, multi-line pi and
  pastes: the screen) changes and goes 60 s after the last change, at once on an empty box. Nothing is stashed or restored. Temporary until cores carry
  herdr-core 3ba03e6; `HC_DRAFT_HOLD` in `plugins/post/src/main.rs` switches it off.

## 143 · 2026-11-09

- feature: an inbox-only host (deimos, puck) no longer waits 4 s and reports a registry timeout:
  `vega uac` prints `registry: off (inbox-only host: local agents only)`, and `vega mux next`,
  `vega notify`, `vega post notes`, `vega orchestration`, `vega config infra --fleet` and the wait commands skip the read too. `vega post relay install`
  on such a host no longer warns that the host is not verified, and the old-login-job warning
  names its next step (vega-0h4c.44).

## 142 · 2026-11-09

- feature: an inbox-only hc machine (puck) is a known registry host without a beat, so adopt, mux
  send and post send to `puck/tN` no longer fail with `machine_host_unknown` and
  `vega config infra --fleet` stops flagging it (vega-0h4c.45). `vega uac grant`/`revoke` also take
  hc terminal IDs: `vega uac grant puck:t1 proteus:t6p --expires 7d` on the receiving host.

## 141 · 2026-11-09

- migration: vegad has no login job any more (vega-0h4c.42). `vega post relay install` writes
  none and removes `vega.vegad-start` (macOS LaunchAgent) or `vega-vegad-start.service` (Linux
  user unit) from an earlier install; rerun it on each host, or deploy and let the next install
  drop it. The herdr-app vega mod now starts vegad on its host and, through `hc exec`, on every
  connected hc machine (at start, when a machine connects, then every five minutes), honouring
  that host's `vegad.env` pin. `vega config infra` flags a leftover login job as `vegad-start`.

## 140 · 2026-11-09

- feature: puck (corp work Mac) joins deimos in the built-in inbox-only host list: its mail goes to
  quarantine and it never mails agents, as its broker key already enforces. A configured
  `~/.config/vega/inbox-only` or `VEGA_POST_INBOX_ONLY` still replaces the default.

## 139 · 2026-11-09

- unit: `herdr-app.layouts` is rebased onto the shipped `main` of herdr-app 0.1.14 (no structural change
  versus 0.1.13), so the layout drift note is gone (vega-0h4c.41).
- unit: the three herdr-app mod units treat `gpui-kit.d.ts` and `jsconfig.json`, which the app writes into
  every mod folder, as app-owned: they show `ok` after an app restart instead of `differs`, and survive a
  folder replacement (vega-0h4c.40).
- feature: vega now checks hc results against the vendored hc 0.1.19 schema (identical to 0.1.18 apart from
  the version; 0.1.18 cores still validate). The empty-`machines` workaround stays: 0.1.19 still omits it.

## 138 · 2026-11-08

- feature: `vega config apply herdr-app` retries missing-mod layout activation for
  up to five seconds while the app discovers freshly installed mod folders, so
  the first apply can succeed in one run (vega-0h4c.22).

## 137 · 2026-11-08

- feature: Scratch herdr-app instances with private app, config or state homes no
  longer start a detached vegad; real-home startup remains a fallback to the login
  starter (vega-0h4c.33).

## 136 · 2026-11-08

- feature: The herdr-app status line keeps badge state through a slow read and reports
  a short error only after three consecutive failures (vega-0h4c.36).

## 135 · 2026-11-08

- feature: mail to `machine/tN` checks hc when a fresh registry has not listed the
  terminal yet, so the first send after a remote spawn works immediately. The machine
  must still match a verified registry host; TUI mail keeps its existing checks.

## 134 · 2026-11-08

- feature: relay install always pins its resolved host and warns when a configured registry
  cannot verify it. If your hostname differs (WSL `io-wsl`, registry `io`), run
  `VEGA_POST_HOST=io vega post relay install` from the logged-in user session.

## 133 · 2026-11-08

- feature: Skills library test fixtures isolate required executables in a private bin,
  so host plugins beside node cannot leak into guide discovery (vega-0h4c.29).

## 132 · 2026-11-08

- feature: Gate starts resolve hc author metadata before creating running review records,
  so a failed lookup leaves no orphan running beads (vega-0h4c.16).

## 131 · 2026-11-08

- feature: Uber Slayer input captures exclude hc-native agent messages and system notes,
  keeping peer messages out of human decisions (vega-0h4c.14).

## 130 · 2026-11-08

- feature: sentinels spawned onto a machine from another one (rally-ng's io lanes) show their
  name in the sidebar instead of their agent kind.

## 129 · 2026-11-08

- feature: vegad labels the hc terminals of a core with no machines added (io, deimos):
  hc 0.1.18 omits the empty `machines` list from `terminal.list`, which vega rejected before.

## 128 · 2026-11-08

- feature: `vega mux spawn --worktree` on hc selects the herdr-app space you were looking at
  again, so the new worktree space opens in the background; it warns on stderr when it cannot.

## 127 · 2026-11-08

- feature: hc RPC calls keep their connection open until the reply arrives,
  fixing lost replies from slow commands and remote agent preparation.

## 126 · 2026-11-08

- unit: The herdr-app sidebar mod is renamed `vega-agents` -> `vega-sidebar` (unit
  `herdr-app.vega-sidebar`) and is now the only sidebar list, titled VEGA: UAC
  projects in dragged order with their agents on every machine, then spaces without
  a UAC, in the herdr TUI's symbols. The layout drops the spaces list and the
  Settings / Add machine footer; the status line is the shipped one without the
  palette hint. Run `vega config apply herdr-app`.
- migration: Delete the old mod folder `~/.config/herdr-app/mods/vega-agents`
  (and its `.vega-mod.vega-agents-bak`) after the apply.

## 125 · 2026-11-07

- feature: `vega uber` claim, status, ask and brief work from hc terminals.
  hc and herdr sessions share one host claim and check both local inventories;
  concurrent claim/release operations refuse with `claim_busy` and can be retried.

## 124 · 2026-11-07

- feature: hc calls use the versioned RPC API, including remote preparation and
  vegad's event stream. Results are checked against the vendored hc 0.1.18 schema
  so incompatible responses fail explicitly; subprocess test fakes use the same
  check. The herdr TUI backend is unchanged.

## 123 · 2026-11-07

- feature: hc terminal discovery uses the full RPC inventory, restoring harness
  status, space labels, parent links and project attribution from foreground cwd.
  The herdr TUI backend continues to work alongside hc.

## 122 · 2026-11-07

- unit: Reapply the herdr-app mods for short machine/project group labels, a single
  tree row per child, and project totals only when positive. Mail opens readable
  archive cards; the decision inbox closes when its last decision or live waiter
  disappears, including a project-filtered inbox.
- feature: vegad uses the local herdr-app space folder when an hc agent's cwd
  cannot identify a project. Full project IDs remain available for routing.
  `vega post log --json` supplies full archive messages without acknowledging mail.
- feature: New hc worktree launches record the shell created by `space.new`.
  Closing the worker also removes its owned placeholder when no other terminal
  uses the space. Existing unmarked spaces need manual cleanup with the user.

## 121 · 2026-11-07

- optional: `vegad-start` starts vegad at login without herdr-app or herdr TUI;
  `vega config infra` reports the installed login job.
- migration: Run `vega post relay install` after upgrading on each host to install
  its launchd/systemd user login job. The herdr TUI hook stays supported.
- feature: vega-inbox reads decisions and waiting panes from connected hc machines;
  answers and dismissals execute in that machine's recorded checkout.

## 120 · 2026-11-07

- migration: Remote hc spawns prepare repository trust and inherited hook approvals
  on the target machine. Deploy matching vega binaries on both ends. Resolve macOS
  Herdr Core privacy prompts on the target's screen before unattended spawning.
- feature: hc spawns pass environment settings separately from the direct harness
  argv. Status still requires a harness that reports OSC 7501; unknown status waits
  for delivery rather than being treated as idle.
- migration: An hc machine name must equal its vega registry host. `mux send
  ganymede/t7` and `post send hc:ganymede/t7` route without `--host`, after verifying
  the host exists in the registry. Unknown hosts and conflicting `--host` values
  fail without sending. `vega config infra --fleet` checks every configured hc
  machine and reports `hc machine rename <old> <host>` for mismatches.

## 119 · 2026-11-07

- migration: `vega uac` status, claim/release, adopt, park/unpark and ask/answer
  work from hc terminals alongside the herdr TUI. Local views include both
  backends; explicit `hc:t7` and `herdr:w1:p2` targets select their own backend.
  Answer lookup failures report `unknown`; the answer remains in the bead.
- feature: `vega uac hold <pane>` pauses sentinel attention without a decision
  bead or bead polling, replacing any decision park. `unpark` or new work from
  the parent resumes it.
- feature: vegad attention wakes exclude already mailed done reports and silent
  children waiting on gate or CI verdicts. Parked children and closed hc terminals
  remain quiet. Wakes track actionable item identities and remind the UAC to reply
  in at most ten words. Spawn briefs request only done, blocked, or requested CI
  verdict mail, with no acknowledgement or progress mail.

## 118 · 2026-11-07

- optional: `hc` and `herdr-app` enable the new terminal UI; `python3` reads its badge snapshots.
  Legacy `herdr` is now optional for hosts using hc. Doctor checks these tools locally.
- feature: vegad publishes changed per-terminal `vega.*` labels to hc. The Vega layout's
  agents clone keeps the tree and priority view, adds machine/project grouping,
  and makes mail, waiting, decisions, UAC, harness and git indicators clickable.
- unit: `herdr-app.vega-agents` and `herdr-app.vega-inbox` join `herdr-app.mods` and
  `herdr-app.layouts`. Apply the `herdr-app` group together; every mod is checked before writing.
  Shipped agents and spaces are unchanged. Doctor lists hc, herdr-app and Python 3 locally.
- feature: Vega inbox shows decision questions, Answer/Dismiss actions and waiting panes.
  New decisions open a kept non-modal window and an actionable toast, preserving keyboard
  focus. Waiting panes and unread notes never auto-open it. Mail stays in the sidebar.
  Answers and dismissal reasons run in the decision project's recorded checkout.

## 117 · 2026-11-07

- migration: `vega mux spawn` follows the caller's terminal world; `--backend herdr|hc`
  overrides it. hc children retain their parent link and outlive a parent restart.
  Local spawn uses the caller's herdr-app space; remote children omit that local
  space ID. `--worktree` creates a space through
  worktrunk and herdr-app. Explicit split/workspace placement remains TUI-only.
  Show, direct send, close, dialog/answer, next, reopen and transcript support hc
  targets. Spawn records retain their backend; hc harness session IDs are captured
  by ancestry-verified vega calls from the child for reopen/transcript.
- feature: `vega deploy` and mux machine lookup read `hc machine ls`, falling back to
  `herdr machine list` when hc is absent or its fleet is empty. Deploy shows `--via`
  machines as unsupported and skips them with a warning during apply; SSH targets
  remain usable. hc remote spawn and deployment apply refuse corporate-to-private
  control. Qualified hc mail addresses retain their machine; post requires the
  receiver's registry host before enqueuing them. No machines migrate automatically.

## 116 · 2026-11-07

- feature: vegad subscribes to hc events, with `hc ls` polling as a fallback, and delivers mail
  and supervises agents in hc terminals. Registry agent rows include their backend and address.
  `vega post send t7` and `vega post send hc:t7` target hc terminals; machine-qualified hc
  addresses require `--host <registry-host>`. See [hc migration](docs/design/hc-migration.md).

- unit: `herdr-app.mods` and `herdr-app.layouts` add a maintained vega clone of Herdr's main layout
  with a small vegad status row. The row starts vegad when instantiated. With herdr-app running,
  `vega config apply herdr-app` validates the mod and selects the vega layout on first apply;
  later layout choices stay yours. `vega config` reports source version/hash drift; regenerate with
  `python3 scripts/herdr-app-layout.py --write`, review and commit the result before applying.
  See [config](docs/design/config.md) "Vega layout and startup mod".
- feature: `vega gate start` and gated pushes now work from hc/herdr-app terminals. Reviewers
  run in detached hc terminals visible in the author's space, with the same harness defaults
  and repo trust propagation as TUI reviewers. hc verdict delivery uses `hc send` during
  the migration; the `VEGA_GATE=skip` workaround for hc terminals is no longer needed.
  See [gate](docs/design/gate.md) and [hc migration](docs/design/hc-migration.md).

## 115 · 2026-11-06

- optional: faster builds. `scripts/install-cli.sh` builds every binary in one `cargo build --release`
  (no fat LTO any more) and copies them; `mise run install` skips `check` when it already passed on
  this clean tree. Dev builds keep line tables only (the old `target/debug` can be removed once:
  `cargo clean`). Worktrees compile through sccache (`mise install` fetches it; without it rustc runs
  unchanged); the main checkout builds incrementally. `mise run check` runs the tests with
  `cargo nextest` (`mise install` fetches it).

## 114 · 2026-11-06

- unit: `herdr-app.keys`, `herdr-app.settings`, `herdr-app.themes` (group `herdr-app`) own
  `~/.config/herdr-app/`: the TUI's ctrl-s prefix keys as chords (`ctrl-s ctrl-s` sends ctrl-s),
  Catppuccin latte/mocha themes, sounds off. Skipped without herdr-app. `vega config apply herdr-app`.
  See [config](docs/design/config.md) "herdr-app".

## 113 · 2026-11-05

- feature: `vega history read/export` filters central events by project, bead, host
  and timestamp using a separate viewer seed; export emits full JSONL. `vega history status`
  shows this host's last PubAck, pending count and publication error without
  privileged keys. See [history](docs/design/history.md).
- optional: metadata-only OTLP history logs use the shared `vega/otlp.toml` config;
  missing/disabled is off. Enable explicitly and restart vegad to send the closed
  metadata allowlist to the orrery collector's `/v1/logs`; bodies are excluded.

## 112 · 2026-11-05

- optional: vegad exports JetStream inbox capacity gauges through OTLP/HTTP; VEGA_HISTORY gauges
  require a separate stream-info grant, which generated host grants do not include.
  Export is off by default; set `enabled = true` and an `endpoint` in
  `~/.config/vega/otlp.toml` to enable it. It reports stream bytes, messages and max_bytes every
  five minutes.

## 111 · 2026-11-05

- feature: central history now records land attempts and phase outcomes (including
  resumes, validation, conflicts and timings), plus deploy machine commits and
  install/config unit evidence through the existing bounded outbox. History
  capture failures remain diagnostic and do not change command results.

## 110 · 2026-11-05

- feature: UAC repo groups use `.vega/group.toml` in a coordination repo.
  `vega uac group add <path|project-id>` and `group discover <dir>` manage members;
  claim registers their UAC routing, including locally on inbox-only hosts.
  The facility view includes member agents with project IDs. Use
  `vega uac land BEAD --repo <member-main-checkout> --branch BRANCH` to land there.

## 109 · 2026-11-05

- feature: `vega post log` without `--pane` waits for a cold archive scan instead of printing
  `0 messages`. Empty results use the broker's filtered pending count; a stalled first
  or later read fails with a broker error instead of an empty or short list.

## 108 · 2026-11-05

- unit: vegad preserves live registry entries when a herdr agent or workspace snapshot
  fails or is malformed; it logs `registry: off (...)` instead of reconciling an empty set.
- migration: central history needs operator provisioning and a larger broker store.
  Use temporary `auth-config --create history --create archive`, then `vega post provision history`
  and `vega post provision <host> --archive`; remove the grants and reload afterwards.
  Inbox age expiry becomes unlimited with a 10 GiB cap; VEGA_HISTORY retains events at capacity
  and rejects new ones. Hosts publish spawn/mux/mail/lineage events through a bounded outbox;
  inbox-only hosts may also publish their own history, with no history reads or new mail rights.
  See [history](docs/design/history.md) for capture coverage, limits and rollout.

## 107 · 2026-11-04

- feature: inbox-only hosts can use `vega post send` and `done` for local panes through herdr.
  `send --uac` resolves open local UAC claims and aliases; cross-host mail stays quarantined
  unless a receiver pair grant permits peer mail. No broker permission change.

## 106 · 2026-11-04

- migration: operators can enable registry message TTL with the temporary admin grant
  `auth-config --create registry-ttl` and `vega post provision registry --ttl`.
  Regenerate without the grant and reload immediately afterwards, including on failure.
  Enabling is irreversible on NATS 2.14.7; see [vegad](docs/design/vegad.md#one-time-ttl-enablement-vega-lhc246).

## 105 · 2026-11-04

- required: `vega mux spawn` now requires `--tier flagship|balanced|budget`, resolved
  through lanista's model pins for the agent kind (Pi uses zai). Explicit `-m`/`--model`
  after `--` and `--no-parent` are exempt. Update spawn scripts and briefs; mux show
  and the UAC agent table expose the recorded tier and model.

## 104 · 2026-11-04

- feature: `vega uac` and `vega janitor` show repeated vegad loop/phase budget overruns from
  local health evidence, with stale reports marked unknown. Slow daemon log lines are summarized
  at most once per ten minutes. See [vegad](docs/design/vegad.md#loop-budget-health-vega-lhc244).

## 103 · 2026-11-04

- migration: registry deletes use one-hour TTL purge markers. Enable `allow_msg_ttl` on
  `KV_VEGA_REGISTRY` to let dead subjects expire; otherwise vegad logs once and keeps
  ordinary deletes. Each host gradually cleans its own old tombstones once per daemon
  start. No host ACL change. See [vegad](docs/design/vegad.md#registry-m4).

## 102 · 2026-11-04

- migration: `claude.hooks` now installs the minimal `vega-hooks@vega-local` Claude Code plugin
  per host. Legacy vega hooks leave settings.json after the enabled plugin is verified; other hooks
  and seeded preferences survive. Restart running Claude sessions after deploy. See
  [config](docs/design/config.md#claude-hook-plugin-vega-lhc243).

## 101 · 2026-11-04

- feature: config apply retains 256 timestamped per-unit results and redacted managed-key
  operations in `~/.local/state/vega/config-evidence.json`. Deploy reports evidence from
  both its install-time hooks and final config apply, so a write count can be attributed.

## 100 · 2026-11-04

- feature: vegad preserves pending bounces across self-upgrades, including senders already notified
  and the close timestamp, so unread-mail notifications resume once the broker returns.

## 99 · 2026-11-03

- feature: the broker outage test's waits now cover the full probe backoff plus 20 s of headroom for
  a busy machine, and it always checks the sidebar renewal during an outage. A run takes about
  135 s, up from as little as 22 s, and `mise run check` grows with it. See
  [vegad](docs/design/vegad.md#broker-outage-vega-lhc205).

## 98 · 2026-11-03

- feature: vegad bounds herdr subprocesses and socket requests, logs timeouts and
  retries pending pointers. `vega uac` and `vega mux show` expose mail metadata as
  an age, `pending`, or `unknown`; missing metadata no longer appears blank or
  implies an empty inbox. See [vegad](docs/design/vegad.md#herdr-integration).

## 97 · 2026-11-02

- feature: operators can grant exactly one inbox-only agent pair temporary peer mail with
  `vega uac grant <sender-host>:<pane> <receiver-host>:<pane> --expires 2h` on the receiver.
  `revoke` ends it and `grants` lists live, expired and revoked pairs. The sender pane is its
  host's own claim; replies are never Slayer approval. No broker changes; see
  [vegad quarantine](docs/design/vegad.md#quarantine-inbox-only-replies).

## 96 · 2026-10-30

- feature: review overlays can send notes and load patches on local apps with strict Content Security
  Policy. CSP bypass is enabled only on the review tab before loading the app and lasts with its inbox;
  browser profile settings are unchanged. Restart an existing review to apply it.

## 95 · 2026-10-30

- feature: agent mail, notify and mux now use the host pinned in vegad.env when the shell has no
  VEGA_POST_HOST override. WSL panes use the same broker identity as vegad without shell setup.
  When that differs from the hostname, respawn existing agents to refresh their saved parent links.

## 94 · 2026-10-30

- migration: pi built-in themes are now found in standard fnm and npm/Homebrew installations even
  with vegad’s minimal PATH. Legacy `system/system` uses the default catppuccin pair, which requires
  `pi-community-themes`; alternatively set `[tools.pi] light/dark`. See [theme](docs/design/theme.md).

## 93 · 2026-10-30

- migration: pi now follows vega's mode through `~/.pi/agent/themes/vega-auto.json`, reloaded live
  without a restart. Existing light/dark pairs move into `[tools.pi]` in theme.toml; the default pair
  is catppuccin-latte/catppuccin-mocha. Install those source themes or set overrides. Select
  `vega-auto` once in already running pi sessions. See [theme](docs/design/theme.md).

## 92 · 2026-10-30

- feature: notify.toml routes asks by level and project, with one phone escalation, one reminder,
  quiet hours (22:30–07:30; only PagerDuty high breaks them), and notification-only Uber rerouting
  while engaged. Missing config stays toast-only. Phone adapters follow separately.
  See [notify](docs/design/notify.md).

## 91 · 2026-10-30

- feature: vegad delivers blocking asks on the host with the newest human input, with durable
  cross-host receipts and focus/draft suppression. No config uses toast; notify.toml also enables
  desktop notifications for action/urgent. `vega notify test toast|macos` probes local delivery.
  Phone channels, routing and timers follow separately. See [notify](docs/design/notify.md).

## 90 · 2026-10-30

- feature: the Claude `vega_uber_capture` hook resolves the current pane's open notify asks when
  the Slayer submits a typed prompt. Vegad pointers, task notices and mux mail keep asks open.
  Panes without the hook still use `vega notify resolve`. See [notify](docs/design/notify.md).

## 89 · 2026-10-30

- feature: `vega notify info|action|urgent` stores blocking operator asks; `list`, `resolve` and `ack`
  manage them across hosts with qualified IDs. Re-raising a key keeps its clock; pane/bead closure
  resolves on the origin. This build persists state only; delivery channels land separately. See
  [notify](docs/design/notify.md).

## 88 · 2026-10-30

- required: xai (Grok) is gone: the subscription was cancelled. The gate's reviewers are now codex,
  `pi:zai/glm-5.3` and claude, and the lanista seats drop the xai lane. Run `vega lanista install` to
  rewrite `~/.config/lanista/seats.json`, and clear any `vega lanista pin xai` override.

## 87 · 2026-10-28

- required: with a Dolt remote that has named users, run `vega post relay install` again: it pins
  `DOLT_REMOTE_USER` in `vegad.env`, and vegad's sync pull, fast-forward heal and push pass
  `--user` (they ran as `root` before). See [Beads sync](docs/design/beads-sync.md). Thanks to goern
  (Christoph Görn).

## 86 · 2026-10-27

- feature: `vega uber brief` (on the Uber-UAC) asks every live UAC for its FYI and Blocked lines
  (`uber.brief`, replies within 2 min) and `vega uber brief --show` renders the open decisions plus
  those lines in at most 25 lines. A UAC replies once, each line `FYI: ...` or `Blocked: ...`.
  `vega post ask --kind` sends a typed question. See
  [Uber-UAC protocol](docs/design/uber-uac.md#3-protocol-ask-answer-brief-over-vega-post).

## 85 · 2026-10-27

- feature: `vega gate start` skips a reviewer candidate whose harness binary is not on `PATH`
  (stderr names it) and, when a reviewer spawn brings up no pane, tries the next candidate. A
  failed spawn now shows its own cause (for example `command not found: codex`), not only the
  error header. See [gate.md section 4](docs/design/gate.md#4-vega-gate-start).

## 84 · 2026-10-26

- feature: the Uber-UAC is engaged only while the Slayer talks to it: each
  Slayer capture on the Uber host opens a 2 h window, and herdr pane focus on
  any host ends it (vegad publishes it as `human_at` in the beat). Outside the
  window `vega uber ask` returns `uber_absent` and vegad resends no ask;
  `vega uber status` prints `engaged: yes|no (reason)`. See
  [Uber-UAC singleton](docs/design/uber-uac.md#2-singleton-one-estate-wide-claim).

## 83 · 2026-10-26

- feature: vegad's sidebar no longer flickers on a loaded host: its tokens live 5 min and a thread
  of their own renews them; project identity is cached between refreshes and `git status` runs off
  vegad's loop. `vega mux spawn` starts agents at nice 10, below vegad. See
  [Sidebar cost](docs/design/vegad.md#sidebar-cost-vega-lhc212).

## 82 · 2026-10-26

- feature: on the Uber host, `vega_uber_capture` (Claude Code hook for
  `PostToolUse` AskUserQuestion and `UserPromptSubmit`) records the Slayer's
  input in `~/.local/state/vega/uber/captures.jsonl`; vegad pointers and task
  notices are dropped. `vega uber answer <tag> --capture <n>` sends an
  `uber.answer` built only from that capture, checked against the session's
  own Claude Code log, and refuses without one;
  `vega uber audit` prints every ask received, answered, sent, applied or
  refused. The hooks are not installed yet (`vega uber start`, vega-lhc.209.7).
  See [Uber-UAC trust rule](docs/design/uber-uac.md#4-trust-rule-and-guards).

## 81 · 2026-10-26

- feature: vegad verifies each `uber.answer` before the UAC sees it: an open
  ask, the claim it was sent to, the tag, pick and rendering, the L4 approval
  rule; the first answer wins. A verified one becomes a bead comment by Slayer
  (`via uber <id>`) and mail to the UAC; a refused one a note (`uber-refused`).
  `vega post inbox` prints these answers withheld. See
  [Uber-UAC trust rule](docs/design/uber-uac.md#4-trust-rule-and-guards).

## 80 · 2026-10-26

- feature: `vega uber ask --bead <id> ...` sends a UAC's `slayer-decision` bead to the live
  Uber-UAC and records the open ask in `~/.local/state/vega/uber/asks/`. Without a live Uber
  it returns `uber_absent` and changes nothing. vegad withdraws an ask when its bead closes and
  resends it after a new Uber claim. Answers are not applied yet. See
  [Uber-UAC protocol](docs/design/uber-uac.md#3-protocol-ask-answer-brief-over-vega-post).

## 79 · 2026-10-26

- feature: `vega post send --uber` mails the live Uber-UAC claim (`uber_absent`
  when none is live), and `--kind uber.ask|answer|applied|withdraw|brief` sends
  a typed JSON body after checking its schema and size cap; a bad body is
  refused and nothing is sent. The Uber-UAC itself is not usable yet; see
  [Uber-UAC protocol](docs/design/uber-uac.md#3-protocol-ask-answer-brief-over-vega-post).

## 78 · 2026-10-26

- feature: `vega uber claim|release|status`, the estate-wide singleton claim of
  the Uber-UAC (registry key `<host>.uber.v1`, claim file
  `~/.local/state/vega/uber/claim.json`). A live claim elsewhere refuses, a gone
  one is taken over, and of two racing claims the newest wins; the loser gets
  `uber_conflict`. Needs the broker. Nothing uses the claim yet. See
  [Uber-UAC](docs/design/uber-uac.md).

## 77 · 2026-10-26

- feature: after a rate limit or overload vegad no longer retypes the mail
  pointer every few seconds. It retries after 2, 5 and 15 minutes, then holds
  the pointer until new input (you type into the pane, or it reads its inbox);
  a pane without a parent gets a note. Also for panes that are no sentinel. See
  [Provider aborts](docs/design/vegad.md).

## 76 · 2026-10-25

- feature: `vega post identity rekey preview|apply|rollback` moves a project's
  records to its new ID after a remote was added or changed (UAC claim and
  status, badges, lanes, bd sync records, schedules, notes, PagerDuty receipts,
  this host's registry UAC and pins), with relay and writers stopped. Each
  rekey is a new journal generation; the flag-day journal and `approved.json`
  stay unchanged. No Slayer approval unless an inbox-only host has an exact
  grant for the old ID. Moving files by hand is not enough; see
  [Rekey after a remote change](docs/design/project-identity.md#rekey-after-a-remote-change).

## 75 · 2026-10-25

- feature: vegad no longer hangs when the NATS broker goes away. Within 10 to 20 s
  without the broker it switches to local mode (sidebar labels, badges,
  attention and wakes keep running), logs `broker: lost` once, shows a herdr
  toast and a note on the vega UAC's badge, and reconnects by itself with
  backoff (2 s up to 60 s), clearing the note. See
  [Broker outage](docs/design/vegad.md#broker-outage-vega-lhc205). A vegad
  already hung by the old bug cannot self-exec; the install restarts it after
  15 s (`relay status` shows it `stale` until then).

## 74 · 2026-10-25

- feature: vegad logs a project whose ID changed (first remote added, remote URL
  changed) once per transition instead of every pass, naming the registered and
  the current ID and the fix. A UAC re-claim leaves the old-ID records behind;
  remove them as in [Rekey after a remote change](docs/design/project-identity.md#rekey-after-a-remote-change).

## 73 · 2026-10-24

- feature: a host that missed the identity flag day (io) rejoins with a
  re-audited bundle that completed hosts accept without re-migrating; the UAC
  runs the [Late host rejoin](docs/design/project-registry-cutover.md#late-host-rejoin)
  runbook. `vega post identity verify --host <late>` checks that host's
  projection; plain `verify` stays the flag-day check (later UAC/pin changes
  fail it). Identity subcommands now load `vegad.env`, so a WSL host whose
  `hostname -s` differs from `VEGA_POST_HOST` previews under vegad's host name.

## 72 · 2026-10-24

- feature: a vegad in identity quiesce (e.g. `relay status` says `identity
  quiesced (… unplanned live host: io …)`) still labels its host's panes in
  herdr's sidebar: `$host`, `$kind`, `$project` (the workspace label) and
  `$label`. Before, every agent row of that host showed only its state icon.
  UAC badges, sentinel names and `$git` still wait for the identity. See
  [Sidebar label](docs/design/theme-symbols.md). Takes effect after the vegad upgrade.

## 71 · 2026-10-23

- feature: `vega bd init` (and `--adopt`) authenticate against a Dolt remote with a
  named user: set `DOLT_REMOTE_USER` next to `DOLT_REMOTE_PASSWORD` and the remote
  fetch and push pass `--user` (dolt itself defaults to `root`). See
  [Beads sync](docs/design/beads-sync.md). Thanks to goern (Christoph Görn).
- feature: `mise run install` no longer fails where the VS Code CLI `code` is not
  on PATH (e.g. WSL): it skips the extension with a hint to install
  `vscode/dist/vega-vscode.vsix` by hand. Thanks to goern (Christoph Görn).

## 70 · 2026-10-23

- feature: vegad's draft guard knows pi's input box: a pointer no longer joins
  text you are typing in pi (it waits, and a single-line draft left 5 minutes is
  stashed and typed back). A screen without a known input box (another harness)
  now holds the pointer until it has been unchanged for 20 s instead of typing at
  once. `vega mux send --now` still pastes directly. See
  [Draft guard](docs/design/vegad.md). Takes effect after the vegad upgrade.

## 69 · 2026-10-23

- feature: `vega mux spawn --kind codex` also sets `HERDR_PANE_ID` (and a local
  `HERDR_SOCKET_PATH`) in Codex's tool shell environment policy, so projects with
  `inherit = "core"` keep the pane identity. Running Codex agents get it after a respawn.
- feature: `vega ci watch`/`unwatch` find the caller's pane through the process tree
  (`HERDR_PANE_ID` only a hint, so a missing or stale value cannot misroute verdicts) and otherwise fail with `not_in_herdr`. `--for <pane>` still mails any
  pane; the hint now says to pass your own, since passing the UAC's pane mailed verdicts
  to the wrong inbox.

## 68 · 2026-10-23

- unit: `codex.hooks` matches `request_user_input|request_user_input_async`.
  A headless child's async Codex question goes to its parent like the
  blocking dialog; a headed child's question stays in its bar and the parent
  gets a copy by mail. Run `vega config apply codex.hooks`, then trust the hook
  once in a new codex session ("Hooks need review": the matcher changed). See
  [Codex async questions](docs/design/pane-control.md#codex-async-questions-vega-lhc190).

## 67 · 2026-10-22

- required: Private service endpoints no longer have compiled defaults. Before
  upgrading estate hosts, preserve their current endpoints in local config:
  `vega/nats.url`, `vega/bd.remote`, `vega/bd.sql`, and
  `vega/janitor-embed.url` under `$XDG_CONFIG_HOME` (normally `~/.config`).
  Existing environment overrides still win. See the
  [endpoint migration](docs/design/config.md#service-endpoints).
- migration: The agentsview sidecar is now a generic example. Keep personal
  setup instructions in the local `vega/config/agentsview/LOCAL-SETUP.md` source
  before applying it. Caveman style names remain available with original vega
  wording replacing text whose provenance could not be verified.

## 66 · 2026-10-22

- migration: identity cutover accepts legacy integer status timestamps and
  migrates inbox-only hosts' local records without publishing them. Grants
  audit accepts inbox consumer permissions while still rejecting registry
  consumers. Separate Git directories need `core.worktree` when mappings start
  from the Git directory; resolver errors now identify that repair. Rehearse
  startup/file rollback on state copies as described in the
  [runbook](docs/design/project-registry-cutover.md#rehearsal-on-state-copies).

## 65 · 2026-10-22

- feature: `vega post identity mappings --output mappings.json` generates the
  owning host's preview inputs from local references and owned registry UACs/pins.
  Resolve reported paths before preview; source records stay unchanged. See the
  [operator runbook](docs/design/project-registry-cutover.md#operator-runbook-146).

## 64 · 2026-10-22

- migration: project identity now has a durable startup journal and rollback.
  Stop writers on all hosts, run `vega post identity preview` and `audit`, then
  deploy the audited bundle together. Vegad migrates before workers start,
  verifies v2 publication and exact inbox reads, and resumes interrupted plans.
  Unapproved offline hosts and mixed versions are refused. Only orrery changes
  grants, with the Slayer's approval. The first migration needs the audited
  approved.json and a reachable broker: until it completes, offline local-mode
  jobs stay off, so run the online cutover once before using offline mode. See the
  [operator runbook](docs/design/project-registry-cutover.md#operator-runbook-146).

## 63 · 2026-10-22

- migration: lanes, notes, badges, PagerDuty ownership, vegad membership/decisions
  and upgrade UAC discovery now join by project ID while execution uses local
  locations. Conflicting committed ownership across clones blocks routing.
  Local migration plans preserve trackers, timestamps, notification causes and
  delivery receipts for the single startup journal. See
  [the remaining-consumer handoff](docs/design/project-local-consumers-cutover.md).

- migration: bd and Git sync records now use project IDs plus separate local
  clone locations. Status shows locations; resume requires a local path when
  an ID/database matches several clones. The bd shim keeps its Dolt guard and
  sync hints. This is part of the coordinated `identity` branch cutover, not
  a standalone upgrade. See [the local sync handoff](docs/design/project-local-sync-cutover.md).

## 62 · 2026-10-22

- migration: registry UAC routing and lanista project pins now use v2 remote
  project IDs and reversible keys. This requires the coordinated all-host
  project-identity cutover, migrated local UAC state, backed-up pins and exact
  inbox-only read grants. The UAC keeps steps .14.4–.14.6 on the `identity`
  integration branch until complete; do not land or deploy this step alone. See
  [the cutover handoff](docs/design/project-registry-cutover.md). No dual protocol
  is supported; the UAC performs deployment after the startup migration is ready.

## 61 · 2026-10-22

- feature: The bd shim removes its own directories from child PATH, skips links to itself,
  and blocks recursive wrapper invocation. Metadata probes time out after two seconds,
  so a stuck helper cannot indefinitely delay bd; the original bd command remains unbounded.

## 60 · 2026-10-22

- feature: `vega bd init --database` accepts hyphenated names such as `oparl-lite-2`,
  including with `--adopt`. Explicit names are preserved locally and remotely; defaults
  keep replacing repo-name hyphens with underscores.

## 59 · 2026-10-22

- optional: `dcg` is recommended for destructive command protection. Onboard checks
  its binary and PreToolUse registration for present Claude/Codex agents and gives
  installation/repair guidance; it never installs dcg or its hooks automatically.
- unit: `dcg.config` manages dcg's config.toml, including a blocking branch-delete
  reason pointing to `vega uac prune` or `wt remove`. `git branch -D` stays blocked.
- unit: `dcg.packs` manages custom YAML packs, including the existing oc.yaml
  unchanged. Both units support local sources; allowlist.toml stays user-owned.
- migration: If chezmoi still owns a dcg target, release it in the dotfiles before
  an explicit config apply; `vega config diff dcg.config dcg.packs` shows ownership
  and the proposed changes. Existing files stay untouched until that handoff.

## 58 · 2026-10-21

- feature: `vega uac prune [--dry-run]` cleans ancestry-merged local branches
  and clean paneless merged worktrees. Unmerged and squash-merged branches,
  dirty trees and live panes remain residue. Removal never forces or approves hooks.

## 57 · 2026-10-21

- feature: `vega ci watch` checks GitHub workflow runs for the head before
  reporting green. Queued, running and approval-waiting workflows keep it
  pending even when external checks pass; `vega ci show` includes runs.
  All watchers share one extra conditional request per head per poll at most.

## 56 · 2026-10-21

- feature: hygiene lists open Slayer decisions after 24 h instead of seven days.
  The UAC records chat answers and immediately closes or updates their decision
  beads, keeping the board and sidebar aligned with chat.

## 55 · 2026-10-21

- feature: `vega uac land` accepts beads closed by the completed sentinel after
  verifying the closure audit actor, preserving the close reason and appending
  the landing receipt to notes. Reclaim uses valid conditional bd flags, and
  beads already held by the UAC proceed without reclaim.

## 54 · 2026-10-21

- feature: `vega mux reopen <name|pane>` respawns a closed Claude or Codex sentinel
  with its saved session resumed, in the same cwd (worktree or plain directory),
  preserving its parent and mode. New spawns record session identity; close refreshes
  and keeps it. Keep the directory if you want to reopen; local machine only.

## 53 · 2026-10-21

- feature: `vega orchestration waiting` records durable waits; `clear` requires
  non-empty evidence text and is idempotent. Remote updates use authenticated post
  commands and verified waiter/coordinator relationships. Waits stay in local
  runtime state, never in git. Unanswered post asks survive gather deadlines;
  idle sentinels owing replies raise one existing attention transition, with
  headed questions kept with the Slayer. Abandoned question metadata expires
  seven days after its gather deadline; post messages remain available.

## 52 · 2026-10-21

- feature: `vega orchestration` shows project waits from existing questions,
  attention, parked decisions and CI receipts, alongside bead readiness.
  Missing sources remain unknown; `vega skills orchestration` serves the
  supervised-coordination guide. This view does not schedule or send mail.

## 51 · 2026-10-21

- feature: `vega uac land BEAD --branch BRANCH` performs resumable reviewed
  landing with project merge/check/install policy in `.vega/land.toml`, exact
  integrated-head validation, verified push and completed sentinel cleanup.
  Inspect residue with `--status`, continue with `--resume`, retain the sentinel
  with `--keep`, or choose an integration branch with `--target`. Stashes and
  branches stay intact. Landing runs the project's optional install argv, adds
  no deploy step of its own, and never grants hook approval.

## 50 · 2026-10-21

- feature: `vega mux spawn --kind codex` sets the caller's `SSH_AUTH_SOCK` in
  Codex's tool shell environment policy, so projects with `inherit = "core"` keep
  the socket for SSH authentication and signed commits. `--env SSH_AUTH_SOCK=...`
  and explicit Codex config arguments win; other policy fields and manually
  started panes are unchanged. An `include_only` list must still allow the socket.

## 49 · 2026-10-21

- feature: shared project identity resolver normalizes fetch remotes and keeps
  canonical local checkout locations separately, including linked worktrees and
  bare repositories. This adds the foundation for the approved flag-day cutover;
  registry, routing and sync consumers migrate in the following steps.

## 48 · 2026-10-20

- feature: `vega mux spawn` defaults to headed: the user answers questions in the pane.
  Add `--headless` for unattended children whose questions go to their parent.
  `spawn --resume` preserves the choice; older children without a valid mode record
  remain headless. Inspect or change it with `vega mux mode`.
  Inbox-only targets ask the user in either mode because their mail cannot reach the parent.

## 47 · 2026-10-20

- feature: `vega mux spawn --help` explains Codex's shell environment filtering:
  forwarding `SSH_AUTH_SOCK` to the pane cannot override a project's `inherit = "core"`.
  Configure the project's inheritance or explicit socket setting for SSH-signed commits;
  vega preserves the project's environment policy.

## 46 · 2026-10-20

- feature: `vega mux spawn` forwards the caller's `SSH_AUTH_SOCK` to every agent kind
  and placement, including worktrees, so SSH authentication and signed commits use
  its agent socket. `--env SSH_AUTH_SOCK=...` overrides it, including an empty value;
  use a path valid on the target host for remote spawns.

## 45 · 2026-10-20

- feature: `vega post` counts only fast replies toward its eight-hop loop limit. A reply at least
  120 seconds after the answered message resets the counter on the same thread. Inbox reply commands
  now carry a timestamp; upgrade sending hosts before using them. At the limit, stop automatic replies
  and tell the user; a substantive reply after work can resume with the same command.

## 44 · 2026-10-20

- feature: `vega post send --uac <project>` accepts `.bare` layout folder names and origin
  repository names, so `spellkave/.bare` resolves as `spellkave` and a clone in `orrery/main`
  can resolve as `orrery`. Matching multiple live UACs reports the candidates. Origin names
  require updated vegad on the UAC's host and one registry reconcile; full keys and folder
  names remain supported.

## 43 · 2026-10-20

- feature: gate starts a fresh reviewer after new commits, closing the old review instead of moving
  its clone by mail. Verdicts stay bound to the original head and generation. After new commits run
  `vega gate start` again; never re-point a reviewer by mail.

## 42 · 2026-10-20

- feature: gate reviewers prefer other harnesses, then allow a different model on the author's harness.
  Unknown author models still exclude the whole harness. Reviewer prompts pin target and base SHAs so
  a shared clone's remote refs cannot misdirect the review.

## 41 · 2026-10-20

- feature: `vega mux spawn --kind codex` defaults to `FORCE_COLOR=1` (basic ANSI colours),
  keeping the composer readable across live light/dark appearance changes despite Codex's cached
  startup background ([upstream issue](https://github.com/openai/codex/issues/54159#issuecomment-6294011108)).
  Pass `--env FORCE_COLOR=...` to override it.

## 40 · 2026-10-20

- feature: vegad on `proteus` polls PagerDuty every 15 minutes using the `pdc` secret,
  mails the owning UAC on any host, and reminds every four hours while an incident is open.
  Commit `.vega/pagerduty.toml` with `services = ["Orrery Estate"]` (names or IDs) in each
  owning project known locally to the poller. Unmapped services, duplicate owners and missing
  UACs produce board inbox notes. State survives restarts; a crash can cause a duplicate mail.
  To choose another polling host, set `~/.config/vega/pagerduty-poller` consistently on all hosts.
  Agents may investigate, acknowledge and add notes; resolve, escalate and reassign require
  a Slayer-approved board decision. See `docs/design/pagerduty.md` for setup and rollout.

## 39 · 2026-10-20

- unit: `herdr` removes vega's `prefix+d` and `prefix+shift+d` overrides. Apply the updated
  config unit to restore herdr's defaults: `prefix+d` unbound, `prefix+shift+d` closes the
  workspace. Use herdr's goto overlay (`prefix+g`, then `d`) to list done agents across
  machines and switch to one. `vega mux next` remains available as a command.

## 38 · 2026-10-20

- feature: `vega deploy apply <machine>|--all --from-branch <branch>` installs a pushed branch,
  defaulting to `main` when omitted. The target stays on that branch; deploy without the flag to
  return to main. Divergent branches fail before install. Gitsync skips main's fast-forward while
  no checkout has main checked out; deploy status still compares with main.

## 37 · 2026-10-20

- unit: `herdr` tightens the spacing after harness glyphs in the machines/spaces sidebar.
  Apply the updated config unit and update vegad on each host: workspace glyphs now use compact
  Unicode text symbols with the same harness colours.

## 36 · 2026-10-20

- feature: `vega lanista pin <lane> --tier <tier> --set <model>` shares a per-project model
  override through the registry, expiring at the provider's binding quota reset. `--until 2h|never`
  overrides expiry; `--clear` returns to global seats. Resolution names the source host; unavailable
  registry reads visibly use global seats. Gate's tier lookup uses this resolver. Normal hosts need
  no broker changes; inbox-only hosts use global seats unless granted exact read keys.

## 35 · 2026-10-20

- feature: herdr's machines/spaces sidebar shows each workspace's distinct harness glyphs beside its
  UAC/sentinel badge, with the same colours as the agents section. The UAC or sentinel kind comes
  first; remote workspaces use their own host's vegad metadata. Apply the updated `herdr` config unit
  and update vegad on each host to show them.

## 34 · 2026-10-20

- feature: lanista and pi capacity handling no longer offer the `openai-codex` lane. Pi uses xai or zai;
  Anthropic and OpenAI reviews use the native claude and codex harnesses.

## 33 · 2026-10-20

- feature: a `vega gate` reviewer candidate may pin a model as `<kind>:<lane>/<model>`, avoiding a lanista
  tier pin while retaining author exclusion and capacity checks. Vega's own XAI reviewer now uses
  `pi:xai/grok-4.7` instead of the budget pin `grok-4.3`; its fallback is now `pi:zai/glm-5.3`, while
  Anthropic and OpenAI use their native claude and codex harnesses.

## 32 · 2026-10-20

- feature: after a red `vega ci watch` verdict, fix and push immediately. If a repository blocks that
  push because the red run still continues, cancel it with `gh run cancel <id>`, then push; do not wait
  or poll. vegad now records a sentinel with an owed CI verdict as `waiting on ci: <targets>`, rather
  than `silent`.
- feature: vegad runs `vega gate gc` with its five-minute janitor tick, removing gate-review clones left
  by crashed reviewers. A GC failure is logged once and never stops the daemon; a later successful tick
  resets the failure notice.

## 31 · 2026-10-20

- feature: `vega ci watch` mails two more GitHub signals, once each: a PR that conflicts with its base
  (or is behind a base that requires up-to-date branches) gets "rebase before you push again", and a
  zombie run (still queued or in progress, all jobs completed for 15 min) gets the exact force-cancel
  command; vega never cancels. `vega ci show` lists the merge state and zombie runs. Cost: the merge state
  rides on the PR request already made; the zombie check adds one conditional request per poll once the
  checks are not pending, and a jobs request per open run every 15 min.

## 30 · 2026-10-19

- feature: `vega gate` reviewers start without help: `vega mux spawn` waits until codex has left
  `Starting` before typing a brief and presses Enter while the brief still sits in the input box (codex
  dropped the Enter at startup). One verdict sends one mail, from `tool vega-gate`, with the findings
  inline and the next step by role (a sentinel names the verdict in its done report; others push again).
  `vega gate` shows each reviewer's state and flags one that never took its prompt or sits at a dialog.
  New `vega mux spawn --no-parent` for agents that report by their own command (gate reviewers use it).

## 29 · 2026-10-19

- feature: `vega deploy apply` starts a stopped vegad on the target (`vega post relay start`), so the host
  ends at `relay=current`; before, a stopped vegad stayed down until started by hand.

## 28 · 2026-10-19

- feature: `vega gate`: an LLM review before an agent pushes, in repos with a `.vega/gate.toml` (paths to
  rubric, read from the push target). A new agent-only `git` shim fails `git push` until each segment of the
  push (direct commits, each merged branch) has a recorded pass; it starts a reviewer of another kind and
  model in a tab `gate-<branch>`, whose verdict arrives by mail. Records are beads with label `gate`
  (`vega gate log`). `VEGA_GATE=skip` skips visibly; gate errors let the push through with one stderr line.
  Every other git command is unchanged. Sentinels in gated repos run `vega gate start` before their done
  report. `vega skills gate` has the guide.

## 27 · 2026-10-19

- feature: `vega mux spawn --env K=V` (repeatable) sets env for the agent and its tool shells (claude, codex,
  pi); it wins over the default `BEADS_ACTOR`. `vega lanista pin claude|codex --tier <t>` answers for the
  claude and codex CLIs (claude budget: claude-haiku-5-5).

## 26 · 2026-10-19

- feature: `vega review patch <file.js>` live-patches the reviewed page for design feedback, without a code
  change: the overlay re-runs the patch after reloads, navigation and re-renders (`--list`, `--clear`).
  `vega review shot [--selector <css>]` takes a screenshot cropped to an element. `vega skills review` has
  the loop and its gotchas (Tailwind JIT, HMR).

## 25 · 2026-10-19

- unit: `herdr`: prefix+d runs `vega mux next` (the agent that has been done longest, on any host),
  prefix+shift+d `vega mux next --status blocked,done`. vegad keeps the queue itself, so a tab visit or a
  herdr restart no longer drops it; an agent leaves it when focused, working again, answered or gone.
  On another host vega focuses it there and shows "switch with prefix+w": herdr cannot switch your client.
  `~/bin/herdr-next-done` is no longer used.

## 24 · 2026-10-19

- feature: ganymede's vegad builds the lanista model catalog daily (`lanista fetch` with the lanista
  secrets) and puts it into the NATS object store `VEGA_LANISTA`; every other vegad copies a newer one
  every 15 min. `vega lanista` shows the catalog's age and source host. Override the builder with
  `VEGA_LANISTA_BUILDER` or `~/.config/vega/lanista-builder`.
- optional: until the broker grants the bucket, receivers log `check failed` every 15 min. Once: `vega
  secrets set artificial-analysis --hosts proteus,ganymede`; `vega post auth-config … --create lanista
  --lanista-writer ganymede`, reload nats, `vega post provision lanista`, then `auth-config` without
  `--create lanista` and reload again (docs/design/lanista-catalog.md).

## 23 · 2026-10-19

- feature: `vega janitor search <words>` finds checkouts by their README, offline (FTS5); after `vega
  janitor embed` (needs the secret `triton-embedding` for tool janitor) it also matches by meaning.
  `vega janitor history <name>` lists what changed in a checkout; `list --stale` shows since when and
  when it was last dirty.

## 22 · 2026-10-19

- feature: `vega janitor` keeps its index in SQLite (`~/.local/state/vega/janitor/janitor.db`; the first run
  imports the old `index.json`, no rescan) and `find`/`list` answer across hosts: vegad publishes each
  host's checkouts and pulls the other hosts' every 5 min, shown with a host column and their age. Needs
  the new `vega-janitor` and a restarted vegad on every host (`vega deploy apply`).

## 21 · 2026-10-19

- feature: `vega bd resume --project` takes every form of a project: the key `vega bd status` prints, `~`
  expanded by the shell (absolute), a trailing `/`, the dir without `.git`/`.bare`, a checkout dir, or the
  database (`VEGA`). vegad no longer stops a project on a pull that failed without a conflict
  (`cannot merge with uncommitted changes`, `invalid connection` after bd restarted its server, a
  remote's `http status: 400`, `behind a fast-forwardable remote tip`): it pulls again at once (the
  last one by a fast-forward), then on the next interval. A project an older vegad stopped that way
  resumes by itself once the new vegad runs.

## 20 · 2026-10-19

- feature: vegad keeps git checkouts fresh: every 5 min it fetches `origin` of each beads-synced or
  UAC-claimed project, and at most every 15 min fast-forwards the checkout that has the default branch
  checked out when its tracked files are clean and it has no local commits; otherwise it skips and says
  why. Worktrees are never touched. `vega bd status` shows the `git` table; in a checkout that is behind,
  `bd` prints one stderr line saying so, as it does when the beads sync stopped.

## 19 · 2026-10-19

- unit: `herdr`: the agent kind glyph takes one cell, like the host symbol (vegad puts a U+2800 blank after
  a Nerd Font glyph: Ghostty draws one two cells wide before whitespace, and herdr trims spaces from token
  values), and wears its kind's colour: claude terracotta, codex green, copilot purple, pi blue, others
  grey, overridable under `[symbols.colors]` (`claude`, `codex`, `copilot`, `pi`, `other_agent`).
  `vega config apply herdr` takes it; a copy under `~/.config/vega/config/herdr/` needs the repo
  template's `$kind` entry (`{{color.other_agent}}`, `{{kind_rules}}`).

## 18 · 2026-10-18

- unit: `herdr`: every agent row in the sidebar starts with its agent kind's glyph (`$kind`: claude, codex,
  copilot, pi, else a robot), UAC and sentinel rows included; plain agent rows show the glyph and the tab
  instead of `claude 1`. `vega config apply herdr` takes it; a copy under `~/.config/vega/config/herdr/`
  needs `"$kind"` first in the agents' second row. The claude and codex glyphs need Nerd Fonts >= 3.4, else
  override them under `[symbols.icons]` (docs/design/theme-symbols.md).

## 17 · 2026-10-18

- feature: `vega post send` to a closed pane fails with `pane_gone` and names the project's UAC instead of
  mailing into the void; a host whose registry entry is stale gets the mail with a warning. vegad bounces
  mail a pane closed without reading once to each sender. `vega post send --uac <project> "<text>"` mails a
  project's UAC on whichever machine it runs.

## 16 · 2026-10-17

- feature: vegad mails each UAC about stale work: once about a headed pane that has waited on you over 24 h
  (the UAC records the question as a decision bead and closes the pane), and at most daily a list of
  decisions older than 7 days, panes, mail and notes waiting over 24 h, which the UAC asks you about once.

## 15 · 2026-10-17

- feature: each UAC's notes badge counts only its project's notes; host-level ones (quarantine, drafts)
  count on the vega UAC. A beads sync stop note marks itself read once the project syncs again.

## 14 · 2026-10-17

- feature: in projects vegad syncs (`vega bd init`), the bd shim skips `bd dolt push` and `bd dolt pull` in
  agent sessions (exit 0 and a hint; exit 1 with the reason while the sync is stopped). `VEGA_BD_MANUAL=1`
  runs them anyway.
- feature: `mise run install-cli` (and so `vega upgrade` and `vega deploy apply`) restarts a running vegad that
  has not switched to the new binary by itself within 15 s; a stopped vegad stays stopped.

## 13 · 2026-10-17

- feature: `vega board` dismisses a stale decision: `x` on a decision in the Inbox asks for a one-line reason
  and closes the bead (`dismissed by the Slayer: <reason>`), no answer to the UAC needed.

## 12 · 2026-10-16

- feature: vegad runs without a NATS broker (local mode): the herdr sidebar, sentinel attention and the daily
  upgrade check keep working, and the upgrade proposal is typed into your vega session, which asks you before
  `vega upgrade`. Cross-host mail, registry, beads sync and CI verdict mail stay off until a broker answers.
- migration: without a broker, run `vega post relay install` once now (earlier onboarding said to skip it).

## 11 · 2026-10-16

- feature: `vega capacity` shows and binds every provider and window token-burn reports, without a vega
  release; a new `scope` column marks model-specific windows (`model:fable`), which never bind. They come
  from the config source `capacity/windows.toml` (override: `vega config init capacity/windows.toml`).
  copilot now binds too and shows in `vega uac status`; any provider in the sample takes ledger entries.

## 10 · 2026-10-16

- feature: `vega capacity` shows every subscription window per provider from token-burn (the binding one
  marked, forecast run-out), the resets ledger and token-burn's health. The ledger commands (`resets`,
  `plan`, `used`, `override`) moved there; `vega uac capacity` is an alias.
- optional: `token-burn` (the subscription quota monitor) for `vega capacity`, the capacity line of
  `vega uac status` and the `vega mux spawn` warning; without it capacity is unknown.

## 9 · 2026-10-16

- feature: `vega ci watch --run <id>` and `vega ci watch --workflow <file> [--ref <ref>]` mail the verdict of
  a GitHub Actions run you dispatched (`gh workflow run`); a PR or branch watch can miss it. The gh shim
  hints at it after `gh workflow run`.

## 8 · 2026-10-15

- feature: `vega upgrade` pulls your vega checkout (fast-forward only), installs it, upgrades moshi-hook where
  installed and shows what changed; then run `vega onboard apply`.
- feature: vegad checks once a day whether your vega checkout has new commits and tells your vega session
  (an agent pane in the checkout), or shows `vega ↓N` on its herdr workspace.
- optional: `moshi-hook` (phone notifications and approvals through Moshi, Homebrew); `vega upgrade` upgrades
  it, restarts its service and runs `moshi-hook doctor`.
- migration: run `mise run install-cli` once in your vega checkout so vega records where it was installed
  from; without that record vegad does not check and `vega upgrade` refuses.
- unit: `herdr`: the workspace sidebar shows `$upgrade` (`vega ↓N`); `vega config apply herdr` takes it.

## 7 · 2026-10-15

- feature: `vega onboard` walks a host through the infra vega needs and what changed since its last look;
  `vega onboard apply` writes the defaults without replacing your own files.
- feature: `vega config infra` lists every component, required or optional, with its next step.
- required: `mise`, `cargo`, `git`, `vega` on PATH, `herdr` and an `agent` (Claude Code, Codex or pi) are
  the core; everything else is optional.
- optional: `chezmoi` must not manage vega's config targets, or `vega config apply` leaves them alone.
- unit: `vega config` owns the agents' config: `claude.md`, `codex.md`, `pi.md`, `claude.styles`,
  `claude.settings`, `claude.hooks`, `codex.hooks`, `pi.prefix`, `skills`, `agentsview`. Local overrides
  go to `~/.config/vega/config/` (`vega config init <source>`).
- migration: `vega theme herdr` and `vega mux hook install` are aliases of `vega config`; edit sources,
  never the targets.

## 6 · 2026-10-13

- unit: `herdr`: vega renders herdr's `config.toml` (sidebar rows, host labels, badges from `theme.toml`).

## 5 · 2026-10-12

- feature: `vega theme` passes the OS dark/light mode to every TUI; headless hosts follow the control host.
- feature: `vega open` jumps to a selected pane id; the board gained an Inbox tab and mouse support.

## 4 · 2026-10-11

- optional: `dolt` and a Dolt remote: `vega bd init` syncs a project's beads; without them beads stay local.
- optional: `op` (1Password CLI) for `vega secrets adopt`; secrets are age-encrypted per host.
- optional: `skills.sh` (`npx skills`) for `vega skills lib`, the global skills library.
- feature: `vega skills` (plural) lists the plugin guides; `vega skills <name>` loads one.

## 3 · 2026-10-10

- optional: `gh` and `tea`: `vega ci watch` mails the CI verdict; the gh shim denies `gh run watch`.
- optional: `uv` for `vega lanista`, the model catalog.
- optional: `agent-browser` for `vega review`, live review in a headed browser.
- feature: `vega janitor` maps every git checkout; vegad is started by herdr, no launchd or systemd.

## 2 · 2026-10-09

- optional: `bd` (beads) for `vega board` and the UAC's decisions.
- optional: `wt` (worktrunk) for `vega mux spawn --worktree`.
- feature: agent-only shims (`shims/activate.sh`) and the ask-parent hook: a spawned agent asks its
  parent, not the human in its pane.
- feature: `vega uac`, coordinator mode, and `vega board`, its decisions in a herdr pane.

## 1 · 2026-10-07

- required: `herdr`: `vega mux spawn`, `send`, `ask` run and connect agents in herdr panes.
- optional: `nats`: `vega post`, mail over a NATS broker (`VEGA_NATS_URL` or `~/.config/vega/nats.url`),
  and vegad, one relay per host. Without it `vega mux send` pastes directly on the local machine.
- optional: `tailnet`: the estate's broker, Dolt remote and `vega deploy` to other herdr machines.
