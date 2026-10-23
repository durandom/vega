# Orchestration

Status: approved survey-driven slices (.166.5, .166.9, .166.10); attempts deferred. 2026-10-21.

Add `vega-orchestration`, a waiting/ownership view and coordinator guide first.
A supervised-attempt ledger is a conditional later extension, not the MVP.
Keep tasks in beads, mail in post, terminals in mux, and project authority in the
UAC. Ordinary handoffs still use `vega mux spawn`; explicit supervision opts into
attempt accounting. This is not an automatic scheduler.

## Source and inventory

Read-only source: `/Users/mhild/src/durandom/vega/repos/orca`, stablyai/orca commit
`907cb6e6038efda0fd1ee1975a3787200d0801b6`. Orca was not installed or run.
Paths prefixed **O/** below are relative to that checkout; **V/** means this repo.
The stub is **O/skills/orchestration/SKILL.md**. The real guide is
**O/skill-guides/orchestration.md**, with conditional documents under
**O/skill-guides/orchestration/references/** (abbreviated **R/**).
**O/config/scripts/generate-bundled-skill-guides.mjs** bundles these into
**O/src/cli/bundled-skill-guides.ts**; **O/src/cli/handlers/skills.ts** serves them.

| Orca feature and evidence | Vega today and actual gap |
| --- | --- |
| Threaded send/reply, dispatch addresses, groups; **R/messaging-and-gates.md**, **O/src/cli/handlers/orchestration/message-inbox-handlers.ts** | `vega post send`, timestamped `--re`, archive and cross-host addressing already exist (**V/plugins/post/src/main.rs**, `send`, `next_hop`, `log`). Missing stable attempt address; groups are optional convenience, not a prerequisite. |
| Blocking `ask`, reply by message ID; timeout resumes the same question (**R/worker-contract.md**, **R/messaging-and-gates.md**) | `vega post ask --to ... --within ...` gathers replies asynchronously on one thread (**V/plugins/post/src/gather.rs**). It returns immediately; it is not a blocking ask or a durable resume-by-question-ID API. Keep asynchronous questions; no shell wait needed. |
| Task + authoritative Dispatch, injected worker contract; **R/worker-contract.md**, **O/src/cli/handlers/orchestration/dispatch-handlers.ts**, **O/src/main/runtime/orchestration/coordinator-task-dispatch.ts** | `vega mux spawn --prompt-file ... --worktree ...`, parent/adopt and brief timestamps exist (**V/plugins/mux/src/main.rs**, **V/crates/vega-herdr/src/state.rs**). No explicit bead-to-attempt identity across pane reuse, retry and remote reports. |
| Typed `worker_done`, escalation/question waits and FIFO Delivery replay until explicit ack; **O/skill-guides/orchestration.md**, **R/messaging-and-gates.md**, **O/src/cli/handlers/orchestration/message-check-handler.ts** | `vega post done --outcome`, vegad attention/wakeups, stale-report suppression, `inbox --keep` and `--show` already exist (**V/plugins/post/src/main.rs**, **V/plugins/post/src/supervise.rs**, **V/plugins/post/src/delivery.rs**). Default inbox acks on read; no durable application settlement. Latest done replaces earlier done. Missing idempotent attempt settlement and processed-event cursor, not notification transport. |
| Task DAG and ready waves; **R/coordinator-loop.md**, **O/src/cli/handlers/orchestration/task-handlers.ts** | `bd dep add`, `bd ready`, `bd blocked` already own durable ordering. Missing joined view of ready beads and active attempts; no second DAG needed. |
| Decision gates block Tasks; **R/messaging-and-gates.md**, **O/src/cli/handlers/orchestration/gate-handlers.ts**, **O/src/main/runtime/orchestration/coordinator-decision-gates.ts** | Decision beads, deps, `vega uac answer`, `park`/`unpark` and wake-on-close already cover decisions (**V/plugins/uac/help/uac.md**). `vega gate` is diff review, a different gate (**V/docs/design/gate.md**). Need a documented recipe tying decisions to dependent beads, not another approval database. |
| Coordinator loop: launch independent wave, consume questions/completions, settle each terminal; **O/skill-guides/orchestration.md**, **R/coordinator-loop.md**, **R/recovery-and-cleanup.md** | `vega skills`, UAC role, registry, vegad wakes and landing rules already cover supervision (**V/docs/design/uac-role.md**, **V/docs/design/uac-rules.md**). Missing compact reusable supervised-loop guide and explicit unresolved-attempt/cleanup accounting. Orca's old `coordinator-start/stop` commands are retired errors in its dispatch handler; do not infer an active autonomous scheduler from old runtime code. |

## Usage (survey)

UAC reports (post messages 5434 and 5438, 2026-10-20): six projects answered.
These are reported needs, not reproduced bugs.

| Project | Observed use and requested help |
| --- | --- |
| dotfiles | Little multi-agent work; route host/file handoffs to the owning UAC; sentinel-set waiting-on-Slayer state. No DAG demand. |
| spellkave | Five concurrent sentinels; PR+head ready event should arm CI watch on that head and wake only on completion; headed defaults and working environment. |
| vega-skills | No DAG demand; wants reviewed landing with real merged-head checks, push retry, bead/pane/worktree accounting and a residue view. |
| orrery | Cross-thread open asks, who owes a reply and why; attention when an idle sentinel owes an answer. Spawn-after-dependency adds little. |
| parallax | Manual brief/spawn/diff review/ff-merge/pytest/close/remove; serializes edits to shared files. Wants one landing step and reliable child-process cleanup; worktree directories sometimes remain. DAGs rarely needed. |
| rally-coach | Dependency-aware integration lane; explicit lock-file/generated-doc/BDD ownership, timely unread proposals, durable process handles and validation of final source. |

Common boundary: no automated approvals, trust answers, relayed “OK”, owner/approach
choices, unreviewed landing or deletions. A machine message never supplies Slayer
consent. Demand favors waiting, ownership and landing integration over a new DAG.
Ship the read-only waiting view first; add minimal explicit wait state next.
Keep landing in the UAC/mux/gate workflow and CI in `vega ci watch`; this plugin
links evidence and outstanding actions, rather than implementing merge/push/cleanup.
Defer the attempt ledger and replay machinery below until a concrete workflow
justifies them. No surveyed project has yet reported attempt misattribution.
Rally-coach does justify an integration-ownership slice: dependencies remain beads,
but conflicting edit ownership and validation against the integrated head need an
explicit contract. Preserve cross-repo ordering and Slayer-machine control with
their owners; never weaken a gate or infer readiness from worker completion.

## Proposed surface

`vega orchestration [--parent <bead>]`, `waiting` and `clear` are implemented.
The home view is read-only; explicit mutations require waiter/coordinator proof.
Ownership routing (.166.10) waits for the identity cutover per the UAC's
2026-10-21 direction. Attempt commands remain deferred.
Discovery: `vega skills orchestration` loads
`plugins/orchestration/SKILL.md` through `vega-orchestration --skill`. The guide
names existing post/mux/bd commands; it does not duplicate their references.

| Command | Contract |
| --- | --- |
| `vega orchestration [--parent <bead>]` | Implemented read-only view of explicit waits, observed asks, registry questions/attention, local parks and CI receipts, plus bead readiness. Shows totals, stable wait IDs and source freshness; unavailable sources and unresolved host/mode mappings stay unknown. A resolved parent scopes waits to itself and all descendants (including closed beads), omitting unassociated evidence; readiness uses bd ready descendants. An unresolved scope retains unfiltered project evidence and reports readiness unknown. No authority or scheduler. |
| `vega orchestration waiting --on <owner> --for <reason> [--bead <id>] [--thread <id>] [--coordinator <pane>@<host>]` | Set this caller's explicit wait; a stable coordinator-host-qualified ID is returned. Default coordinator is the recorded local parent, otherwise the caller; an explicit remote coordinator must prove its waiter lineage. Owner is pane@host, Slayer, or ci:<existing-watch-target>@<head>. No impersonation, new watcher or automatic permission grant. |
| `vega orchestration clear <wait-id> --evidence <text>` | Waiter or its coordinator records resolution idempotently. A reply is evidence to inspect, not automatic authorization to act. |
| `vega orchestration dispatch <bead> --to <pane>@<host> --brief-file <path> --request <id>` | Opt an existing idle sentinel into one supervised attempt; verify UAC/parent ownership, current session and bead readiness, persist reservation, then send its brief via post. Return attempt ID and delivery receipt. Start new terminals with existing mux spawn first. |
| `vega orchestration show <attempt> [--full]` | Owner, worker identity, request, delivery/evidence, outcome, outstanding questions and disposition. `delivery_unknown` gives inspection instructions; never suggests blind resend. |
| `vega orchestration reconcile [--parent <bead>]` | Replay durable lifecycle events into the ledger, deduplicate, expose discrepancies against beads/registry. No spawn, close, retry or task closure. |
| `vega orchestration settle <attempt> --disposition reuse\|retain\|release --evidence-file <path>` | Coordinator acknowledges reviewing the report and records next owner/cleanup intent. Reuse requires a new dispatch; release records intent, then existing mux/worktrunk commands do cleanup. Record verified cleanup on repeating settle with evidence; failed cleanup remains outstanding. |

Add optional `--attempt <id>` to **post** send/ask/done, and attempt addressing to
post send, rather than parallel messaging verbs. Dispatch prints exact worker
question/done commands. Typed done includes the explicit outcome and is immutable
per attempt: identical repeats succeed as no-ops; conflicting outcomes fail.
Legacy commands keep their current behavior; untagged reports cannot settle an
opted-in attempt. An ask timeout leaves a question pending; reply threads correlate
it, without requiring blocking shell calls. Defer a question-resume command until
the survey demonstrates need.

Separate survey-driven slices compose this plugin rather than growing its verbs:

- Proposed `vega orchestration owner <path> [--host <host>]` reads explicit
  project-relative prefix rules from `.vega/ownership.toml` and the project UAC
  registry. It returns the declared edit/integration owner and post routing command;
  no match is unknown, conflicting equal-prefix rules fail, longest prefix wins.
  Owners explicitly declare shared lock files, generated docs and BDD. This is
  routing evidence, not a file lock or authority to edit another owner's work.
- `vega uac land <bead> --branch <branch> [--head <sha>]` lives in UAC;
  see the [approved landing contract](#uac-landing-contract-vega-lhc16611) below. Bind
  reviewed diff, gate and project validation to the actual integrated head; expose
  phases and real exit statuses, resume push without repeating a successful merge,
  and list residue after failure. Preserve configured merge strategy (parallax
  uses ff, vega-skills requests no-ff), decisions, and deletion/cleanup approval.
  Fix mux child-process/worktree residue handling in its owning tools, not by
  silently adding destructive cleanup to orchestration. This design authorizes
  neither automatic landing nor a new cross-repo scheduler.

## UAC landing contract (vega-lhc.166.11)

Status: approved by UAC, post 5783 (2026-10-21), with the amendments below. `land` is an explicit UAC
operation, never a sentinel completion hook. It never deploys, grants approvals,
skips gates, resets a checkout, drops a stash, or deletes a branch.

### Inputs and authority

`vega uac land <bead> --branch <branch> [--head <sha>] [--target <branch>]
[--keep]` starts a landing. Freeze the branch tip (or verify the supplied head
matches it). `vega uac land <bead> --resume` continues its existing receipt;
`--status` only inspects it. Resume cannot replace inputs. Unknown, duplicate,
missing and incompatible flags fail before effects; `--help` always works.
One repository OS lock excludes concurrent landings; never steal it.

Run from the registered UAC's target checkout, including a linked target checkout
in a bare layout. Resolve Git's
common directory; reject sentinel worktrees, another UAC, dirty index/worktree
(including untracked files), active merge/rebase, unresolved blocking dependencies
or missing bead. `--target` overrides configured target for integration branches.
Review evidence comes from gate coverage computed for exactly the frozen source
head and from the completed sentinel's done report or nonempty bead notes; no
hand-written evidence JSON. The UAC invocation attests acceptance of that report.
Missing gate configuration is explicitly reported as not configured; configured
gates must pass and are never inferred from report text. Cleanup defaults on for
this UAC's own completed local sentinel identified by spawn/parent records and
worktree identity; `--keep` retains it. Unknown or remote ownership is
retained with residue, never inferred from lane labels alone. Cleanup authorizes
only this pane and a clean merged worktree, never branch deletion/hook approval.

Stable project policy lives in committed `.vega/land.toml`, read from the target
commit before merging (incoming changes cannot relax their own policy). Required
fields: `strategy = "no-ff" | "ff-only"`, `check = ["program", "arg", ...]`.
Optional: `install = ["program", "arg", ...]`, `remote = "origin"`,
`target = "main"`. Assisted-by comes from the calling harness/model (harness
only when model is unavailable), never project config. No implicit project check or install guess. Vega declares
`["nice", "mise", "run", "check"]` and `["mise", "run", "install-cli"]`; parallax can
choose ff-only and its own validation. Commands are argv arrays, run without a
shell, with stdin closed; diagnostics go to stderr and actual status is recorded.
Never invoke a deployment command. Policy is a trusted repository declaration;
land itself has no deploy phase. Store the frozen policy in the receipt so incoming policy edits cannot affect resume.

### Ordered phases

| Phase | Operation and success condition |
| --- | --- |
| Preflight | Validate authority, inputs and policy; fetch the remote, record target/base/source SHAs and branch/worktree/pane identity. Require local target equals remote target (no implicit pull/rebase). Persist intent before any mutation. |
| Merge | No-ff: `git merge --no-ff --no-edit <head> -m <subject> -m "Assisted-by: <value>"`; ff-only: `git merge --ff-only <head>`. Record resulting integrated SHA and parents. A source already integrated is an explicit no-op, not a second merge. |
| Changelog | Only a conflict consisting solely of parallel new CHANGELOG blocks may be resolved automatically: preserve target blocks byte-for-byte, place incoming blocks above them, renumber incoming blocks consecutively above the target maximum and record old/new numbers. Any other conflict stops with paths and instructions; never choose ours/theirs wholesale. The resolution commit carries Assisted-by. |
| Check | Run configured project check at the integrated SHA; success requires actual exit 0 and unchanged HEAD and clean tree afterward. Output text never substitutes for exit status. |
| Gate | Run `vega gate check`; if required segments lack coverage, run `vega gate start` and stop pending review. Resume rechecks coverage for the current integrated head. Failed, unavailable or running review never permits push. |
| Push | Push only the recorded integrated SHA to the configured target (`git push <remote> <sha>:refs/heads/<target>`). Verify with `git ls-remote` even after apparent success. Retry only recognized GitHub SSH transport timeouts, at most three attempts per invocation; inspect remote first after each failed push. Never force-push. Remote equality to the integrated SHA is required before continuing; movement or unknown remote state stops. |
| Install | After verified push, run optional project install argv on that same clean head; absent install is recorded as not configured. Failure retains pushed state and resumes installation without merging/pushing again. |
| Close bead | Build permanent close reason: changed files/approach, real check/gate results, source/integrated commit hashes, recorded changelog renumbering and follow-up IDs (explicit none allowed). Reclaim from the completed sentinel only after verifying its identity and completion, using `bd update --if-assignee <sentinel> --if-status <observed> --assignee <UAC>` without the mutually exclusive `--force`; a bead already held by the UAC is reclaimed. Other owners block. A sentinel-closed bead is accepted in preflight and on resume only when its latest closure audit actor matches that completed sentinel; preserve its close reason and append the full landing receipt to notes. Missing or unrelated closure evidence stops for inspection. A matching receipt marker in either close reason or notes makes closure idempotent. |
| Close pane | Unless --keep, only for the UAC-owned completed sentinel, revalidate saved pane/session, project, parent and completed status before `vega mux close <pane>`. Absent/reused/unknown identity is reported, never guessed or killed. Other panes in its workspace or using its worktree block cleanup; no cleanup of unrelated processes. |
| Remove worktree | Only after pane closure, verify saved worktree path belongs to saved branch, branch still equals reviewed head and is integrated, tree is clean, and path is neither main nor current checkout. Delegate non-forced removal to Worktrunk with branch retention; never approve hooks, force-remove or recursively delete a directory. Refusal is residue. |

Cleanup is optional: --keep or unproven ownership retains and lists pane/worktree. A failed
phase stops subsequent phases; unrelated cleanup is never attempted as recovery.
After validation, any changed HEAD/tree invalidates check/gate/push eligibility.
A remotely advanced target requires explicit UAC reconciliation and renewed
validation, never automatic rebase of already reviewed or integrated commits.

### Receipts, crashes and resume

Runtime state belongs under `$XDG_STATE_HOME/vega/uac/land/` (default
`~/.local/state/vega/uac/land/`), keyed by canonical Git common directory and bead;
never in committed `.vega/`. Use atomic durable replacement under the repository
lock. Keep receipts minimal: frozen inputs/policy and identities, pre-merge SHA,
integrated SHA, changelog renumbering, measured check/gate result, and install
started/done. Re-observe remote, bead and cleanup state instead of duplicating
their databases. Retain only the last failed output, not a log archive. Phase
failures report the actual exit code or signal/spawn failure.

Resume re-observes reality before trusting a receipt. A crash after a merge but
before recording success is reconciled using saved pre-merge/source SHAs and
actual parents (ff-only: exact source SHA); it never repeats a successful merge.
An in-progress merge must match saved MERGE_HEAD; preserve it and show resolution
instructions. A manually resolved merge may resume only with verified parents,
a recorded automatic renumbering (or explicit manual resolution commit) and
renewed check/gate coverage. Ambiguous changes
stop for UAC inspection. Never abort someone else's merge or adopt an unrelated
HEAD. A successful push is reconciled from remote SHA; bead closure, pane absence
and worktree absence are verified against their saved identities. Non-idempotent
install interrupted without a completion receipt is reported as uncertain and
requires UAC inspection before a retry. After inspecting effects,
`--resume --retry-install` explicitly retries, or `--resume --install-done`
attests completion (recorded as attested, not measured exit 0), only on the same
verified pushed head. Completed runs are idempotent no-ops after
identity checks; failed checks can rerun on the same head, successful phases are
not blindly replayed. Failure receipts survive failures; the next failure replaces the previous output.

An unfinished receipt blocks every new landing in that repository, even for
another bead. There is no automatic discard or abandon command. If a landing
cannot finish, the UAC inspects `vega uac land <bead> --status`, the saved receipt,
Git merge/index state, remote SHA, bead closure and all install/cleanup effects.
Resolve every uncertain effect before restarting; archival does not make an
interrupted install safe to repeat. After explicit UAC review, preserve the
receipt with a non-`.json` suffix (for example, rename `<bead>.json` to
`<bead>.json.archived`). The active-receipt scan only reads `.json` files, so this
reversible archival releases the landing lane while retaining its evidence.
A fresh landing must pass preflight again. Never use archival to bypass a gate,
abort a merge, delete residue or forget a pushed commit; unresolved cases stay
blocked and deletions remain with the Slayer.

### Output, statuses and residue

Structured stdout reports receipt, bead, reviewed/integrated heads, current phase,
phase status and real child exit status, plus a residue list and next command.
Diagnostics/progress belong on stderr. Exit 0 means all required phases completed
(including explicitly retained cleanup), or a successful status/no-op; exit 1 means
failed, waiting on review, uncertain or blocked with actionable recovery; exit 2
means invalid invocation. Child exit codes are data, not remapped to success.

Always list residue on success and failure: stash entries (never drop/pop),
unmerged local branches, retained pane/worktree, dirty or conflicted paths,
remaining worktree directories, failed cleanup and unknown inspection sources.
Each entry names identity/path, reason and owner/next action; zero is explicit.
Do not claim process cleanup from pane disappearance: mux owns termination and
child-process accounting. Investigate `plugins/mux/src/worktree.rs` and route
substantiated lifecycle defects to their owning tools with discovered-from beads.
Deletion of stash entries, branches, dirty directories and unrelated processes
stays with the Slayer.

Fixture-repository tests exercise the phase engine with fake authority, evidence,
bead-close, pane-close and worktree-removal services. They cover both merge
strategies, policy rejection, parallel changelog renumbering and unsafe conflicts,
actual check failures, head changes, SSH timeout versus other push failures,
remote verification, interrupted installs and phase failure injection. Resume
fixtures assert a single merge and no repeated completed engine effects.
Separate tests exercise Live gate inspection with a stub `bd`, target-checkout
selection (including bare layouts), and worktree parsing, identity matching,
occupancy and clean-tree helpers.

Coverage gap: tests do not exercise `Live::evidence`, registered-UAC rejection in
`Live::authority`, guarded `bd update --force` reclamation, mux pane closure or
Worktrunk removal end to end. Fake-service failure injection does not verify
those external-tool adapter paths or their real approval/ownership behavior.
Required implementation gate:
`RUST_TEST_THREADS=1 nice mise run check` exit 0, then fetch/rebase onto origin/main
and `vega gate start`; review must cover the final head.

## Stable declarations and runtime state

`.vega/` contains only stable, committed declarations. Ownership rules may map
project-relative path prefixes to owning project IDs in `.vega/ownership.toml`.
Never commit panes, hosts, timestamps, explicit waits or other churning state
there. Resolve routing against the live registry. Explicit waits live only in
local transactional state or the runtime registry, never in git.

The six-project survey above is accepted as the slice disposition: waiting view,
explicit waits and declared ownership proceed; .166.6–.166.8 remain deferred.
Landing (.166.11) is a separate sentinel's UAC implementation.

## State and failure behavior

Explicit wait records contain ID, waiter host/pane/name/session, coordinator
host/pane/name, awaited owner, reason, optional bead/thread/CI-head reference,
created/resolved times and evidence. Until the identity cutover, they are keyed
only by pane@host, bead and thread; do not add a parallel project-ID publication
protocol. Project display association uses existing pane metadata or a known bead.
Use local transactional state on the coordinator host and post for remote updates;
resolve project ownership through the UAC registry, never a guessed file owner.
If no ownership declaration exists, show unknown and ask the UAC. Join existing
post ask threads, UAC park state and CI receipts without duplicating them; surfaced
source and freshness distinguish declared waits from inferred attention. Do not
infer answered from a thread ack or remove a wait because a pane is idle. Reuse
vegad attention for one wake per newly actionable idle/unanswered state, with no
auto-ping loop. Preserve headed-pane routing and Slayer-owned decisions. The MVP
is useful without the attempt states below; missing sources are explicitly unknown.

The implemented ledger is `$XDG_STATE_HOME/vega/orchestration/waits.sqlite3`,
with atomic SQLite transactions and no git state. Identical waiting retries return
the same ID; changing an active wait in the same waiter-session/bead/thread scope
requires explicit clear evidence. Duplicate clears retain the first resolution.
The registry projects active records under their coordinator host; resolved local
records fence a lagging projection. Stale projections remain marked unknown.
Registry wait and ask readers validate the whole corresponding source: one malformed
or publisher-mismatched entry makes that source unknown, including otherwise valid
entries from other hosts. The view does not claim that the source is empty.

Remote updates use `vega.cmd.<coordinator-host>.orchestration.<sender-host>`.
The existing post command handler passes the broker-authenticated sender host;
the orchestration handler verifies it owns the asserted waiter, checks fresh
agent/name evidence, and verifies lineage from the coordinator's local records
and saved SSH-target mapping. Hosts assert their own harness sessions inside the
P7 machine boundary; this is not a process capability. Missing session identity,
unknown lineage or stale identity refuses mutation. No remote path is opened,
registry grant broadened, or UAC identity guessed. Limited hosts need the existing
explicit command/read grants; failure stays unknown rather than bypassing them.

Post gather deadlines release held mail while retaining unanswered thread metadata
under local `asks-open` state and the publisher's runtime registry keys. Matching
thread replies update it; reads/acks do not. The existing supervisor raises one
`unanswered` attention when a known idle child owes a reply, with positive mode
evidence and headed routing preserved. Unchanged idle/unanswered state does not
repeat a wake; working/answered transitions rearm it. Missing/corrupt source files
are unknown, while an existing readable empty source is empty. All target replies
remove the question metadata immediately. Otherwise its retention ends seven days
after the gather deadline: reads stop projecting it, and the asker's daemon removes
the local archived metadata on its next gather sweep, after releasing any held
mail. This is retention expiry, never evidence of an answer; stored post messages
remain available. Late replies after retention deliver as normal mail without
reviving the expired question. No operator cleanup is required for abandoned asks.

The following ledger design applies only to the deferred supervised-attempt slice.

Beads own descriptions, acceptance, deps, decisions and closure. Post owns message
bodies and archived lifecycle events. Mux/registry own panes, parenthood and session
identity. The plugin owns only attempt IDs, bead/project ID, coordinator identity,
worker host/pane/session, request ID, delivery IDs, outcome, evidence pointers,
processed-event IDs and disposition. Store these in a local transactional SQLite
ledger under `$XDG_STATE_HOME/vega/orchestration/<project-key>/`; no git files or
second Dolt task database. The coordinator host is the single writer. Remote
workers publish tagged events through post to that host.

One active attempt per bead and per worker session is enforced transactionally.
Record the request before sending; repeated request IDs return the original
receipt. A crash between publish and receipt is `delivery_unknown`: inspect the
archived request ID and worker before completing the reservation. Never infer
that a lost response means nothing started. Validate sender host/pane/session,
attempt and owner before applying events; fence older attempts even if their
reports arrive later. If session identity is unavailable, refuse opt-in dispatch;
do not claim pane identity provides a process capability or security sandbox.

Use a dedicated replayable post lifecycle consumer/outbox, independent of the
agent inbox's read acknowledgements. Commit ledger changes and processed event IDs
atomically before acking that consumer. Reconcile on restart and explicit command;
vegad invokes the same reducer on new events and sends existing attention pointers.
A crash after commit replays harmlessly. Unknown headers/capabilities on older
hosts fail opt-in explicitly; ordinary post remains usable. If ledger or transport
is unavailable, show unknown and preserve the reservation.

Report states: `reserved -> dispatched|delivery_unknown -> reported -> settled`;
failed is an explicit report outcome, not inferred from silence. Connectivity loss,
quota, idle panes and wait deadlines never authorize duplicate editors or cleanup.
Retry requires positive exit/stop evidence and an explicit coordinator action that
supersedes the old attempt; the next dispatch uses a fresh request ID. Settling a
report does not close the bead or land changes: the UAC checks acceptance, review
and cleanup evidence, then closes with the normal permanent handoff record.
Coordinator takeover is deliberately not automatic: reconstruct from events and
explicitly transfer ownership with old-writer fencing before allowing mutations.
The first implementation may refuse takeover and remote dispatch until it can
prove those contracts.

## Fit and deliberate exclusions

Follow [agent-tooling-principles.md](agent-tooling-principles.md) P1/P5: one plugin,
runtime guide with `vega_plugin::assert_guide`, long help in embedded Markdown;
no SessionStart discovery hook. P2/P6: keep `bd` native and compose existing tools.
AXI: minimal TOON rows, totals and explicit empty states; preview large text with
`--full`, actionable next commands, reject unknown flags before effects, structured
stdout errors, diagnostics on stderr, exits 0/1/2, idempotent mutations, concise
per-command help. Use vega's version stamping (P1), not AXI's conflicting bare
version rule. No interactive prompts or periodic agent-side polling.

Do not copy Orca's Run/Task database, terminal/browser/worktree stack, SSH/WSL
placement, group vocabulary, heartbeat protocol, automatic scheduler, provider
launch policy or compatibility labels. Do not add decision-gate verbs or reinterpret
`vega gate`. Do not auto-close beads on done or auto-release panes. Keep useful
Orca invariants: attempts outrank pane titles, enqueue is not acceptance, missing
observations are unknown, and every completion needs evidence and a next owner.

## Build slices

Child beads below carry implementation and dependency order. The read-only home
view, runtime guide (.166.5) and explicit waits (.166.9) are built; the remaining slices retain the survey
and approval boundaries above.

1. **vega-lhc.166.5**: guide and waiting/ownership home view; incorporate survey.
2. **vega-lhc.166.9** (after .5): explicit wait lifecycle and existing attention integration.
3. **vega-lhc.166.10** (after .5): declared edit/integration ownership and routing.
4. **vega-lhc.166.11**: UAC landing contract and resumable phases; review contract before implementation.
5. **vega-lhc.166.6** (after .5; deferred pending evidence): transactional attempts, reservations and dispatch.
6. **vega-lhc.166.7** (after .6; deferred): optional post tags, lifecycle replay and fencing.
7. **vega-lhc.166.8** (after .7; deferred): disposition, decision recipe and restart scenario.

Each slice requires `mise run check` and a gate pass when implemented. This design
runs no build. The parent-child links record provenance; bd cannot also store a
`discovered-from` edge between the same two issues.
