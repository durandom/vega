TARGET: 42, #42, a PR URL, a run URL, owner/repo#42, owner/repo@branch, owner/repo/run/<id>,
or a branch name. Without TARGET: the current branch (its upstream name) of this checkout.
The watch mails this pane: the one whose process tree holds the caller (HERDR_PANE_ID is a hint);
outside a pane it fails with not_in_herdr. --for <pane> mails that pane instead (any pane on this host): for yourself, pass your own pane, not the one you report to.
One mail per new verdict of the head (green, or red with the failed checks), one when the
PR is merged or closed or the branch deleted. The watch also ends when the pane closes.
On GitHub, queued, running and approval-waiting workflows for the head keep it pending,
even when external checks pass and no Actions job checks exist yet.
GitHub also mails once when the PR conflicts with its base or is behind a base that requires
up-to-date branches (rebase), and once per zombie run (queued or in progress, all jobs done for
15 min) with the force-cancel command; vega never cancels it.

A workflow_dispatch run (GitHub Actions) has no PR and its own sha: watch the run.
--run <id> (gh workflow run prints the run URL), or --workflow <file> [--ref <branch|sha>]:
vegad takes the newest dispatched run of that workflow from 10 min before the watch on.
Its verdict is the last mail; a cancelled run mails its conclusion.

Examples:
  vega ci watch
  vega ci watch 42
  vega ci watch https://github.com/o/r/pull/42
  vega ci watch --run 38721967731
  vega ci watch --workflow deploy.yml --ref main
  vega ci watch git.example.org/o/r#42   # Forgejo/Gitea host (needs tea login)
  vega ci watch 42 --for w3V:p2     # for another pane on this host
