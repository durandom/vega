# Repo layout

Status: accepted
Date: 2026-10-06
Parent: [agent-tooling-principles.md](agent-tooling-principles.md)

## Rule

Top-level directories are sorted by **kind of artifact**, not by feature or by host app.

| Directory | Holds | Example |
|---|---|---|
| `vega/` | The umbrella CLI (P1) | `vega`, `vega help <name>` |
| `plugins/<name>/` | One `vega-<name>` crate per plugin (P1) | `plugins/mux`, `plugins/open` |
| `shims/<tool>/` | Agent-only shims; never on a human's `PATH` (P3) | `shims/herdr`, `shims/gh`, `shims/tea`, `shims/bd` |
| `crates/<name>/` | Libraries shared by plugins and shims | `crates/vega-herdr`, `crates/vega-shim` |
| `githooks/` | Global git hooks, installed as config-based hooks (`hook.vega-<name>.*`); stderr hints only, stdout and exit codes untouched (P2) | `githooks/wt-guard` |
| `skills/<name>/` | Agent skill stubs (P5) | `skills/vega` |
| `vscode/` | The VS Code extension, a companion for all plugins | reveal, diff URI handler |
| `docs/design/` | Principles and design notes | this file |
| `config/` | Sources of the agent, herdr and dcg config `vega config` renders ([config.md](config.md)) | `config/herdr/config.toml`, `config/instructions/`, `config/dcg/` |
| `repos/` | Reference clones for inspiration; gitignored | `repos/herdr` |

## Consequences

- A plugin may expose itself to other hosts from its own directory. `plugins/open/herdr-plugin.toml` sits next to its crate because herdr runs `./bin/vega-open` relative to the manifest.
- Host-side companions that serve several plugins get their own top-level directory named after the host (`vscode/`). A new one (for example a Zed extension) would be `zed/`, not a subdirectory of a plugin.
- Build output stays inside the artifact's directory and is gitignored there (`plugins/open/bin/`, `vscode/dist/`).
- `mise run check` and `mise run install` cover every artifact; adding a directory means adding it to both tasks.
- `scripts/readmes.test.sh` runs the offline inventory check (`scripts/check-readmes.sh`) and fixture regressions
  as part of `mise run check`: each plugin and shim has exactly one linked directory row in the root README,
  shims also appear in `shims/README.md`, and rows must point to existing directories. Every plugin and shim
  has a README; every plugin also has a SKILL.md guide. The gate rubric covers command/flag documentation
  currency, which directory checks cannot infer from prose.
- Coding-agent, herdr and dcg config is vega's (vega-lhc.107, vega-lhc.174,
  [config.md](config.md)): sources in `config/`, including `config/dcg/config.toml` and
  `config/dcg/packs/`, rendered or key-merged by `vega config` and rolled out by `vega deploy`.
  Config outside these owned units stays with its existing owner, such as the dotfiles (chezmoi);
  where vega does not own a target it reads it and prints the snippet a change needs.

## Rejected

- `integrations/{herdr,vscode}/`: separates the herdr manifest from the crate it launches.
- `extensions/vscode/` or `editors/vscode/`: one level of nesting for a category with a single member.
