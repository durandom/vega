# vega config: vega owns agent, herdr and dcg config

Status: accepted
Date: 2026-10-15
Bead: vega-lhc.107 (Slayer decision 2026-10-14)
Parent: [repo-layout.md](repo-layout.md); builds on [theme-symbols.md](theme-symbols.md) "herdr's config",
[skills-management.md](skills-management.md)

## Context

The coding agents' config came from three places: chezmoi (instructions, output styles, two skills, a
`run_onchange` script that merges portable keys into Claude's `settings.json`), `scripts/install-cli.sh`
(the shim activation in Claude's and pi's settings), and vega plugins (`vega mux hook install`,
`vega theme herdr`, `vega skills lib sync`). Two writers for one file drift: on proteus, chezmoi still
manages `~/.claude/skills/git-assisted-by/SKILL.md` while `vega skills lib sync` replaced the directory
with a link (`chezmoi status`: `MM`). The decision: all coding-agent config leaves chezmoi and is
written by one vega plugin, `vega config`, from sources in this repo. `vega deploy` applies it on every
host.

## Units

A unit is one target file (or a small set of files of one kind) with one ownership rule. Unit names
select units on the command line; `hooks` and `instructions` are groups. Sources are paths relative to
`~/.config/vega/config/` (local) or the repo's `config/` (default); see "Sources".

| Unit | Target | Ownership | Source |
|---|---|---|---|
| `herdr` | `~/.config/herdr/config.toml` | whole file; herdr-owned keys keep their live value | `herdr/config.toml` + `[symbols]` of `~/.config/vega/theme.toml` |
| `dcg.config` | `$XDG_CONFIG_HOME/dcg/config.toml` | whole file | `dcg/config.toml` |
| `dcg.packs` | `$XDG_CONFIG_HOME/dcg/packs/<name>.yaml` (also `.yml`) | whole file each; other live packs untouched | `dcg/packs/` |
| `claude.md` | `~/.claude/CLAUDE.md` | whole file | `instructions/*.md`, parts per agent |
| `codex.md` | `~/.codex/AGENTS.md` | whole file | same |
| `pi.md` | `~/.pi/agent/AGENTS.md` | whole file | same |
| `claude.styles` | `~/.claude/output-styles/<name>.md`, one per source file | whole file each; other files in the dir untouched | `claude/output-styles/` |
| `claude.settings` | `~/.claude/settings.json` | key-merge: the top-level keys of the source, re-asserted only when their source value changes | `claude/settings.json` |
| `claude.hooks` | `~/.claude/settings.json` | key-merge: vega's `PreToolUse` (ask-parent) and `SessionStart` (shim activation) entries | code (plugins/config) |
| `codex.hooks` | `~/.codex/hooks.json` | key-merge: vega's `PreToolUse` entry | code |
| `pi.prefix` | `~/.pi/agent/settings.json` | key-merge: the activation line in `shellCommandPrefix` | code |
| `skills` | `~/.agents/skills/<n>`, `~/.claude/skills/<n>` | the skills library; `vega skills lib sync` | `~/.config/vega/skills-global`, the library clone |
| `agentsview` | `LOCAL-SETUP.md` in `~/.claude/skills/agentsview-finding-history/` and `~/.agents/skills/agentsview-finding-history/` | whole file each, only where agentsview generated the skill (`SKILL.md` exists) | `agentsview/LOCAL-SETUP.md` |

Groups: `instructions` = `claude.md codex.md pi.md`; `hooks` = `claude.hooks codex.hooks pi.prefix`.

### Destructive Command Guard

`dcg.config` and `dcg.packs` are personal whole-file units. Explicit config apply/deploy
manages them, but install remains hooks-only; onboard preserves differing existing files.
`vega config diff dcg.config dcg.packs` previews the change. Local sources use the same
`dcg/` layout under `$XDG_CONFIG_HOME/vega/config/`; default packs and local YAML additions
are combined by filename. `oc.yaml` is imported byte-for-byte from the existing host pack.
`allowlist.toml`, history and other dcg state remain user-owned.
The default pack globs follow a nonstandard `$XDG_CONFIG_HOME`; local config sources
keep their explicitly chosen paths.
`vega config init dcg/config.toml` copies the literal source defaults. With a nonstandard
`XDG_CONFIG_HOME`, adjust its `packs.custom_paths` from `~/.config/dcg/packs/` to the
actual pack directory before applying the local source.

dcg's [custom pack schema](https://github.com/Dicklesworthstone/destructive_command_guard/blob/main/docs/custom-packs.md)
renders `description` and `explanation`, but currently does not render custom `suggestions`.
Built-in packs cannot be replaced. We use an `overrides.block` rule and its `reason`, evaluated
before built-in packs, to direct `git branch -d`, `-D` and `--delete` to `vega uac prune` (reviewed merged branches)
or `wt remove <branch>` (worktree cleanup). This adds a denial; it does not allowlist
`core.git:branch-force-delete`, relax its policy, or permit `-D`. Other core blocks remain.
The hint rule scans complete arguments in direct Git invocations and common literal command/env
prefixes. Delete flags are caught after other branch arguments too, including `--merged` and
`--no-merged`: dcg 0.9.2's core listing safe patterns can otherwise allow those combinations.
Quoted words and required option values such as `--format` are consumed as data, so echoed
examples and read-only formatting do not acquire new denials. dcg also applies the rule to
extracted inline-shell bodies such as `bash -c 'git branch -d topic'`.
The git shim does not redirect branch deletion: dcg runs before the shim, and a shim must
preserve git's arguments, stdout and exit status. Safe cleanup is an explicit workflow.

The explain fixture in `plugins/config/tests/fixtures/dcg-explain.json` runs through the real
dcg binary when installed, in a temporary HOME with hook self-healing disabled. Config unit
tests verify apply, backups, local overrides, idempotence and chezmoi refusal in temporary homes.

### Ownership rules

- **Whole file**: vega renders the full content; a live file that differs is replaced. Edit the
  source (local, after `vega config init`), never the target: `vega config` shows the drift as `differs`, `vega config diff` the
  diff. A target that is a symlink (chezmoi's `symlink_` entries, e.g. pi's `AGENTS.md ->
  ~/.claude/CLAUDE.md`) counts as differing and is replaced by a regular file, never written through.
- **Key-merge**: for files a tool writes too (`settings.json`, `hooks.json`). vega parses the file,
  changes only its own keys or entries and keeps every other key, entry and the key order. vega's hook
  entries are found by the command they run (`hook ask-parent`, `shims/activate.sh`), so a changed
  binary path updates the entry instead of adding a second one. A file that does not parse is an error
  and stays untouched. A missing `settings.json` is created only when its tool's directory exists.
- **Seeded keys** (`claude.settings`): portable preferences the user also switches by hand
  (`outputStyle`). vega sets a key when it is missing or when its *source* value changed since vega last
  set it (`$XDG_STATE_HOME/vega/config.json` keeps the last value per key). Between source changes a
  local edit survives; `vega config` shows it as `local` instead of drift. This is the semantics of the
  chezmoi `run_onchange_74-merge-claude-settings.sh` it replaces. `model` and `theme` stay out of it
  (the theme belongs to `vega theme`).
- **Library**: global skills stay with `vega skills lib` (skills-management.md); `vega config` reports
  them and runs `vega skills lib sync`. The repo does not list them; `~/.config/vega/skills-global` does.

### Tool-written state survives

| Writer | What | How it survives |
|---|---|---|
| herdr | `[ui] agent_panel_sort` (UI toggle) | `HERDR_OWNED`: the live value replaces the template's |
| Claude Code | everything else in `settings.json` (plugins, permissions, statusLine, survey state, other hooks) | key-merge |
| codex | other hooks in `hooks.json`; `config.toml` (trust, hook approvals) | key-merge; `config.toml` is not a unit (folder trust stays in `vega mux spawn`, plugins/mux/src/trust.rs) |
| pi | `settings.json` except the prefix line; an existing prefix stays after vega's line | key-merge |
| agentsview | the generated `SKILL.md` (and its LOCAL-SETUP pointer) | not a target; vega owns only the sidecar |
| skills.sh | skill copies, links and `.skill-lock.json` | `vega skills lib` owns them |

Every write first copies the previous file to `<file>.vega-bak` (overwritten on the next write) and
replaces the file atomically (temp file + rename in the same directory).

## Chezmoi refusal

`apply` refuses a unit while chezmoi manages any of its targets: one `chezmoi managed -p absolute`
call per run, compared by exact path. The unit reports `managed (chezmoi)` and the next step (the
dotfiles drop the source, then `vega config apply <unit>`); the other units still apply. Without
chezmoi nothing is managed. Two exceptions, because chezmoi cannot see them or vega wrote them first:

- `claude.settings`: chezmoi's `run_onchange_74` is a script, not a managed file. Both write the same
  values until the dotfiles delete the script.
- `skills`: `vega skills lib sync` already writes these directories on every deploy. An overlap is a
  warning in the status (`chezmoi also manages …`), not a refusal, so hosts that keep chezmoi longer
  (deimos, io) still get their skills.

## Per-host differences

None of the sources are templates per host. What differs comes from the host's own state:

- `herdr`: colours, host labels and the sentinel icon from the host's `theme.toml` (as before).
- `agentsview`: only where `agentsview skills install` generated the skill (proteus, ganymede today).
- Key-merge units: the rest of each file is the host's own.
- A unit whose tool is missing is skipped: `herdr` without herdr; the claude, codex and pi units
  without `~/.claude`, `~/.codex` or `~/.pi/agent`; `pi.prefix` without `~/.pi/agent/settings.json`.
  The dcg units remain available before dcg is installed, so onboarding can seed their defaults.

The chezmoi templates these targets came from use only `.chezmoi.homeDir` (the `symlink_` targets);
`.chezmoiignore` adds `hostname` (agentsview on proteus/ganymede) and `machine_type` (`windows`: only
`CLAUDE.md` and output styles; `linux-dev`: no `settings.local.json`). Rendered for proteus:
`homeDir=/Users/mhild`, `hostname=proteus`, `machine_type=mac-personal`; nothing in the content
changes per host.

## Instructions: one source, parts per agent

`config/instructions/` holds one Markdown file per top-level section. Each agent's file is the
concatenation of its parts, separated by one blank line:

| Agent | Parts |
|---|---|
| claude, pi | `role`, `interaction`, `reasoning`, `determinism`, `token-efficiency`, `vega` |
| codex | `git-commits`, `vega` |

Claude and pi read the same text, as before (pi's file was a link to `CLAUDE.md`). Codex gets
`git-commits` because it has no commit-trailer skill trigger like Claude's. The `vega` part is shared:
codex now also gets the `from_parent` exception that only `CLAUDE.md` had.

## Sources: repo defaults, local overrides

The repo's `config/` holds the Slayer's personal defaults, as best practice to start from; it is not
the live config. The live source is local, like `theme.toml`: `~/.config/vega/config/`
(`$XDG_CONFIG_HOME/vega/config/`) with the same layout. Per source file, the local file wins;
otherwise the repo default applies. The binary embeds the defaults (`include_str!`), so a host renders
what its deployed commit holds, without a checkout path, and defaults that are not overridden keep
flowing in with new vega versions.

- Output styles are the union of the defaults and the local `claude/output-styles/*.md`: a local file
  overrides the default of the same name or adds a style.
- Instruction parts per agent stay fixed in code; a local part file replaces that part's text.
- `vega config init <source>` copies a default to its local path for editing (refuses an existing
  local file). `vega config` lists which sources are local; the rest are defaults.
- Local overrides travel with `vega deploy` (Slayer decision 2026-10-15): before it runs
  `vega config apply` on a target, deploy replaces the target's `~/.config/vega/config/` with the
  deploying host's (proteus): the files go base64-encoded inside the deploy's ssh script, into a
  fresh `config.vega-new` that then replaces the directory. A file deleted on the deploying host is
  deleted on the target; without a local directory the target's is emptied. Edit overrides on the
  deploying host only: edits on a target are lost on the next deploy. No vegad sync.

```
config/                         # repo defaults; ~/.config/vega/config/ overrides per file
  herdr/config.toml             # moved from herdr/config.toml
  instructions/<part>.md
  claude/output-styles/<name>.md
  claude/settings.json          # seeded keys
  agentsview/LOCAL-SETUP.md
  dcg/config.toml
  dcg/packs/oc.yaml
  capacity/windows.toml         # window scopes; read by vega capacity, no unit (capacity.md)
```

A test fails when a file under `config/claude/output-styles/` or `config/instructions/` is not
embedded.

## Command surface

```
vega config                    # per unit: target, ownership, state (ok, differs, missing, local, managed, skipped)
vega config diff [UNIT...]     # unified diff of every target that would change
vega config apply [UNIT...]    # apply; per unit: written, unchanged, managed, skipped, rejected, failed
vega config print UNIT         # the rendered file of a single-file unit
vega config init SOURCE        # copy a default source to ~/.config/vega/config/ for local editing
vega config infra              # the infra vega needs: required/optional, state, next step (onboarding.md)
```

Exit 1 when a unit fails or is rejected; `managed` and `skipped` are not failures. Moved commands:

- `vega theme herdr [--print|--apply]` → thin alias for `vega config diff|print|apply herdr`.
  `vega theme` keeps the runtime mode switching.
- `vega mux hook install [--codex]` → thin alias for `vega config apply hooks`. `vega mux hook
  ask-parent` (the hook itself) stays in mux.
- `scripts/install-cli.sh` no longer edits settings files; it runs `vega config apply hooks`.

Personal units (`instructions`, `claude.styles`, `claude.settings`, `agentsview`, `dcg.config`,
`dcg.packs`) are the Slayer's own preferences. `vega deploy apply` and an explicit `vega config apply`
manage them; `vega onboard apply` seeds defaults while preserving differing files and user settings.
`mise run install` and `scripts/install-cli.sh` never apply them: another user of this repo who installs
vega must not get the Slayer's `CLAUDE.md` replacing their own. `install-cli.sh` stays hooks-only
(`vega config apply hooks`). `herdr` and `skills` also stay out of that installer and remain available
to deploy, onboarding and explicit config apply.
- `vega deploy apply` copies the local overrides (column `overrides`: file count) and runs
  `vega config apply` instead of `vega skills lib sync` and `vega theme herdr --apply`; its `config`
  column is the apply's `summary:` line. Neither fails the deploy.

## Cut-over per host

Order per host; the UAC drives it, the dotfiles UAC removes the chezmoi sources.

1. Merge and install vega (`mise run install` on proteus, `vega deploy apply <host>` elsewhere).
2. `vega config diff` on the host: check that only the expected changes show (see the bead notes for
   proteus).
3. Dotfiles: `chezmoi forget` the targets and delete their sources: `dot_claude/CLAUDE.md`,
   `dot_claude/output-styles/`, `dot_claude/skills/`, `dot_agents/skills/`, `dot_codex/AGENTS.md`,
   `dot_pi/private_agent/`, `run_onchange_74-merge-claude-settings.sh`; `run_onchange_71` keeps
   generating the agentsview `SKILL.md` but must stop including the sidecar's chezmoi source (hash it
   from vega's copy or drop the trigger).
4. `vega config apply` on the host; `vega config` shows every unit `ok`.
5. Remove `~/.pi/agent/skills/git-assisted-by` if chezmoi left it (pi reads `~/.agents/skills`; the
   duplicate makes pi warn).

Hosts: proteus and ganymede first; deimos and io after the Slayer's OK (absorbs vega-lhc.104, herdr on
deimos and io). io's native Windows home gets `CLAUDE.md` and output styles from chezmoi today; vega
does not run there, so that part stays in chezmoi until vega does.

## Rejected

- A host-keyed template language for the sources: no source differs per host today; host state
  (theme.toml, installed tools) covers what does.
- A manifest file listing targets: the units differ in kind (render, merge, library), so code lists
  them and a test keeps code and `config/` in step.
- Writing through symlinks: it would write chezmoi's link target (pi's `AGENTS.md` into `CLAUDE.md`).
- Moving the global skill list into the repo: `vega skills lib` owns it; a second list would drift.

## Service endpoints

Endpoints are local configuration, independent of the rendered agent config
units. Neither `vega config apply` nor onboarding guesses private hostnames.
Onboarding reports a missing broker or Dolt remote even when Tailscale is installed.

Set an environment variable, or write the endpoint on the first line of its file
under `$XDG_CONFIG_HOME/vega` (normally `~/.config/vega`). Environment values take
precedence; blank values fall through to the file. No endpoint is enabled by default.

| Service | Environment | File | Example |
| --- | --- | --- | --- |
| NATS (post and registry) | `VEGA_NATS_URL` | `nats.url` | `nats://nats.<tailnet>:4222` |
| Dolt remotesapi | `VEGA_BD_REMOTE` | `bd.remote` | `http://dolt.<tailnet>:8080` |
| Dolt SQL (creating databases) | `VEGA_BD_SQL` | `bd.sql` | `dolt.<tailnet>:3306` |
| Janitor embeddings | `VEGA_JANITOR_EMBED_ENDPOINT` | `janitor-embed.url` | `https://embedding.<tailnet>/v1` |

Before upgrading existing hosts, save their current service addresses in these
files or retain their environment overrides. Use real deployment addresses in
local config, not the placeholders above. Keep the files in private dotfiles and
provision them on every host that uses the service. Existing provisioned beads
projects retain their recorded remote; `vega bd init` needs the configured remote,
and creating a missing database also needs the SQL endpoint. Without a broker,
vegad enters local mode only after a completed identity migration; unmigrated or
failed-cutover hosts remain in identity quiesce. Local mode enables mux's local
paste fallback. See [Local mode](vegad.md#local-mode-vega-lhc118).
Embedding requires an endpoint plus the existing secret and model configuration.

The public agentsview sidecar is an example. Before applying it on an existing
host, retain its personal instructions in
`$XDG_CONFIG_HOME/vega/config/agentsview/LOCAL-SETUP.md` (normally
`~/.config/vega/config/agentsview/LOCAL-SETUP.md`). Without that source, applying
the agentsview unit replaces installed sidecars with the generic example.
Private server access and token paths belong in private dotfiles.
