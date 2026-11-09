# vega ci

Waits for CI on GitHub or Forgejo/Gitea without polling. All agents share one forge API budget, and a single
`gh run watch` loop burns a large part of it. Instead, vegad runs one poller per host and mails the verdict (green,
or red with the failed checks) to the agent that asked.

GitHub heads stay pending while any workflow run is queued or running, even before job checks exist.
External passing checks alone do not make a head green.

Commands: `vega ci watch`, `vega ci`, `vega ci show`, `vega ci unwatch`.

Design: [ci-watch.md](../../docs/design/ci-watch.md).

Agent guide: `vega skills ci`
## Overview JSON (schema 1)

`vega ci --json [--project <project-id>]` reads cached watches and emits
`{schema: 1, project: string|null, watches: [...]}`. JSON defaults to the cwd
project; outside a project it lists all host watches. Explicit IDs match
the target's `host/owner/repo`. Rows group watchers by target and contain
`project`, `target` (label), `watchers` (pane IDs), `state` (`unpolled`,
`active`, `error`, `ended`), `head` (string|null), `polled` (Unix seconds|null),
`error` (string|null), `verdict` (`unpolled`, `none`, `pending`, `green`, `red`),
and `failed_checks` (full names). No forge API call is made.

JSON applies to the home view. Errors use `{error: {code, message, hint}}`
(exit 2 usage, 1 runtime). Empty lists are arrays; TOON remains the default.
Consumers should ignore future additive fields.
