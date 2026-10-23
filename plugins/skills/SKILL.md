---
name: vega-skills
description: Lists and loads the vega plugin guides, and manages the personal skills library (show a library skill without installing, install or remove it in a project or globally via skills.sh, vendor new skills into the library). Use when looking for a guide or skill that is not loaded, when asked to install, remove, add or update a skill, or to check skill installs for drift.
---

# vega skills: guides and the skills library

- `vega skills`: index of the vega plugin guides.
- `vega skills NAME`: one plugin guide.

Library skills live under `vega skills lib`:

- `vega skills lib`: index of library skills with where each is installed (`project`, `global`, `both`, `no`).
- `vega skills lib show NAME`: print a library skill, headed by its read-only dir in the library clone; relative paths resolve there. No install needed.
- `vega skills lib install NAME... [-g]`: install from the library clone; its commit is the pin. Project scope copies into `.agents/skills` with a `.claude/skills` link; commit both and `skills-lock.json`. `-g` installs globally and records the name in `~/.config/vega/skills-global`.
- `vega skills lib remove NAME... [-g]`: uninstall.
- `vega skills lib list [-g]`: every skill the project (or machine) sees, one row each: origin (`library`, `own`, `upstream` per skills-lock.json, `local`, `link`, `broken`), agent dirs holding it, status (doctor's checks) and a suggested action (`keep`, a fix, `vega skills lib install NAME`, or `vega skills lib add OWNER -s NAME`). Changes nothing.
- `vega skills lib sync`: clone or pull the library, restore the global set.
- `vega skills lib doctor [--fix]`: drift, missing Claude links, broken links, and copies in pi/codex dirs (`~/.pi/agent/skills`, `~/.codex/skills`, project `.pi/skills`, `.codex/skills`) that shadow `.agents/skills` and load a skill twice; exit 1 on findings, each with a fix. `--fix` removes shadows that are links or identical copies; a differing copy (`shadow_differs`) is left for you to compare and remove.
- `vega skills lib add OWNER -s NAME...`: vendor skills into the library clone; never install from upstream directly.
- `vega skills lib update [NAME...]`: re-vendor from the owners in the library's `skills-lock.json`.

`add` and `update` commit in the clone, never push (run the printed `git -C <clone> push`), and refuse a dirty clone.

The library remote is the first line of `~/.config/vega/skills-remote`; without it the index names the command that sets it.

Every skill has one owner repo. A project that owns a skill keeps its source in `skills/<name>/`, links `.agents/skills/<name>` -> `../../skills/<name>` (origin `own`) and the library vendors it with `vega skills lib add <owner/repo> -s <name>`. There `vega skills lib install <name>` is refused (`owned_here`): edit `skills/<name>` instead; `doctor` reports `library_stale` when the library copy is behind (push, then `vega skills lib update <name>`).

Errors print `error{code,message,hint}`: exit 1 runtime, exit 2 usage. Follow the hint.
