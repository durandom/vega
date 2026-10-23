# Local setup: agentsview

Agentsview generates the `agentsview-finding-history` skill. Vega installs this
sidecar beside that skill when it is present.

Use your deployment's server URL, for example `https://agentsview.example.com`,
and its authentication instructions. Keep tokens and host access details outside
this repository.

Put deployment-specific instructions in
`$XDG_CONFIG_HOME/vega/config/agentsview/LOCAL-SETUP.md` (normally
`~/.config/vega/config/agentsview/LOCAL-SETUP.md`), then run
`vega config apply agentsview`. Local source files take precedence over this
example. Store them in private dotfiles if they need to travel between hosts.
