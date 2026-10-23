vega gate: an LLM review before an agent's push

Usage: vega gate [command]

With no command: this repo's gate config (read from the push target, never from
the branch), the segments of the current branch with their pass state, and the
reviews running on this host.

Commands:
  check [--target <ref>]    what the git shim would do on a push, without pushing:
                            target, base, each segment's key and pass state
  start [--target <ref>]    start a reviewer for every segment without a pass:
                            a tab gate-<branch> in your workspace, verdict by mail
  verdict pass|fail [--findings <file.jsonl>]
                            the reviewer's verdict; only the pane `start` spawned
                            may give it; mails the author, closes the tab
  log [--since <7d>]        every gate bead: passes, fails, skips, no-reviewer
  gc                        remove clones of finished or day-old reviews

Target: the branch's upstream, else origin/HEAD, else origin/main.
Config: .vega/gate.toml at the target; no file, no gate. Beads repos only.
Records: one bead per segment review, label gate, external ref gate:<key>.

git push in agent sessions (the git shim) checks a pass per segment; none:
it runs `start` and fails the push (exit 1). VEGA_GATE=skip skips visibly
(VEGA_GATE_REASON=<why> is recorded); a gate error lets the push through
with one stderr line.

Flags:
  --skill      the agent guide (also `vega skills gate`)
  --version    version and build

Design: docs/design/gate.md
