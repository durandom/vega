# Git commits

Every commit you create must include an `Assisted-by` trailer. Use the exact
harness and model ID when known:

```
git commit -m "Subject" --trailer "Assisted-by: codex:<model>"
```

If the model ID is unavailable, use `Assisted-by: codex` without a model suffix.
Do not guess an ID, ask for approval, or block work solely on missing model
metadata. Reasoning effort (such as `low`) is not part of the model ID.

Do not use `Co-authored-by` for the AI. Do not add `Signed-off-by` as the agent.
See `~/.agents/skills/git-assisted-by/SKILL.md`.
