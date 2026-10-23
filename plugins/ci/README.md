# vega ci

Waits for CI on GitHub or Forgejo/Gitea without polling. All agents share one forge API budget, and a single
`gh run watch` loop burns a large part of it. Instead, vegad runs one poller per host and mails the verdict (green,
or red with the failed checks) to the agent that asked.

GitHub heads stay pending while any workflow run is queued or running, even before job checks exist.
External passing checks alone do not make a head green.

Commands: `vega ci watch`, `vega ci`, `vega ci show`, `vega ci unwatch`.

Design: [ci-watch.md](../../docs/design/ci-watch.md).

Agent guide: `vega skills ci`
