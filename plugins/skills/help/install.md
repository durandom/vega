Usage: vega skills lib install NAME... [-g]

Installs library skills with skills.sh from the vega-owned clone (cloned on first
use, never pulled here; its commit is the pin). Project scope (default): copies
into <git root>/.agents/skills with a relative .claude/skills link and updates
skills-lock.json, its entries pointing at the library remote (owner/repo for
GitHub) and the skill's path in it; commit them. -g: ~/.agents/skills + ~/.claude/skills, and the
name joins ~/.config/vega/skills-global so `vega skills lib sync` restores it.
Reinstalling an installed skill updates it to the clone's commit. Exit 1 when a
copy is missing, unlinked or still differs from the library (drift).
Refused (owned_here) in the repo that owns the skill (.agents/skills/NAME links
to its skills/NAME): the library copy would replace the live source.

Examples:
  vega skills lib install axi beads
  vega skills lib install caveman -g
