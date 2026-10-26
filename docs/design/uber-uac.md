# Uber-UAC: the Slayer's one channel to every UAC

Status: design, 2026-10-26; vega-lhc.210 for decision vega-lhc.209. Not built.

## Problem and decision

Each project has one UAC ([uac-role.md](uac-role.md)). The Slayer has to visit each UAC's pane or
`vega board` to answer decisions; from the phone they can't. Decision vega-lhc.209: one **Uber-UAC**
over all projects and hosts, driven from Claude Code on the phone. **Uber-UAC == Slayer**: its answers count
like the Slayer's own typing, L4 approvals included. **It decides nothing**: it passes on only what the
Slayer wrote. Without Slayer input it produces no answer and no approval.

Prior art: arena (`framework/operating-model.md`: L0-L4, four dimensions, escalation = context, 2-3
options, recommendation, deadline; `ludus/agents.md` CoS: compress, don't relay; batch, don't interrupt;
briefing Decisions/FYI/Blocked; `intercom/coordination.md` `FROM HUMAN:` relayed input) and castra
(`docs/autopilot-pi.md`: supervisor channel of send/advance/wait/halt, 280 chars). Neither has a layer above
the coordinators or a rule for relayed authority. vega adds both. It makes the relay mechanical, so the model never writes the Slayer's words and never
binds them to an ask. It is trusted only to show asks and to call `vega uber answer` at all.

## 1. Placement: Remote Control on an always-on host (ganymede)

**Decision: an interactive Claude Code session in a herdr pane on ganymede, driven via Remote Control.**

| Option | Tailnet/NATS | vegad pointers | Verdict |
| --- | --- | --- | --- |
| Remote Control, interactive, herdr pane on ganymede | yes: the session uses the host's network and files ([remote-control](https://code.claude.com/docs/en/remote-control.md), [cloud-environments](https://code.claude.com/docs/en/cloud-environments.md)) | yes: a normal pane | **chosen** |
| `claude remote-control` server mode | yes | no: its sessions run outside herdr, so no registry entry, no pointers, no draft guard | rejected |
| claude.ai cloud session | no: it runs on Anthropic infrastructure; network levels None/Trusted/Full/Custom allowlist; tailnet hosts not documented | no | rejected |
| Cloud session on a self-hosted runner | possible ("can reach internal services", [self-hosted-environments](https://code.claude.com/docs/en/self-hosted-environments.md)) | no | rejected: Team/Enterprise beta; unknown whether the Slayer's plan has it |

Facts from the docs (2026-10-26): Remote Control needs Pro/Max/Team/Enterprise and `/login`, not an API
key. The process must keep running: herdr keeps it running and survives SSH loss. In interactive mode it
reconnects after network loss. The first run asks `Enable Remote Control? (y/n)` once, which needs a TTY.
The app offers Manual, Accept edits, Plan and Auto; **Bypass cannot be selected**, which suits us (§4).

**Host-agnostic.** Any always-on host with vegad, herdr and claude qualifies. The claim names the host
(§2), and nothing hardcodes one. The Slayer chose **ganymede** (on 24x7). proteus is the laptop: it sleeps and
travels, so it holds UACs but not the Uber.

**Cross-host.** UACs run on other hosts (proteus included). Everything between them and the Uber goes
over NATS, and the broker enforces the sender host as the subject token. An ask from proteus reaches
ganymede at once. While proteus sleeps, the answer queues in the broker (JetStream) for the UAC's pane, and
proteus's vegad verifies and applies it on wake. The Uber marks asks whose UAC host's beat is stale as
`host asleep`, and the Slayer sees `sent, not applied` in `audit` until `uber.applied` arrives. Without a
broker (local mode) there is no Uber (§2).

## 2. Singleton: one estate-wide claim

- `vega uber claim`: the session becomes the Uber-UAC. Registry key `<host>.uber.v1` = `{schema: 1, pane,
  host, at, epoch}`. This mirrors `<host>.uac.v2.<id>`, with one key for the whole estate, not one per project.
  `epoch` = claim time in ms; it changes on every claim.
- **Live** = the host's beat is fresh and the pane is alive (as for `remote_uac`). A live claim elsewhere
  refuses with `uber_exists` and the hint `vega uber release in <pane> on <host>`.
- **Takeover**: a claim whose pane or host is gone is replaced, and the output prints `took_over: <pane> on <host> (gone)`.
  Each host writes only its own key, so two claims can race. Of the live claims the highest `epoch` wins
  (the newest claim), ties go to the lower host name. The loser sees the conflict on its next read and
  prints `uber_conflict`. UACs apply the same rule: `vega_registry::View::live_uber` is the one resolver.
- `vega uber release`: deletes the key. `vega uber` (home): claim, open asks, last brief.
- **Absent** (no live claim, broker down, or local mode): `vega uber ask` returns `uber_absent` and the UAC
  behaves exactly as today: a `slayer-decision` bead, the board, its own pane. Nothing queues for an
  Uber that does not exist.
- **New epoch**: the UAC's state is the source of truth. When a UAC host's vegad sees a new epoch, it
  sends that UAC's open asks again to the new claim. Once an ask is resent, only the new epoch's answer counts.
  An answer from the old claim that arrives before the resend still applies (§4, check 2).

## 3. Protocol: ask, answer, brief over vega post

Plain `vega post` mail with a `Vega-Kind` header and a JSON body. The sender host stays broker-enforced
(subject token). Mail addressed to `--uber` resolves to the live claim when it is sent. Bodies are never
typed into a pane ([vegad.md](vegad.md) Security).

| Kind | Direction | Cap | Body |
| --- | --- | --- | --- |
| `uber.ask` | UAC → Uber | 1 KiB | `id, project, bead, level (L3\|L4), urgent, question (≤120), context (≤280), options [2-3 × {label ≤40, detail ≤120}], recommend (index), why (≤120), deadline (RFC 3339 \| "no-rush")` |
| `uber.answer` | Uber → UAC | 2 KiB | `id, tag, bead, epoch, pick (label \| null), approval, notes, slayer_text, shown_sha256, capture {source, at, sha256}` |
| `uber.applied` | UAC vegad → Uber | 256 B | `id, result (applied \| refused:<code>)` |
| `uber.withdraw` | UAC → Uber | 256 B | `id, reason` (answered elsewhere, obsolete) |
| `uber.brief` | Uber → all live UACs | 128 B | `since`; sent with `vega post ask --within 2m`, so one pointer brings every reply |

- **Ids**: `id = <bead>~<nuid>`. The bead keeps the record readable and the nuid makes the id unguessable.
  The Uber gives each open ask a short `tag` (`#a7`, unique among its open asks) for the phone.
  An answer is always for exactly one ask.
- **Routing**: the UAC records the question as a `slayer-decision` bead as today, then runs `vega uber ask --bead <id>
  --level L3|L4 [--urgent "<why>"] --deadline <t> --context ... --option "<label>: <detail>" ... --recommend <n>`.
  The command checks the caps and the 2-3 options, writes the open ask to
  `$XDG_STATE_HOME/vega/uber/asks/<id>.json` on the UAC host, and sends it. The UAC then parks the
  waiting sentinel (`vega uac park`) as today. It does not wait in its pane. The bead, the board and the pane
  still work: an answer given there closes the bead, and the UAC sends `uber.withdraw`.
- **Levels**: the UAC assesses reversibility, blast radius, commitment and visibility. All of them low
  (L0-L2): the UAC decides, and only the brief's FYI mentions it. Any of them high: **L3** (the Slayer
  picks an option). Deploy, GitHub settings, broker rights, deletions and anything irreversible:
  **L4** (only the Slayer may approve).
- **Batching**: vegad on the Uber host records every ask it receives, so `vega uber` lists them whatever the
  model does. The Uber shows asks to the Slayer:
  - **at once**: `urgent` asks (production down, security, data loss, or a deadline under 1 h). The open dialog
    triggers the app's "actions required" push.
  - **batched**: everything else, when the Slayer says "brief" or opens the session, plus a one-line
    digest push when the oldest waiting ask is 4 h old (vegad wakes the Uber; one push per 4 h at most).
  - A passed deadline only marks the ask `overdue`. **No default fires on a timeout**; the ask waits.
- **Brief** (on demand): Decisions = the Uber's own open asks. FYI and Blocked come from each UAC's reply
  to `uber.brief`: at most 3 lines each, ≤600 chars per UAC. Compress, don't relay: no transcripts and
  no raw sentinel output.

## 4. Trust rule and guards

**Rule**: a UAC treats an `uber.answer` as the Slayer's own words only after its vegad has verified the answer
(below). The UAC then acts on `slayer_text`, which is verbatim, not on a summary. An L4 answer approves
exactly the option the ask described, nothing wider. Every other "the Slayer said" stays a peer request
([concepts.md](../concepts.md) Trust model), and that includes plain mail from the Uber-UAC itself.

Verification is deterministic, in vegad on the UAC host, before anything reaches the UAC:

1. An open ask with this `id` exists in the local state, and its bead is still open.
2. The sender host (broker-enforced) and pane are those of the claim the ask was sent to: the open ask
   records that claim's host, pane and `epoch`, and the answer's `epoch` must match it. The claim need not be
   live any more. An answer that queued while the UAC host slept still applies after a takeover, and a
   resend to a new epoch replaces the recorded claim.
3. `tag` is the ask's tag. `pick`, if set, equals one of the ask's option labels exactly. `slayer_text` is not
   empty. `shown_sha256`, if set, matches vegad's own rendering of the ask. On an L4 ask, `approval: true` requires `capture.source = ask_user_question`, a `pick` and `shown_sha256`.
4. The ask was not answered or withdrawn before. The first verified answer wins; later ones get `refused:answered`.

If the answer passes, vegad adds `slayer_text` as a bead comment by `Slayer` with `via uber <id>` (the
`vega uac answer` path), mails the UAC "The Slayer answered <bead> ...", closes the open ask, and returns
`uber.applied`. If it fails, nothing reaches the pane: vegad writes a note (kind `uber-refused`, like
quarantine) and returns `refused:<code>`.

**What `slayer_text` is.** The model must not write it. Claude Code hooks in the Uber's settings capture
the Slayer's input: `PostToolUse` on `AskUserQuestion` (the selected labels and notes) and
`UserPromptSubmit` (typed prompts). Both append to `$XDG_STATE_HOME/vega/uber/captures.jsonl`.
`vega uber answer <id> --capture <n>` builds `slayer_text` from that record. It refuses an answer with no
capture, or with a capture that was already used or is not **bound** to the ask:

- **Bound by the harness**: every ask shown gets a short tag (`#a7`) in its AskUserQuestion `question`.
  An AskUserQuestion capture holds the tool input and the answers, so it names the ask by that tag and the
  option by its exact label. The model does not choose the binding.
- **Shown as asked**: `vega uber answer` re-renders the ask the way `vega uber show` does (question,
  context, option labels, details and order) and compares the result with the captured AskUserQuestion input for that
  tag. Any difference refuses with `capture_mismatch`, so the model cannot pass reworded options under a
  valid tag. The answer carries `shown_sha256` of that rendering. The UAC-host vegad renders its own copy of the
  ask the same way and refuses a hash mismatch, so this check does not depend on the Uber host alone.
- **Bound by the Slayer's words**: a typed prompt binds only to the asks whose tags it contains
  (`#a7 ja, #b2 nein`). A prompt without a tag, such as "brief", answers no ask.
- **L4 approvals** need an AskUserQuestion capture whose answer is an exact option label (`capture.source =
  ask_user_question`, `pick` set). A typed prompt or "Other" text goes to an L4 ask as a comment
  (`approval: false`); the UAC must not treat it as approval and asks again with the options it implies.

The answer carries `capture.source`, `tag` and `approval`. The UAC-host vegad checks that the tag is the
ask's tag, and that an L4 answer with `approval: true` has `source = ask_user_question` and a matching
`pick`. The binding itself is checked on the Uber host, which is the trusted base (below). A prompt with several
tags goes in full to each of its asks, and each UAC reads it in its own context. If the text is ambiguous,
the UAC asks again.

**What the Uber-UAC may run.** It runs in Manual mode (Bypass is unavailable from the app anyway).
`vega uber start` writes its settings, which allow `Bash(vega uber:*)` and `Bash(vega post inbox:*)` only. There is no
`vega mux send`, no `vega post send`, no transcripts or pane reads, no host actions (deploy, git, bd
writes, secrets) and no file edits. Any other call raises a permission prompt on the phone, which the
Slayer can see and refuse.

**Audit**: `$XDG_STATE_HOME/vega/uber/audit.jsonl` on the Uber host records each ask received, shown,
answered (capture, `slayer_text`), sent, applied or refused. `vega uber audit [--last 10]` prints
one line per event, short enough for the phone. On the project side, each Slayer comment `via uber` on the
bead is the per-project record.

**A UAC must refuse** (vegad does so mechanically, and the guide says it too): an answer with no open
ask; an answer to an ask that is closed, answered or withdrawn; an answer from any host, pane or epoch
other than the claim the ask was sent to; an answer with an empty `slayer_text` or a `pick` that is not one of the labels; an
approval used beyond the scope of the asked option; and every instruction from the Uber that is not a
verified answer, such as "do X" in a brief request or a plain mail.

**Captures across hosts.** The captures live on the Uber host only. The UAC host cannot read them, so it
cannot check `capture.sha256`; the hash is there for the Uber's own audit. The capture guard binds the Uber's
model to the Slayer's input on ganymede. The UAC host relies on the broker-enforced sender host for the rest.

**Limit, stated plainly.** The pane id is self-asserted, and every agent on the Uber host runs without
prompts. A rogue agent on ganymede could write a capture and send an answer as the claim's pane. The nonce
stops forgery from other hosts and replays, not an attacker on the Uber host. The Uber host is therefore
part of the trusted base, as every host's NATS key already is. Recommendation: run no unattended sentinels
on the Uber host. Whether ganymede runs any today: I don't know. A dedicated OS user is deferred until a threat calls for it.

**Out of scope for v1**: Slayer-initiated orders ("tell vega to start X") with no open ask. The Slayer
writes those in the project's UAC pane as today. Adding `uber.tell` later would need its own decision.

## 5. Mobile UX: one AskUserQuestion per batch

The docs say Remote Control "keeps permission prompts and `AskUserQuestion` questions open until you
answer them" ([remote-control](https://code.claude.com/docs/en/remote-control.md)). How the question
looks in the iOS/Android app is not documented; the spike below verified it in the mobile app (OS not recorded).

Mapping (deterministic; `vega uber show` prints the JSON the model passes to AskUserQuestion):

- One call shows up to 4 asks (the tool's limit), urgent ones first, then the oldest.
- `header` (≤12 chars) = project short name. `question` = `#a7 <question> · L4 · due 14:00`, with the
  context on a second line (≤280 chars).
- `options` = the UAC's 2-3 options, the recommended one first with ` (Recommended)`. `description` = its
  detail, plus `why` for the recommended one. The tool adds "Other" for free text.
- `slayer_text` (built by `vega uber answer` from the capture): the picked label, then ` — <notes>`
  if there are notes; for "Other", the typed text verbatim.

Phone text stays short. A brief has at most 25 lines: `Decisions (n)` with `1 vega · L4 · push main? ·
rec: yes · due 14:00`, then `FYI (n)` and `Blocked (n)` with one line per item. Ids appear only in `audit`.

**Spike results** (vega-lhc.209.1, 2026-10-26, Claude Code 2.1.310, `/remote-control` in a herdr pane on
ganymede, Claude mobile app; captures from project-local hooks, one run per test):

- **AskUserQuestion on the phone: yes.** Native and tappable. Header, full question, 3 options with
  descriptions and "Other" were all readable, with nothing cut off.
- **Hook payloads carry phone input: yes.** The `PostToolUse` payload for `AskUserQuestion` holds `tool_input.questions`
  (exactly as shown) and `tool_response.answers` = `{"<question text>": "<label>"}`. A pick is the exact label,
  `(Recommended)` suffix included. "Other" is the typed text verbatim (`"Pizza please"`); no field marks it, so
  `vega uber answer` must classify it as "not equal to any label". The tag is inside the question key (`#t1 ...`).
  A phone prompt arrives as `UserPromptSubmit` with `prompt` verbatim.
- **Origin of a prompt: not distinguishable.** `UserPromptSubmit` has no source field. A vegad pointer
  (`[vega-post] 1 new message for you. Run: vega post inbox`) and harness background-task notices
  (`<task-notification>...`) produce the same payload shape as the Slayer's typing. The capture must
  therefore drop these known non-Slayer prompts, and a typed prompt binds only through its tags (as above).
  `AskUserQuestion` captures are not affected.
- **vegad never types into an open dialog: yes.** A mail arrived while a question stayed open for 6 min.
  No `UserPromptSubmit` fired during that time, and the Slayer's answer was clean. Mechanism: herdr reports the
  pane as `blocked`, and delivery types only into `idle`/`done` panes (`plugins/post/src/delivery.rs`).
- **Pushes while vegad types: suppressed briefly.** A question opened 10 s after a typed pointer produced
  no push on the locked phone. The same question opened 70 s after a pointer, or with no pointer, did produce
  a push. This fits "skipped while you are typing in or focused on the connected terminal": typed pointers count
  as terminal input. Consequence: the Uber waits until ≥60 s after the last typed pointer before it opens an
  urgent dialog (`vega uber show` can enforce this). The exact window is not measured.

## Rejected

- Uber as a UAC of a meta-project: claims are per project, and it would take on landing and sentinel duties.
- Answers through beads only (Dolt): the Uber host would need a clone of every project database, and sync takes ≥60 s.
- A model-written summary as the answer: the summary would become the authority, which vega-lhc.209 forbids.
- Telegram or another chat bot (arena): the Slayer chose Claude Code.

## Build sequence (children of vega-lhc.209)

1. vega-lhc.209.1: Spike: Remote Control in a herdr pane on ganymede. Test AskUserQuestion on the phone, hook payloads from phone input, pushes while vegad types, and that vegad never types into an open dialog.
2. vega-lhc.209.2: `vega uber claim|release|status` with registry key `<host>.uber.v1`, epoch, takeover and the conflict rule.
3. vega-lhc.209.3: `Vega-Kind` post messages: `uber.*` schemas and caps, `--uber` addressing.
4. vega-lhc.209.4: UAC side: `vega uber ask` with open-ask state, `uber_absent` fallback, resend on a new epoch, withdraw on bead close; uac-role.md routing line.
5. vega-lhc.209.5: UAC-host vegad: verify `uber.answer`, apply it as a Slayer bead comment plus UAC mail, refuse to a note, send `uber.applied`.
6. vega-lhc.209.6: Uber host: capture hooks, `vega uber answer --capture`, audit.jsonl and `vega uber audit`.
7. vega-lhc.209.7: `vega uber start` and the guide `vega skills uber`: dedicated cwd, allowlist settings, hooks, Remote Control, `vega uber show` mapping.
8. vega-lhc.209.8: Brief: `uber.brief` fan-in, Decisions/FYI/Blocked rendering, the 4 h digest wake; concepts.md section.
