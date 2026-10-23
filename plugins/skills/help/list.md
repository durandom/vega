Usage: vega skills lib list [-g]

Every skill this project (git root) sees, or with -g the global dirs (HOME).
One row per name. Changes nothing; exit 0 whatever it finds.

  origin   library: in the skills library (or skills-lock.json says so)
           own: .agents/skills/NAME links to this repo's skills/NAME; this
           repo owns the skill (source: skills/NAME)
           upstream: in skills-lock.json from another source (column source)
           local: a real dir not in the library
           link: a symlink (source: its target)
           broken: a dangling link (source: its target)
           claude.ai: ~/.claude/skills/synced, one row for all synced skills
  dirs     the agent dirs holding it: .agents/skills, .claude/skills, and
           .pi/skills, .codex/skills (-g: .pi/agent/skills, .codex/skills)
  status   doctor's checks: ok, drift, shadow, shadow_differs,
           missing_claude_link, broken_link, library_stale; several joined by +
  action   keep (from the library, or own and in the library), a fix (the
           first finding's, as doctor prints it), `vega skills lib install
           NAME` (the library has it, this copy is not from it), or `vega
           skills lib add OWNER -s NAME` (the library lacks it; OWNER from the
           git remote or skills-lock.json)

Examples:
  vega skills lib list
  vega skills lib list -g
