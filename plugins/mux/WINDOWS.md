---
name: vega-mux
description: Prepares native Windows Codex and Claude launches and saves their lineage. Use when a Unix coordinator spawns to a Windows hc machine.
---

# Native Windows target

Use `vega mux spawn <name> --machine <host> --kind codex|claude --cwd 'G:/src/repo'`
on a Unix coordinator. It selects policy/model and launches direct argv through hc.
This target supplies `prepare` and internal `record-spawn`; see `--help`.
Native harness .exe files must be on the target hc PATH. Worktrees and local
app/TUI commands are unsupported. Trust/permission dialogs stay with the user.
