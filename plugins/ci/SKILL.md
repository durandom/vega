---
name: vega-ci
description: Watches CI of a PR or branch on GitHub or Forgejo/Gitea through one shared poller per host (vegad) and delivers green or red, with the failed checks, as mail. Use when waiting for CI or PR checks, instead of gh run watch, gh pr checks --watch or polling gh or tea in a loop.
---

# vega ci: wait for CI without polling

Agents share one forge API budget. Never poll gh or tea in a loop; vegad has one poller per host,
which matters especially for Forgejo/Gitea because it has no conditional requests.

```
vega ci watch                # the current branch of this checkout
vega ci watch 42             # PR #42 of this checkout's repo; ends at merge or close
vega ci watch o/r#42         # any repo; also a PR URL or -R owner/repo
```

It returns at once; end your turn or work on. vegad mails each new head verdict (`green` or `red`
with failed checks) and PR merge or close. The pointer arrives through `vega post inbox`.
GitHub waits for the head's workflow runs too: queued or approval-waiting runs block green even
when external checks pass. `vega ci show` includes those workflows.
GitHub also mails a needed rebase: rebase before you push again.
A zombie run (open, all jobs done) mails its force-cancel command.
A run you dispatched (`gh workflow run`, GitHub Actions) has no PR, and a branch watch mails
once per head, so it can miss the run. Watch the run itself; its verdict is the last mail:

```
vega ci watch --run 38721967731                # the id ends the run URL gh workflow run prints
vega ci watch --workflow deploy.yml --ref main # the newest dispatched run, from 10 min before
```

Until that verdict mail, a sentinel idle on it shows as `waiting`, not `silent`, and its parent
is not woken. After it: act, then report (`vega post done`), or you count as silent again.

- `vega ci`: cached watches, verdict and bucket counts; GitHub rate limit. No API call.
- `vega ci show <target>`: the checks of the current head, failed first.
- `vega ci unwatch [<target>]`: stop early. A watch also ends when your pane closes.
- After a red mail: one `gh run view <id> --log-failed` for the failed job (GitHub), or the failed
  check's link in the PR (Forgejo), not a loop.
- After a red verdict, fix and push at once. If the repository refuses that push while the red run
  still continues, cancel it (`gh run cancel <id>`), then push; do not wait for it to finish. Wait
  only behind a `vega ci watch`, never with a polling loop.
- A Forgejo check that says `Pending approval` waits for a maintainer's `/ok-to-test` comment.
