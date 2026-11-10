# Onboarding: the infra model, vega onboard, the changelog

Status: accepted
Date: 2026-10-15
Bead: vega-lhc.112 (Slayer decision 2026-10-15)
Parent: [config.md](config.md); follows [agent-tooling-principles.md](agent-tooling-principles.md)

## Context

vega grew inside one estate: a tailnet, a NATS broker and a Dolt server on titan, `vega deploy` to the other
herdr machines. The next user is outside it. They work along half-way or not at all, onboard from scratch
now and then, or pull an upgrade after weeks and want to know what changed. They have none of the estate's
services, and the repo is private (they need read access on GitHub to clone it).

The estate stays as it is. Three parts serve the outside user:

1. **Infra model** in `vega config` (plugins/config, `vega_config::infra`): every component the plugins need,
   required or optional, with a local check, what it enables and how to get it.
2. **`vega onboard`** (plugins/onboard): first run and upgrade in one home view, plus `apply`.
3. **README prompts**: one copy-paste prompt for a fresh agent, one for an upgrade.

No bundled skill: `vega skills onboard` serves the plugin's guide (P5).

## Infra model

Each component has a name, a level, a check, what it enables and how to get it. Checks are local and cheap:
a binary on `PATH`, a directory, a config file or env var, one `git --version` and the `chezmoi managed`
call `vega config` already makes. No default check touches the network: `vega post` and `vega bd` report
reachability themselves, and a probe that waits on an unreachable tailnet host would make the home view slow
exactly for the outside user.

The explicit `vega config infra --fleet` diagnostic additionally checks hc machine
names against registry hosts over the network. Names must match; it reports
`hc machine rename <old> <host>` for mismatches without changing any machine.

Required means the build tools, vega CLI and an agent. Optional means a backend or feature set;
hc/herdr-app and the legacy herdr TUI are alternative terminal interfaces.

| Component | Level | Check | Enables | How to get it |
|---|---|---|---|---|
| `mise` | required | `mise` on PATH | `mise run install-cli`, `mise run check` | https://mise.jdx.dev/installing-mise.html |
| `cargo` | required | `cargo` on PATH | building vega (toolchain pinned in `rust-toolchain.toml`) | https://rustup.rs |
| `git` | required | `git` on PATH; version ≥ 2.54 for the wt-guard hook (a note, not a failure) | clone, upgrade, worktrees | the OS package manager |
| `vega` | required | `vega` on PATH | every `vega <plugin>` call | `mise run install-cli`; put `~/.local/bin` on PATH |
| `agent` | required | one of `~/.claude`, `~/.codex`, `~/.pi/agent` | the coding agents vega runs and configures | install Claude Code, Codex or pi and start it once |
| `hc` | optional | `hc` on PATH | hc terminals, agent labels and the herdr-app backend | https://github.com/herdrdev/herdr-core |
| `vegad-start` | optional | no old login job file (problem if present) | no vegad login job; herdr-app and the TUI start vegad | `vega post relay install` removes it; `vega post relay status` for runtime state |
| `herdr-app` | optional | `herdr-app` on PATH | vega-sidebar sidebar and vega-inbox decisions | install herdr-app, then `vega config apply herdr-app` |
| `python3` | optional | `python3` on PATH | herdr-app badge reader and layout maintenance | the OS package manager |
| `herdr` | optional | `herdr` on PATH (or `HERDR_BIN_PATH`) | legacy backend: agents in panes | https://github.com/herdrdev/herdr |
| `chezmoi` | optional | absent, or managing none of vega's config targets | `vega config apply` writes every unit | drop vega's targets from the dotfiles (config.md, cut-over) |
| `wt` | optional | `wt` on PATH | `vega mux spawn --worktree` with worktrunk (else herdr worktrees) | install worktrunk |
| `dcg` | optional, recommended | executable on PATH plus valid `PreToolUse` command hooks in present Claude `settings.json` and Codex `hooks.json` | destructive command protection before shell execution | [dcg installation](https://github.com/Dicklesworthstone/destructive_command_guard#installation); register hooks with its installer, then `dcg doctor`; Claude repair: `dcg install` |
| `bd` | optional | `bd` on PATH | beads: the herdr-app inbox, UAC decisions, the bd shim | https://github.com/gastownhall/beads |
| `gh` | optional | `gh` on PATH | `vega ci` for GitHub, the gh shim, `janitor --mine` | https://cli.github.com |
| `tea` | optional | `tea` on PATH | `vega ci` for Gitea/Forgejo | https://gitea.com/gitea/tea |
| `nats` | optional | `VEGA_NATS_URL` or `~/.config/vega/nats.url` set | `vega post`, vegad (`relay`): cross-host mail, `ci watch` verdicts, beads sync, theme follow, `review` notes | own broker: `mise run nats`, URL into `~/.config/vega/nats.url`; estate: set `VEGA_NATS_URL` or `~/.config/vega/nats.url`, ask the Slayer for a key, `vega post keygen` |
| `tailnet` | optional | `tailscale` on PATH or the macOS app | private services, `vega deploy` to other hosts | the estate's admin invites you |
| `dolt` | optional | `dolt` on PATH and a remote (`VEGA_BD_REMOTE` or `~/.config/vega/bd.remote`) | `vega bd` sync of a project's beads | https://docs.dolthub.com/introduction/installation; without it beads stay local |
| `uv` | optional | `uv` on PATH | `vega lanista` (model catalog) | https://docs.astral.sh/uv/ |
| `agent-browser` | optional | `agent-browser` on PATH | `vega review` | its own install docs |
| `skills.sh` | optional | `skills` or `npx` on PATH | `vega skills lib` (the global skills library) | Node.js (`npx`) |
| `op` | optional | `op` on PATH | `vega secrets` adopt/rekey from 1Password | https://developer.1password.com/docs/cli/ |
| `token-burn` | optional | `token-burn` on PATH (or `VEGA_TOKEN_BURN`) | `vega capacity`, the capacity line of `vega uac status`, the `vega mux spawn` warning ([capacity.md](capacity.md)) | github.com/durandom/token-burn, then `token-burn install` for its daemon |
| `moshi-hook` | optional | `moshi-hook` on PATH; from Homebrew, service registered ([upgrade.md](upgrade.md)) | phone notifications and approvals through Moshi; `vega upgrade` keeps it current | `brew install rjyo/moshi/moshi-hook` |

Not components: `ssh` (part of the OS, only for `deploy` and remote panes), `diff`, `curl`, `hostname`.

The dcg probe reads hook JSON without executing dcg or installing anything. A mention of
dcg elsewhere in settings is insufficient: the hook must be a command for shell tools and
name `dcg` on PATH or an existing executable absolute dcg path. Missing, stale or malformed
hook entries produce a recommendation to use dcg's installer/doctor (Codex configuration:
`~/.codex/hooks.json`). Vega's own hooks do not register dcg. Its managed config units are
whole files, so onboard keeps the user's existing differing config and packs.

### First Herdr Core use on macOS

On each Mac, approve the first **Herdr Core** file-access privacy dialog on that
Mac’s screen before starting unattended agents. Existing Herdr.app permissions
do not transfer to Herdr Core. A remote spawn can appear hung in `getcwd` or file
access (including a checkout under `~/src`) while that dialog waits on the remote
screen; approving it on the initiating machine does not resolve it.

If terminals were created before the signed Core bundle upgrade, recreate those
terminals after the user has resolved access; restarting only the core does not
repair their old holders. Do not restart other users’ terminals to diagnose this.
vegad startup does not start an hc core or alter an existing core LaunchAgent.
`hc machine`/SSH owns remote core startup, including reconnects (Slayer decision,
2026-11-07). Do not install a competing hc login job; the operator removes the
experimental `dev.vega.hc-core` job on ganymede separately.

### Remote-only hosts

Run `vega post relay install` once on each host. It pins `vegad.env`, links the herdr TUI
hook when herdr is installed and starts vegad. It installs no login job (vega-0h4c.42) and
removes one from an earlier version. The app host's herdr-app vega mod starts vegad on itself
and, via `hc exec`, on every connected hc machine, so a remote-only host (a corp Mac without
GUI login) needs only `relay install` and a link from the app host. `vega config infra`
reports an old login job as the `vegad-start` problem; `vega post relay status` reports
whether the daemon actually runs. Do not run installers from sentinel worktrees.

When the OS hostname differs from the registry host, explicitly pin the registry
identity at install: `VEGA_POST_HOST=io vega post relay install` on WSL named
`io-wsl`. The installer always records the resolved host in `vegad.env` and warns
when a configured registry cannot verify it (an inbox-only host skips the check). Check `vega post relay status` for
`host: io`; hc machine names must also be `io`. Reinstall preserves the pin unless
the installing shell explicitly overrides `VEGA_POST_HOST`.

For remote inbox cards, install Python 3 on both machines and apply the updated
herdr-app mods on the app host. Existing connected hc machines supply their badge
files automatically. Answers and dismissals use that host's recorded checkout and
CLI versions; no additional registry grants are needed. Verify a remote decision
and answer after deployment. Keep machine names equal to vega registry host names.

### Degrading cleanly

What the survey of every plugin found (bead notes, 2026-10-15), and what this change adds:

- Without a broker URL, `vega post` fails immediately with `broker_unreachable`;
  with no URL configured its hint says so (`VEGA_NATS_URL`, `~/.config/vega/nats.url`,
  `vega onboard`). `vega mux send` falls back to a direct paste on the local machine, so local coordination
  works without NATS. After a completed identity migration,
  vegad runs in local mode (vegad.md, Local mode; vega-lhc.118); otherwise startup stays
  in identity quiesce. In local mode, herdr metadata,
  attention and the upgrade proposal keep working. So the onboard view recommends `vega post relay install`
  on every host (`vegad:` line), with or without a broker.
- `vega bd init` reports missing tools as `bd_missing` / `dolt_missing`, an unset provisioning
  remote as `remote_unconfigured` (set `VEGA_BD_REMOTE` or `~/.config/vega/bd.remote`), and a failed
  remote probe as `remote_unreachable`. Creating a missing database also needs `VEGA_BD_SQL` or
  `~/.config/vega/bd.sql`; unset returns `sql_unconfigured`. In contrast, `vega bd` / `vega bd status`
  succeeds with no provisioning remote, explicitly labels it unconfigured, and leaves provisioned
  projects' recorded remotes unchanged. Plain bd can still use local databases.
- Without `gh`/`tea`: `vega ci watch` records the watch; vegad logs the missing tool.
- Without `chezmoi`: nothing is managed, every unit applies.
- Without `~/.claude`, `~/.codex`, `~/.pi/agent`, herdr or `~/.config/vega/skills-global`: the matching
  config units are `skipped`.

The clearer `broker_unreachable` and `remote_unreachable` hints came with vega-lhc.118 (folding in
vega-lhc.113).

### Where it lives

`vega_config::infra` holds the table and the checks against a `Probe` (PATH dirs, env vars, the `Env`
directories), so tests run on a temp HOME with fake binaries. `vega config infra` prints every component's
state; the `vega config` home view adds one aggregate line (`infra: required 6/6, optional 5/12 …`).
`vega onboard` reads the same rows.

## vega onboard

```
vega onboard                   # mode, changes since last seen, required/optional infra, what apply does
vega onboard apply             # apply the defaults (rules below), record the changelog as seen
vega onboard changes [--all]   # changelog entries since last seen (or all)
```

The home view never writes. Its mode comes from the state file
`$XDG_STATE_HOME/vega/onboard.json` (`{"seen": <block>, "version": "<vega --version>"}`):

| Mode | When | View |
|---|---|---|
| `first run` | no state file | infra, config plan, `help: vega onboard apply` |
| `upgrade` | state's `seen` < the newest changelog block | the changelog entries after `seen`, then the same checks |
| `current` | `seen` = newest block | the checks only; `changes: none since block N` |

Every gap carries its next command (AXI 9). A required component missing shows first.

### apply: the defaults, without overwriting the user's own files

The repo's `config/` holds the Slayer's best practice. `vega onboard apply` writes it, but never replaces a
file the user already has:

- Key-merge and library units (`codex.hooks`, `pi.prefix`, `skills`) apply regardless of existing
  entries, subject to tool availability: they only add vega's entries.
- `claude.hooks` installs/enables the user-scoped hook plugin through the Claude CLI, then migrates
  legacy vega commands out of settings after verification. Without the CLI it skips with an install
  hint. See [config.md](config.md#claude-hook-plugin-vega-lhc243).
- Whole-file units (`claude.md`, `codex.md`, `pi.md`, `claude.styles`, `agentsview`,
  `dcg.config`, `dcg.packs`) apply only where
  the target is missing; an existing target that differs is `kept`, with `vega config diff <unit>` and
  `vega config apply <unit>` as the next step.
- `claude.settings` (seeded keys) applies only when none of its keys is set in the live file.

Then it records the newest changelog block as seen. Failed units exit 1 but still record the version: the
changes were shown; the failures are in the output.

This keeps the rule of config.md (another user does not get the Slayer's `CLAUDE.md` replacing their own),
while a fresh HOME gets the full defaults. The estate keeps `vega deploy apply` and `vega config apply`.

## Changelog

`CHANGELOG.md` at the repo root, embedded into `vega-onboard` at build time, so the installed binary knows
the changes it was built with.

```
## 7 · 2026-10-15

- unit: `claude.md`, `codex.md`, … come from `vega config` …
- migration: run `vega config apply` once after the dotfiles drop vega's targets.
```

- **Blocks** are numbered, newest first, contiguous down to 1, with a date. There are no release tags (every
  crate is 0.1.0-dev), and a date alone cannot tell two blocks of one day apart; the number can. The state file
  records the highest number seen.
- **Entries** are one bullet each, `- <kind>: <text>`, user-facing, one or two lines. Kinds:
  `required` (new required infra), `optional` (new optional infra), `unit` (new or changed config unit),
  `migration` (a step the user must take), `feature` (a notable new command or behaviour).
- **Who adds entries**: the commit that lands a user-facing change adds its entry, in the same branch: a
  sentinel in its worktree, the main session for its own work. A new block when the newest block's date is
  not today, else append to the newest block — unless that block is already on main and a host may have
  recorded it as seen; then a new block. Two branches that both add block N conflict in git; the merging
  session renumbers. Published blocks are never edited.
- **What counts**: anything an occasional user must act on or would miss: new infra, new or changed units,
  migrations, new plugins and commands. Not: fixes, refactors, internal design notes.
- **Tests** (plugins/onboard): the file parses; blocks are contiguous and dated in non-increasing order;
  kinds are valid; every infra component appears in a `required` or `optional` entry and every config unit in a
  `unit` entry, so a new component or unit without an entry fails `mise run check`.

## README prompts

Two fenced prompts in README.md to paste into a fresh agent session. They use `mise run install-cli` (no
VS Code, npm or test run; the same script `vega deploy` runs), not `mise run install`.

- **Onboard**: check `mise`, `cargo` and `git` (ask before installing any of them), clone the private repo
  (`gh repo clone durandom/vega ~/src/durandom/vega`; read access needed), `mise run install-cli`, then
  `vega onboard`, go through the required gaps, ask the user before each optional one, `vega post relay
  install` (with or without a broker), `vega onboard apply`.
- **Upgrade**: `git -C ~/src/durandom/vega pull`, `mise run install-cli`, `vega onboard`, summarise the
  changes for the user, act on `migration` entries, `vega onboard apply`.

## Upgrade

vegad checks once a day whether the checkout this host was installed from is behind its upstream and tells
one vega session (an agent pane in the checkout, the UAC first); `vega upgrade` pulls, installs, upgrades
moshi-hook and shows the changes. `vega deploy` targets never check. See [upgrade.md](upgrade.md).

## Rejected

- A network reachability probe in the checks: slow when the host is absent, and duplicated by the plugins.
- Semver tags as the "version seen": no release process exists; a numbered changelog needs none.
- Applying every default over existing files: it would replace an outside user's `CLAUDE.md` with the Slayer's.
- A separate onboarding skill: the plugin's `--skill` guide covers it (P5).
