---
name: vega-gate-default-rubric
description: Bare review rubric a vega gate reviewer applies when the repo's .vega/gate.toml names no rubric of its own. Use when reviewing a diff as a vega gate reviewer without a repo rubric.
disable-model-invocation: true
---

# Default gate rubric

Applies to the diff `vega gate start` gave you. The repo names no rubric of its own, so judge only these:

1. **Correctness.** The change does what it sets out to do; no bug, crash, data loss or broken contract
   that a reader of the diff can see.
2. **Tests for changed behaviour.** A new or changed behaviour has a test; a bug fix has a regression test.
   A test that cannot fail does not count.
3. **Claims match code.** Commit messages, comments, help texts and docs in the diff claim only what the
   code does.

## Scale

| Score | Meaning |
|---|---|
| **3** | Follows every principle that applies |
| **2** | Works and is honest, but could be clearer; say it, no fix needed |
| **1** | Breaks a principle without shipping a defect: fix in this change |
| **0** | Ships a defect, breaks a contract or makes a false claim: blocks the push |

Findings: one JSON object per line, `{"file", "line", "rubric", "principle", "score", "fix"}`; `rubric` is
`default`, `principle` the number above. Only scores 0 and 1 need a finding.
