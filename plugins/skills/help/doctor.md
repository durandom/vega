Usage: vega skills lib doctor [--fix]

Checks the library setting and clone, this project (git root) and the global
skill dirs: copies that differ from the library (drift), .agents/skills entries
without a .claude/skills link, broken links, global-set skills not installed.
Exit 1 when it finds any.

In the repo that owns a skill (.agents/skills/NAME links to its skills/NAME) the
link is fine; `library_stale`: the library copy differs from skills/NAME. Push
the owner repo, then run the fix (`vega skills lib update NAME`).

Also checks agent-specific dirs that are read besides .agents/skills (global
~/.pi/agent/skills, ~/.codex/skills; project .pi/skills, .codex/skills): an
entry named like one in .agents/skills of that scope loads the skill twice.
`shadow`: a link, or a copy identical to .agents/skills. `shadow_differs`: a
copy with other content; compare and remove it by hand.

  --fix   remove every `shadow` entry and print what was removed; differing
          copies are never touched. Without it doctor changes nothing.

Examples:
  vega skills lib doctor
  vega skills lib doctor --fix
