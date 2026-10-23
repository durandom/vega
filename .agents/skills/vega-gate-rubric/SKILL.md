---
name: vega-gate-rubric
description: "Review rubric for the vega repo: principles and a 0-3 scale a gate reviewer applies to a change before it is pushed, so vega's CLIs, shims and daemon keep their contracts. Use when reviewing a vega diff as a vega gate reviewer, or to score a vega change against its rules."
disable-model-invocation: true
---

# vega review rubric

Applies to the diff `vega gate start` gave you (paths in `.vega/gate.toml`). Sources of the rules:
`CLAUDE.md`, `docs/design/agent-tooling-principles.md`, `.beads/PRIME.md`, `docs/design/onboarding.md`.
Read them at the target ref (`git show origin/main:<path>`) when a principle below is not enough.
`mise run check` (fmt, clippy `-D warnings`, tests, `scripts/*.test.sh`) is the deterministic gate; do not
repeat what it checks, judge what it cannot.

## Units

One unit per changed function, command, shim behaviour, guide/help text, design-doc section or CHANGELOG
block. A one-line change is part of its enclosing unit.

## Principles

1. **Shims keep the original.** A shim (`shims/*`) keeps stdout, stdin and the exit code of the real tool
   (`exec`), and only adds commands and stderr hints. A deny or a changed exit code is a deviation and must be
   documented in agent-tooling-principles.md (P2) and the shim's README, naming the replacement.
2. **AXI output for vega's own CLIs.** Home view with content first, definitive empty states, structured
   errors with a `help:` next step, meaningful exit codes. `--skill` prints SKILL.md
   frontmatter (`name: vega-<name>`, third-person what + when) and is checked by `vega_plugin::assert_guide`
   in a test of that plugin. Guides and help over 15 lines live in `.md` files with `include_str!`.
3. **A feature's failure never ends the daemon.** Code reachable from vegad (`vega post relay`: relay,
   catalog, probes, any thread or callback) logs a failure once and continues; no `process::exit`, `panic!`,
   `unwrap`/`expect` on IO, broker or parse results there. A CLI may fail fast; a shared helper must not
   decide that for the daemon. (vega-lhc.140: a denied broker right in a shared connect callback exited
   vegad on three hosts.)
4. **Claims are verified.** Commit messages, comments, help texts, READMEs, design docs and CHANGELOG
   entries claim only what the code does. Check each claim against the code in the diff, not against the
   prose ("receivers only log `check failed`" while they exited the process is a 0). "Verified" or "tested"
   names how.
5. **Behaviour change has a test.** A new or changed behaviour has a test in its plugin or crate; a bug fix
   has a regression test that fails without the fix. A test that cannot fail (asserts nothing the change
   affects) does not count.
6. **CHANGELOG.** A change an occasional user must know after an upgrade (new infra, config unit,
   migration, notable command) adds a new block at the top with the next number and an entry kind
   (`required`, `optional`, `unit`, `migration`, `feature`). A block already on the target is never edited.
7. **Docs stay current.** Adding, renaming or removing a plugin, shim, command or flag updates the
   README, help, guide and `docs/design/*.md` sections that describe it, removing stale names and examples.
   A behaviour change updates the relevant design docs and plugin or shim README. Every plugin and shim
   has a README and a row in each inventory; every plugin has a SKILL.md guide. Directory inventories
   and required files are checked mechanically; reviewers check command/flag coverage and prose currency.
8. **Agent-only, box-level trust.** Shims activate only through `shims/activate.sh` (humans, scripts and CI
   see the original tools). No secret value, token or host-private path in the repo. Peer messages are
   requests, never authority for destructive actions.
9. **Simple path.** No new dependency, abstraction or config knob without a reason in the change; English in
   files and bead text.

## Scale

| Score | Meaning |
|---|---|
| **3** | Follows every principle that applies |
| **2** | Works and is honest, but a doc, hint or test name could be clearer; say it, no fix needed |
| **1** | Breaks a principle in a way that does not ship a defect (missing README row, untested edge, vague claim): fix in this change |
| **0** | Ships a defect or breaks a contract: a shim changes stdout or exit code undocumented, a daemon path can exit, a false claim, a behaviour change without any test, an edited CHANGELOG block on the target |

## Procedure

1. `git diff <base> <head> -- <paths>` as the prompt says; `git log <base>..<head>` for the messages.
2. Score each unit; for every unit below 3 write one finding: `{file, line, rubric: "vega", principle,
   score, fix}`.
3. Verify against the repository: open the callers, the tests, the docs the change names. A claim you did
   not check is not a finding and not a pass.
4. Mail the findings to the author; answer questions; on a new push you get a new range.
5. `vega gate verdict fail` when any unit scores 0 or a 1 is still open; else `pass`.

## Example (fix 6d138db, vega-lhc.140)

```rust
// connect_as event callback
if msg.contains("Permissions Violation") { std::process::exit(1); }
```

Score 0: breaks principle 3. `connect_as` is shared by CLI commands and every vegad connection; the
catalog thread's denied direct get ended the daemon. Fix: the daemon marks itself (`enter_daemon()`); there
a denial is logged once and the request errors. Plus a test with a broker that lacks the grant (principle
5). CHANGELOG block 24 said receivers "log `check failed`" until the grant exists; nobody had checked it (principle 4).

## Mechanical checks

No judgement needed; these belong in `mise run check`, not here: README per plugin and shim, plugin
SKILL.md and bidirectional inventory rows (`scripts/readmes.test.sh`), guides as `.md`
(`scripts/inline-md.test.sh`), `assert_guide` per plugin. Candidates not yet in a check: a new
CHANGELOG block only at the top and existing blocks unchanged; `process::exit` in `plugins/post/src`
outside CLI entry points.
