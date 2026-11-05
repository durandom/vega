Usage: vega uac group [add <path|project-id> | discover <dir>]

Manage .vega/group.toml in the current coordination repository. No arguments
lists the canonical member IDs (including an explicit zero count).
add resolves a local Git checkout or accepts an already canonical project ID.
discover adds repositories directly under dir; it does not recurse.
Members are sorted and deduplicated; the coordination project is implicit.

claim records this pane as UAC for every member, refusing live conflicts.
release removes its group records. To remove members: release, edit, claim.
Member routing works locally on inbox-only hosts without registry access.
Land member work with land BEAD --repo <member-main-checkout> --branch BRANCH.

Flags: -h, --help prints this help. Unknown arguments fail before writes.
Examples:
  vega uac group add ../service
  vega uac group discover ~/src/team
  vega uac group
