Usage: vega skills lib
       vega skills lib show NAME
       vega skills lib list [-g]
       vega skills lib install|remove NAME... [-g]
       vega skills lib sync
       vega skills lib doctor [--fix]
       vega skills lib add OWNER -s NAME...
       vega skills lib update [NAME...]

Without COMMAND: the library skills, where each is installed (project,
global, both, no) and their descriptions.

Commands (run `vega skills lib <command> --help`):
  show      print a library skill without installing it, headed by its dir
            in the library clone (relative paths resolve there)
  list      every skill this project (or -g: this machine) sees: origin,
            agent dirs, status, suggested action; changes nothing
  install   copy library skills into this project, or globally with -g
  remove    uninstall skills from this project, or globally with -g
  sync      clone or pull the library, restore the global set
  doctor    report drift, missing Claude links, shadowing copies, library
            problems; --fix removes safe shadows
  add       vendor skills from their owner repo into the library clone, commit
  update    re-vendor skills from the owners in the library's skills-lock.json,
            commit

Settings in $XDG_CONFIG_HOME/vega (~/.config/vega), not synced across machines:
  skills-remote   the library's git URL (first line)
  skills-global   skills installed with -g, one per line (sync restores them)
Clone: $XDG_DATA_HOME/vega/skills (~/.local/share/vega/skills).

Examples:
  vega skills lib
  vega skills lib show caveman
  vega skills lib install axi beads
