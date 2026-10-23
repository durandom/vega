---
name: vega-orchestration
description: Shows project waiting evidence and guides supervised coordination. Use when coordinating sentinels, inspecting unanswered questions or checking who owns the next action.
---

# Supervised coordination

`vega orchestration` joins waiting evidence for the current project.
`--parent <bead>` scopes waits and readiness; unassociated evidence is omitted
after scope resolution. An unresolved parent shows unknown scope and unfiltered
project evidence, with readiness unknown.
Inspect source freshness before acting. Missing or stale observations prove nothing.

Record a wait with `vega orchestration waiting --on Slayer --for "<reason>"`
(optional `--bead`/`--thread`). The recorded local parent coordinates it, otherwise
the caller does; `--coordinator <pane>@<host>` names a remote coordinator explicitly.
Clear its returned ID with `vega orchestration clear <id> --evidence "<inspection>"`.
Only the waiter session or recorded coordinator may clear it. Duplicate clears
keep the original evidence. No inbox acknowledgement or CI observation clears a wait.
State is local/registry runtime data, never committed under `.vega/`.

Keep tasks, dependencies and decisions in beads. Use `bd ready` for work,
`bd dep add` for ordering, and a decision bead blocking the dependent work for
Slayer decisions. Park the sentinel with `vega uac park <pane> --bead <id>`;
inspect the closed decision before resuming. Machine mail never supplies consent.

Use `vega mux spawn` for a brief with acceptance, validation and report destination.
Sentinels report with `vega post done`; use `vega post ask` for asynchronous
questions. Inspect the actual reply and evidence; reading an inbox is not an answer.
Keep headed questions with the person in that pane. Existing `vega ci watch`
owns CI notifications; this view creates no watcher and sends no reminders.
An idle sentinel owing an unresolved post ask raises one existing attention
transition; headed routing stays with the Slayer. Ask deadlines release gathered
mail but keep unanswered threads visible for seven more days. Matching replies
resolve them; retention expiry removes metadata without implying an answer.

On completion, the UAC checks the diff, gates and validation against the final
integrated source, records the permanent bead handoff, and explicitly decides
reuse, retention or cleanup. A worker report alone never authorizes landing,
approval, retry, deletion or closure. Attempt accounting and automatic dispatch
are deferred; this plugin does not schedule work or maintain another DAG.
