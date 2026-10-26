# Changelog

What an occasional user of vega must know after an upgrade: new infra, config units, migrations and
notable commands. `vega onboard` shows the blocks newer than the one this host saw last; the rules are in
[docs/design/onboarding.md](docs/design/onboarding.md). Newest block first; never edit a block that is on
main, add a new one. Entry kinds: `required`, `optional`, `unit`, `migration`, `feature`.

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
