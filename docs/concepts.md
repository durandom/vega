# vega concepts

vega is agent tooling for a small estate of machines. It runs coding agents (Claude Code, Codex, pi, ...) in
[herdr](https://github.com/herdrdev/herdr) panes, lets them mail each other on one machine or across machines,
lets one coordinator agent per project run worker agents in git worktrees, tracks the work in beads, reviews
agent pushes, and keeps tools, config and secrets in sync across hosts. Everything is a `vega <plugin>` CLI;
`vega` lists the plugins, `vega skills` the agent guides.

The guides call the human owner **the Slayer**; this page says "the owner".

## How the pieces fit

```
  owner ── talks to ──► UAC (coordinator agent, one per project)
                          │  vega mux spawn --worktree / vega uac adopt
                          ▼
  host A ─ herdr ─ workspace ─ pane: sentinel (claude|codex|pi|agy) in a git worktree (a lane)
     │                                │ vega post send / done / ask        │ git push
     │                                ▼                                    ▼
     │                    vegad (one per host): mail pointers,       git shim ─► vega gate:
     │                    wake-ups, registry, bd/git/CI sync         reviewer agent, verdict by mail
     │                                │
     └──────────── NATS broker (optional): mail streams, registry KV ◄──── vegad on host B, C ...
                                      │
            Dolt remote (optional) ◄── vegad pushes/pulls each project's beads database

  project = normalized git fetch remote (e.g. github.com/example/app): what UAC, lanes,
            registry entries and beads sync are keyed by, on every host
```

## Host (machine) and herdr

A host is one machine running vega; its name is `hostname -s`, lowercased (override: `VEGA_POST_HOST`).
[herdr](https://github.com/herdrdev/herdr) is the terminal manager the agents run in; vega drives it through
its CLI and socket and never forks it. herdr also knows other machines (`herdr machine list`), which is how
`vega mux --machine <label>` acts on agents elsewhere. Each host runs its own [vegad](#vegad-the-relay-daemon).

- Show: `vega mux --all`, `vega deploy` (the vega commit each herdr machine runs), `vega onboard`.
- Design: [vegad.md](design/vegad.md) (herdr integration), [pane-control.md](design/pane-control.md).

## Workspace and pane

A herdr workspace holds tabs, a tab holds panes, and an agent runs in a pane. A pane id looks like
`w1:p2` (workspace `w1`, pane `p2`) and is the address of an agent: `vega mux send w1:p2 "..."`. An agent can
also be addressed by its name (the tab label `vega mux spawn <name>` gives it). A worktree agent gets its own
workspace.

- Show: `vega mux` (agents in this workspace), `vega mux --all`, `vega mux show <pane|name>`.
- Design: [vega-mux-mvp.md](design/vega-mux-mvp.md), [pane-control.md](design/pane-control.md).

## Agent kinds

The kind is the herdr agent kind of a pane: `claude`, `codex`, `pi`, `agy` (`herdr agent` lists them).
`vega mux spawn --kind <kind>` starts one with its default args (approval prompts off, see `vega mux spawn
--help`); `vega mux reopen` resumes a closed claude or codex session. Delivery, the draft guard and the done
report work the same for every kind. `vega capacity` shows subscription headroom per provider, to choose a kind.

- Show: `vega mux` (kind column), `vega capacity`.
- Design: [vegad.md](design/vegad.md) (Goal, draft guard), [vega-mux-battletest.md](design/vega-mux-battletest.md),
  [capacity.md](design/capacity.md).

## UAC and sentinels

The UAC is a coordinator agent: one session per [project](#project-and-project-identity) that runs the owner's
other agent sessions across hosts, so the owner talks to one agent. A session becomes it with `vega uac claim`
(refused while another live UAC holds the project) and loads the guide `vega skills uac`. Sentinels are the
worker agents it runs: spawned with `vega mux spawn --worktree <branch>`, or adopted with `vega uac adopt
<pane>`, each in its own git worktree, never in the main checkout. A lane is the record of such a worktree: the
beads its bd actor (`<host>/<worktree dir>`) creates after the lane started get the label `uac-lane`. A
sentinel is headed (the owner answers its dialogs) or headless (its questions go to the UAC); it reports once
with `vega post done`. The UAC lands the work (`vega uac land`); sentinels never merge, push or install.

- Show: `vega uac` (the project's agents on every host), `vega uac status`, `vega mux mode <pane>`.
- Design: [uac-role.md](design/uac-role.md), [uac-rules.md](design/uac-rules.md),
  [orchestration.md](design/orchestration.md).

## Project and project identity

A project is one normalized git fetch remote URL, independent of checkout path and host: clones at different
paths or on different hosts with the same remote are the same project. The remote is `vega.projectRemote`
from the repository's common git config if set, else `origin`, else the only remote; several remotes without
`origin` and without that setting are an error (`project_remote_ambiguous`). Normalization is offline: it drops
transport, user info, default port, a trailing `/` and `.git`, and lowercases the host (and the path on
github.com), so `git@github.com:Example/App.git` and `https://github.com/example/app` both become
`github.com/example/app`. A repo without a remote gets `local:<host>:<common dir>`, a local path or `file://`
remote `file:<host>:<target>`; these never join across hosts. A fork with `origin` pointing at the fork is a
different project from its parent; set `vega.projectRemote` to the parent's remote to share it.

UAC claims, lanes, registry entries, beads and git sync records, notes and badges and model pins are keyed by
the ID; git, bd and file access use the local checkout. Changing the selected remote URL (or adding the first
remote) changes the ID: recorded consumers then refuse with `project_identity_changed` until the records are
rekeyed. The one-time cutover from path keys to IDs is `vega post identity`; tooling for later rekeys is not
built yet (vega-lhc.14.7).

- Show: `vega uac` (agents of this project ID), `vega bd status` (project and location per clone).
- Design: [project-identity.md](design/project-identity.md), [project-registry-cutover.md](design/project-registry-cutover.md),
  [project-local-sync-cutover.md](design/project-local-sync-cutover.md),
  [project-local-consumers-cutover.md](design/project-local-consumers-cutover.md).

## vega post: mail, pointers and replies

Agents talk by mail through vegad. `vega post send <pane> "..."` (or `vega mux send`, which uses the same
path) stores the message; the recipient's vegad types one short pointer, `[vega-post] 1 new message for you. Run:
vega post inbox`, into the pane only when the agent is idle or done and the owner is not typing there (draft guard).
Pending mail is coalesced into that one pointer; mail from the agent's parent is also typed while it works.
`vega post inbox` reads and acks the mail. A worker reports once with `vega post done --to <parent>`;
`vega post ask` (and `vega uac ask`) sends one question to several agents and wakes the asker once with all
answers. `vega mux send --now` pastes directly, as does the fallback when no vegad runs.

- Show: `vega post` (broker, my pane, unread count), `vega post inbox`, `vega post log`, `vega post notes`.
- Design: [vegad.md](design/vegad.md) (Message model, Shape), [vega-mux-broker-spike.md](design/vega-mux-broker-spike.md).

## vegad: the relay daemon

vegad is `vega post relay start`: one daemon per host, started by a herdr startup hook (`vega post relay
install`, once per host); it upgrades itself in place and needs no OS service manager. It delivers mail and
pointers, watches agent status through herdr events and wakes a parent on a done report, a question, a
silent or exited child, publishes this host's agents to the registry (a KV bucket on the broker), and runs the
beads sync, git sync (fetch every 5 min, fast-forward of
checkouts without local changes or commits), the CI watch and the daily upgrade
check. Without a broker it runs in local mode: herdr-only jobs stay on, mail, registry, beads sync and CI watch
are off.

- Show: `vega post relay status`, `vega post`, `vega bd status` (git sync too).
- Design: [vegad.md](design/vegad.md), [git-sync.md](design/git-sync.md), [ci-watch.md](design/ci-watch.md).

## Beads (bd) and its sync

Work is tracked in [beads](https://github.com/gastownhall/beads) (`bd`): issues in a per-project Dolt database,
not in git. `vega bd init` provisions a project once per clone: a local bd server, cloned from the project's
database on the configured Dolt remote. vegad then pulls every 60 s and pushes within seconds of a write;
agents never run `bd dolt pull`/`push`. A true merge conflict stops that project's sync until `vega bd resume`.
The `bd` shim sets `BEADS_ACTOR` to `<host>/<worktree dir>`, which is how lanes find their beads.

- Show: `bd ready`, `vega bd status`, `vega board` (the owner's decisions and the beads in a herdr pane).
- Design: [beads-sync.md](design/beads-sync.md), [board-ui.md](design/board-ui.md).

## The gate: review before push

In a repo with `.vega/gate.toml` on the push target, the `git` shim fails an agent's `git push` until an LLM
reviewer passed the change. The push starts the review (`vega gate start`): a reviewer agent in a tab
`gate-<branch>` reads the diff and the rubric, may ask the author by mail, and gives `vega gate verdict
pass|fail`; the author gets the findings by mail and pushes again. Each review is a bead labelled `gate`. The
gate fails open on its own errors, `VEGA_GATE=skip` skips it visibly, humans are not gated (the shims are on
the agents' PATH only), and a reviewer may not push.

- Show: `vega gate` (config and pass state of this branch), `vega gate check`, `vega gate log`.
- Design: [gate.md](design/gate.md).

## Deploy and upgrade

Two ways a host gets a new vega. `vega upgrade` pulls the host's own vega checkout (fast-forward only),
installs it and shows the changelog since the last look; vegad checks once a day whether that checkout is
behind and mails the agent in it to propose the upgrade. `vega deploy apply <machine>` builds and installs a
pushed branch on other herdr machines over SSH; `vega upgrade` refuses hosts that get vega this way.
`vega onboard` lists required and optional infra, `vega onboard apply` writes the default agent config.

- Show: `vega deploy`, `vega onboard`, [CHANGELOG.md](../CHANGELOG.md).
- Design: [upgrade.md](design/upgrade.md), [onboarding.md](design/onboarding.md).

## Config and secrets

vega owns the agents', herdr's and dcg's config (instructions, output styles, settings, hooks, global skills):
`vega config apply` renders them from repo defaults or local overrides in `~/.config/vega/config/` and leaves
units chezmoi manages alone. Service endpoints (broker, Dolt remote, ...) are first-line files under
`~/.config/vega/` (`nats.url`, `bd.remote`, `bd.sql`, ...); vega has no built-in endpoints. Secrets are age
ciphertext per host under `~/.config/vega/secrets/`, decrypted on demand: `vega secrets run --for <tool> --
<cmd>` injects only that tool's keys into one command's env, and values are never printed. Only the admin host
sets or rekeys secrets.

- Show: `vega config`, `vega config diff`, `vega config infra`, `vega secrets`.
- Design: [config.md](design/config.md), [secrets.md](design/secrets.md).

## Trust model

Mail into an agent's inbox is an instruction to an agent that runs without approval prompts, so who may send is
decided per host. Each host has its own NATS key (`vega post keygen`) and writes only its own registry keys and
sender subjects. A host listed as inbox-only (for example a work-managed machine) receives mail but cannot send
it; with a quarantine grant its replies become a note for the owner, never typed into a pane. Broker rights
are not changed by vega itself: `vega post auth-config` prints the `nats.conf` authorization block, the
owner of the broker reviews and applies it, and new hosts' streams need the admin key (`vega post provision`). Agents never approve trust or permission prompts, also not each
other's (`vega mux answer` answers question dialogs only); a peer agent's mail is a request, not an order from
the owner, except the parent's mail as a worker's task channel.

- Show: `vega post auth-config --help`, `vega post notes` (quarantined replies).
- Design: [vegad.md](design/vegad.md) (Security, Quarantine), [agent-tooling-principles.md](design/agent-tooling-principles.md),
  [project-registry-cutover.md](design/project-registry-cutover.md) (exact inbox-only grants).
