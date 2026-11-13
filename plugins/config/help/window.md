Examples:
  vega window
  vega config apply herdr-app.profiles
  vega window status

Profiles must be applied first. Opens $XDG_CONFIG_HOME/herdr-app-<name> (default
~/.config/herdr-app-<name>); reuses live labelled terminals and starts missing ones.
No arguments lists profiles. --skill prints the guide; -v/-V/--version prints version.

Manage tabs: add <name> --machine <machine|local>; rename <name> <id> <title>;
move <name> <id> <1-based-index>; close <name> <id>; reset <name>.
show <name> prints effective config, overlay and applied repo → effective diff JSON.
promote <name> [--repo <checkout>] writes a working-tree edit (default: current directory).
Example: vega window promote status --repo ~/src/vega
