# vega

Agent tooling for a small estate of machines: the `vega` CLI, its plugins, shims and skills. vega runs coding agents
(Claude Code, Codex, Pi) in [herdr](https://github.com/herdrdev/herdr) panes, lets them message each other, and
keeps their tools, themes and config in sync across hosts.

## Concepts

New here? [docs/concepts.md](docs/concepts.md) explains hosts, panes, agent kinds, the UAC (one coordinator
per project) and its sentinels, project identity, mail, vegad, beads, the gate and the trust model.
[docs/README.md](docs/README.md) indexes the how-tos and the design docs.

## What is in it

| Plugin | Purpose |
| --- | --- |
| [`mux`](plugins/mux/) | Agent-to-agent messaging over herdr; spawn and adopt agents |
| [`post`](plugins/post/) | Agent mail over a NATS JetStream broker; `vegad`, the per-host daemon |
| [`uac`](plugins/uac/) | Coordinator mode: the role guide and the home view of the agents a session runs |
| [`board`](plugins/board/) | The UAC's open decisions and the repo's beads in a herdr pane |
| [`open`](plugins/open/) | Open a selected path in an editor, or jump to a herdr pane (herdr plugin) |
| [`review`](plugins/review/) | Live review in the owner's headed browser; notes arrive as mail |
| [`gate`](plugins/gate/) | LLM review before agent pushes; records a pass for each change |
| [`ci`](plugins/ci/) | GitHub CI watch; the verdict arrives by mail |
| [`bd`](plugins/bd/) | Beads sync: a project's bd database on a configured Dolt remote |
| [`deploy`](plugins/deploy/) | Build and install vega on the herdr machines |
| [`theme`](plugins/theme/) | OS dark/light mode to every TUI; renders herdr's config |
| [`secrets`](plugins/secrets/) | Secrets for tools and agents: inventory, env injection, rekey |
| [`janitor`](plugins/janitor/) | Map of every git checkout on a machine (read-only) |
| [`orchestration`](plugins/orchestration/) | Project waits, question evidence and supervised coordination |
| [`capacity`](plugins/capacity/) | Subscription capacity from token-burn: every window, the resets ledger, token-burn's health |
| [`lanista`](plugins/lanista/) | Install, configure and use lanista, the LLM model catalog |
| [`skills`](plugins/skills/) | Index and loader for the plugins' agent guides |
| [`config`](plugins/config/) | The agents', herdr's and dcg's config from one source; the infra model (`vega config infra`) |
| [`onboard`](plugins/onboard/) | First run and upgrades: required and optional infra, defaults, changes since the last look |

[Shims](shims/) sit in front of known CLIs in agent sessions only (`shims/activate.sh`). They keep the original stdout and exit
codes and only add commands and `stderr` hints, with denials documented in each shim's README.

| Shim | Purpose |
| --- | --- |
| [`bd`](shims/bd/) | Derives `BEADS_ACTOR` per worktree (`<host>/<worktree dir>`), then runs the real bd |
| [`git`](shims/git/) | Gates agent pushes in repos with `.vega/gate.toml`; other calls pass through |
| [`gh`](shims/gh/) | Passthrough; denies the watch loops, hints at `vega ci`, counts calls per pane |
| [`herdr`](shims/herdr/) | Passes every call through to the real herdr |
| [`tea`](shims/tea/) | Passthrough; hints at `vega ci`, counts calls per pane |

Every CLI follows AXI (agent-ergonomic output, TOON on stdout) with the deviations in
[docs/design/agent-tooling-principles.md](docs/design/agent-tooling-principles.md).

`vega mux spawn` forwards the caller's `SSH_AUTH_SOCK`. For Codex it also sets the
socket and the herdr pane identity (`HERDR_PANE_ID`, `HERDR_SOCKET_PATH`) in the tool shell
environment policy, including with project `inherit = "core"`.
`--env SSH_AUTH_SOCK=...` and explicit Codex config arguments override the default;
see [spawn help](plugins/mux/help/spawn.md) for policy filters and remote paths.

## Install

Requires [mise](https://mise.jdx.dev/installing-mise.html) and a Rust toolchain.

```sh
mise run install      # workstation: CLI, shims, skills, herdr plugins, VS Code extension
mise run install-cli  # portable CLI part only (also what `vega deploy` runs on remote hosts)
```

Then `vega` with no arguments shows the home view, and `vega skills` lists the agent guides.

## Onboarding with an agent

The repo is private: your GitHub account needs read access. Only `mise`, `cargo`, `git`, `herdr` and one coding
agent are required; the broker, tailnet and Dolt remote of the original setup are optional, and `vega onboard`
says what each one adds. Paste this into a fresh agent session (Claude Code, Codex or pi):

```text
Set up vega (agent tooling, private repo github.com/durandom/vega; my GitHub account has read access).
1. Check `mise --version`, `cargo --version` (rustup) and `git --version`. If one is missing, tell me and
   ask before installing it.
2. Clone it: `gh repo clone durandom/vega ~/src/durandom/vega` (or `git clone git@github.com:durandom/vega.git`
   into the same path).
3. In the checkout: `mise trust`, then `mise run install-cli`. Make sure ~/.local/bin is on my PATH.
4. Run `vega onboard`. Go through the required gaps with me first. For each optional gap, tell me what it
   enables and ask whether I want it. Then run `vega post relay install` (also without a NATS broker: vegad
   then runs in local mode and still proposes upgrades).
5. Run `vega onboard apply` and show me the results. A `kept` unit is my own file: leave it unless I ask.
6. Run `vega skills` and tell me in three lines what I can do with vega now.
```

### Upgrades

vega proposes upgrades itself. Once a day vegad checks whether your checkout is behind upstream; when it is,
the agent running in your vega checkout gets a note with the new changelog entries and asks you whether to
upgrade (no agent there: your herdr workspace shows `vega ↓N`). On your yes it runs `vega upgrade`: pull,
install, the changes since your last look, the migrations, and `vega onboard apply` on your OK. You can run
`vega upgrade` yourself any time.

The changelog lives in [CHANGELOG.md](CHANGELOG.md); [docs/design/onboarding.md](docs/design/onboarding.md)
has the rules.

## Develop

```sh
mise run check  # fmt, clippy (-D warnings), tests
```

Design notes live in [docs/design/](docs/design/); the repo layout is in
[docs/design/repo-layout.md](docs/design/repo-layout.md). Work is tracked with [beads](https://github.com/gastownhall/beads)
(`bd ready`).

## License

Apache License 2.0, see [LICENSE](LICENSE). `plugins/board` is derived from herdr-beads and stays under the MIT
License in [plugins/board/LICENSE](plugins/board/LICENSE).
