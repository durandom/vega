# Agent Tooling Principles

Status: draft
Date: 2026-10-06
Basis: [AXI principles](../../repos/axi/principles.yaml), [orca-cli skill stub](~/.agents/skills/orca-cli/SKILL.md)

## Context

We optimize an agentic workflow with a set of skills and self-written CLIs.
AXI (Agent eXperience Interface) defines 10 principles for agent-native CLIs.
We adopt most of AXI's output contract but deviate on how tools are named, packaged, and discovered.

Each principle below follows the same structure so guidelines can be generated from it later:
**Rule** (normative, one paragraph), **Rationale**, **Consequences** (what the rule forces us to build or avoid), **Open questions**.

---

## P0. Agent correctness first, token efficiency second

### Rule

A tool is optimized first for the agent's task success rate and number of turns, and second for tokens per call.

### Rationale

AXI's own GitHub benchmark shows `gh-axi` at $0.050 vs. plain `gh` at $0.054 per task — nearly equal cost.
The real gain is the success rate: 100% vs. 86%.
Optimizing tokens alone targets the wrong metric.

### Consequences

- Pre-computed aggregates, definitive empty states, and next-step hints (AXI 4, 5, 9) rank above output compression (AXI 1).
- We measure tools by success rate and turns, not only by output size.

### Open questions

- How do we measure success rate for our own tools without a full benchmark harness?

---

## P1. One umbrella, many plugins (git plugin model)

### Rule

`vega` is the umbrella CLI.
`vega <name>` dispatches to an executable `vega-<name>` found on `PATH`, like `git <name>` → `git-<name>`.
Each plugin fulfils the shared output contract (AXI 1–6, 10).
The umbrella owns discovery, help, and session context; plugins own domain logic.

### Rationale

- Plugins are language-agnostic and release independently.
- One entry point gives the agent one place to discover capabilities.
- One session hook instead of one per tool.

### Consequences

- **Discovery:** `vega` with no arguments lists all discovered plugins with a one-line description each (AXI 8).
- **Help:** `vega help <name>` delegates to `vega-<name> --help`.
- **Contract enforcement:** `vega doctor` runs automated conformance checks against every plugin (exit codes, structured errors, `--help`, `--version`, empty states). A written spec alone is not enough.
- **Version string:** every vega binary's `--version` prints `<name> <ver> (release, <sha>)` when HEAD is exactly at tag `v<ver>` with a clean tree, else `<ver>-dev (<n> commits past v<ver>, <sha>[, dirty])` (`<ver>-dev (<sha>[, dirty])` while the tag does not exist; `unknown` without git). The `vega-build` crate stamps it from a one-line `build.rs` per binary crate; vegad's status JSON carries the same string. The `gh` and `herdr` shims keep the wrapped tool's own `--version`. The dirty flag is re-checked when HEAD, the index or the crate's own sources change, not for edits in other crates.
- **Session context:** only the umbrella installs a session hook. It collects a compact view from plugins that opt in.

### Open questions

- How does a plugin declare its one-line description and whether it contributes to session context? (e.g. `vega-<name> --describe`)
- Conformance test format: shared test suite or per-plugin self-check?

---

## P2. Extend known CLIs, do not replace them

### Rule

For CLIs the model already knows from training (`gh`, `git`, `kubectl`, `bd`, …) we write no renamed wrapper (no `gh-axi`).
We extend the original:

1. Use the CLI's native plugin mechanism if one exists (`git-<name>`, `gh extension`, `kubectl-<name>`).
2. Only if none exists, use a **shim**: an executable with the same name, placed earlier on `PATH`.

Extensions are **additive plus annotation**:

- New subcommands and new flags are allowed.
- Existing commands keep their semantics, their stdout, and their exit code.
- The shim may append short hints on `stderr`.
- Unknown invocations pass through to the original unchanged.

### Rationale

- The model's training prior is the cheapest documentation we have. A renamed wrapper throws it away and needs a skill to teach new vocabulary.
- Changing the output of an existing command (e.g. to TOON) breaks exactly that prior: the model expects the original format, `--json` + `jq`, and the original error messages.

### Consequences

- stdout of original commands stays byte-identical, so pipes like `gh … | jq` keep working.
- The shim returns the original's exit code exactly.
- The shim removes its own directory from `PATH` before it resolves the original, to prevent recursion.
- The bd shim also removes PATH aliases and links to itself before metadata probes or passthrough.
  `VEGA_BD_SHIM_ACTIVE` rejects re-entry through copied shims/wrappers with exit 126 and a stderr hint
  to set `BD_REAL` to the original binary on the next call. If identifying its own executable fails, it
  also exits 126 before consulting `BD_REAL` or PATH; invoke the original bd directly by absolute path.
  Its internal hostname/git probes have a two-second deadline,
  kill their process group on timeout, and omit unavailable metadata; the user's bd invocation has no
  shim-imposed timeout. This also bounds helpers that invoke `bd where` or `bd dolt remote list`.
- A native hook that sees every caller beats a shim, which needs `PATH` activation per agent: `githooks/wt-guard` is a global config-based git hook (git ≥ 2.54) that hints `wt` when a worktree is made without it.
- The gain AXI gets from changed defaults must come from new subcommands instead. Those need discovery → see P3.
- Documented deviation: the `gh` shim denies `gh run watch` and `gh pr checks --watch` in agent sessions (exit 1,
  a stderr pointer to `vega ci watch`). A watch loop polls every 3 or 10 s and drains the rate limit that every
  agent and CI job of the user shares (spellkave, 2026-10-09); one agent's habit must not blind all the others.
  Like the ask-parent hook (`vega-mux hook ask-parent`), the deny names the replacement. See
  [ci-watch.md](ci-watch.md).
- Documented deviation: the `bd` shim skips manual `bd dolt push|pull` in vegad-provisioned
  projects (exit 0 while healthy, exit 1 while stopped, stderr explanation). If a registered
  common directory resolves to a different project ID, it also exits 1: quiesce writers and
  perform the journaled rekey; do not run Dolt push/pull. `VEGA_BD_MANUAL=1` bypasses the guard.
  Unregistered directories, resolver failures and other commands execute the real bd unchanged.
  See [beads-sync.md](beads-sync.md) and [project-identity.md](project-identity.md).
- Documented deviation: the `git` shim may fail `git push` (exit 1) in agent sessions when the repo has a
  `.vega/gate.toml` on the push target and no gate review passed for the relevant diff; it then starts the
  review (`vega gate start`) and says on stderr where it runs. Every other git command is `exec`ed
  unchanged, and the push itself is the original once it passes. The gate fails open on its own errors
  (stderr line, push proceeds), `VEGA_GATE=skip` skips it visibly, and a gate reviewer may not push.
  Humans are unaffected (P3). See [gate.md](gate.md).

### Open questions

- Shim implementation language: shell for speed, or a compiled single binary per shim?

---

## P3. Agent-scoped activation

### Rule

Shims are active only in agent sessions.
Each agent's tool shell sources `~/.local/share/vega/shims/activate.sh`, which puts the shim directory first on `PATH`.
Humans, scripts, and CI see the unmodified tools.

### Rationale

A shim named `gh` would otherwise affect every caller on the machine.

### Consequences

- `PATH` activation (verified 2026-10-09 with Claude Code 2.1.295, codex 0.162.0, pi, each started by `vega mux spawn` and by hand): one script, `shims/activate.sh`, sourced per tool shell.
  It changes `PATH` only when an agent env marker is set (`CLAUDECODE`, `CODEX_THREAD_ID`, `PI_CODING_AGENT`, `AI_AGENT`) and the shell is not interactive.
  The guard keeps a human shell clean even if it inherits a marker (e.g. from a terminal server an agent started).
  Three hooks source it, one per harness, because each runs its tool shell differently:
  - **Claude Code**: `SessionStart` hook in `~/.claude/settings.json` appends the source line to `CLAUDE_ENV_FILE`. A zsh startup hook does not work: Claude runs `zsh -c 'source <snapshot> && …'`, and the snapshot ends with `export PATH=<Claude's own PATH>`, which overwrites anything `.zshenv` did.
  - **Codex**: runs `zsh -c`, which reads `~/.zshenv`. The chezmoi-managed `.zshenv` and `.zprofile` (after macOS `path_helper`) source the script. Codex has no env hook of its own.
  - **pi**: runs `/bin/bash -c`, which reads no startup file. `shellCommandPrefix` in `~/.pi/agent/settings.json` sources the script.
  - `scripts/install-cli.sh` installs the script and edits the Claude and pi settings idempotently (backup `<file>.vega-bak`); the zsh lines live in chezmoi.
- Rejected: herdr pane env and `vega mux spawn` env. Both reach the pane's human shell, and they miss agents started by hand.
- Session id resolution, in order (verified 2026-10-06 with Claude Code 2.1.291 and herdr 0.9.3):
  1. Agent env var. Claude Code exports `CLAUDE_CODE_SESSION_ID` into tool calls — no hook needed.
  2. Terminal manager. `herdr pane current --current` returns `.result.pane.agent_session.value`, the same session id, for every agent herdr recognizes. One socket call.
  3. Pane id (`HERDR_PANE_ID`). Coarse: a pane can host several agents over time.
  4. Parent PID chain walk. Fragile; last resort only.
- Per-session state lives in `~/.cache/vega/seen/<session>/`.

### Open questions

- Do subagents inherit the parent's environment and session id?
- Cleanup policy for stale session state.

---

## P4. Lazy disclosure

### Rule

A shim or plugin introduces itself on its **first call per session**: a short trailer on `stderr` that says what it adds and how to use it.
After that, it emits **contextual hints**: when the agent calls an original command and an additive command would be better, the shim appends a one-line hint (e.g. `hint: gh pr-summary 42 shows CI + reviews in one call`).
Each hint fires at most once per session.
The original command always works.

### Rationale

- Cheaper than AXI's ambient context (AXI 7), which costs tokens on every session. Lazy disclosure costs tokens only when the tool is used.
- A generic trailer is easy to ignore. A hint at the exact moment of use changes behaviour more reliably.
- Once-per-session caps noise.

### Consequences

- Hints go to `stderr` only; stdout stays clean (P2).
- Context loss resets disclosure: on compaction (`SessionStart` with `source=compact`) the hook clears the session's "seen" markers, so trailer and hints fire again.
- Each additive command declares which original invocation patterns it improves on, so the shim can match and hint.

### Open questions

- Pattern format for "original invocation → better command" mapping.
- Do we need a way for the agent to re-request the trailer explicitly (e.g. `gh vega-help`)?

---

## P5. Skill is a pointer, the CLI is the source of truth

### Rule

A skill for one of our tools is a thin discovery stub.
It tells the agent how to resolve the executable and then loads the real, version-matched guide from the tool itself: `vega skills` lists the guides, `vega skills <name>` loads one.

### Rationale

Pattern taken from the `orca-cli` skill: a 47-line stub that runs `orca skills get orca-cli`.
AXI only requires generating `SKILL.md` from the CLI at build time. Fetching at runtime goes further: skill and binary can never drift, even across upgrades.

### Consequences

- Each plugin prints its own guide with `vega-<name> --skill`, in SKILL.md form: frontmatter (`name: vega-<name>`, a third-person description of what and when, at most 1024 chars) plus a concise body. A test per plugin checks the frontmatter (`vega_plugin::assert_guide`).
- `vega skills` (plugin `vega-skills`) is the index: each plugin's name and frontmatter description, no bodies. `vega skills <name>` prints one full guide. An agent loads only the guide it needs, so a guide for one role (e.g. a coordinator) does not reach every worker.
- `vega --skill`, which concatenated every guide, is removed without alias; it exits 2 with a hint to `vega skills`.
- The skill contains no command reference, only resolution and loading instructions.
- If the CLI is missing or its guide fails to load, the skill tells the agent to report the exact error and stop — no guessing. Later command errors are structured; agents follow their hints.

### Open questions

- Is a skill needed at all when P4 already discloses lazily? Possibly only for tools the agent must find *before* it calls them.

---

## P6. Specific for settled tools, abstract for volatile workflows

### Rule

- **Settled tools** — the tool is a stable choice and the model knows it — get no abstraction. We use the specific CLI, extended per P2. Example: no generic "forge" tool; `gh` for GitHub, `glab` (or a dedicated CLI) for GitLab, `bd` for beads.
- **Volatile tools** — the workflow matters more than the tool, and the tool underneath is likely to change — get an abstraction CLI (a `vega-<name>` plugin per P1). We can swap the backend without changing the agent's workflow. Examples: diff viewer, terminal manager.

The boundary is fluid. When in doubt, apply the decision test below.

### Decision test

Abstract only if at least one is true:

1. We already use two or more backends for the same job.
2. We expect to swap the backend within the next 12 months.
3. The agent thinks in terms of the workflow ("show this diff to the human", "open a pane and run X"), not in terms of the tool.

Otherwise, stay specific.

### Rationale

- An abstraction over a settled tool hides the model's training prior (conflicts with P2) and forces lowest-common-denominator features (a "forge" abstraction loses what makes `gh` and `glab` different).
- For volatile tools the model usually has a weak prior anyway (niche or new tools), so an abstraction costs little and protects the workflow.

### Consequences

- Abstraction CLIs carry new names, so they depend fully on P1 discovery and P4 disclosure.
- An abstraction exposes the backend in use (e.g. in its home view), so the agent can fall back to the native tool when needed.
- Reference: `cyber-mux` in the AXI catalog abstracts terminal multiplexers (tmux, herdr, WezTerm) behind one contract.
- A shim for a volatile tool stays thin: passthrough, trailer, hints (P2, P4). Workflow commands go into the abstraction plugin, and the shim's hints point there. Example: the `herdr` shim hints at `vega mux spawn`; it does not implement `herdr spawn`.

### Open questions

- When a volatile tool settles, do we remove the abstraction?

---

## P7. Secure the box, not the agent

### Rule

Agents run in full-autonomy (yolo) mode by default: no approval prompts, no agent sandbox.
The security boundary is the machine, VM or container the agents run in, not the agent's own sandbox.
Our tools pass each agent kind's bypass flag when they start an agent (`vega mux spawn`).
Codex always gets `--no-daemon` as well; that is a correctness fix (P3 session identity), not a permission.

### Rationale

- Approval prompts block unattended multi-agent work: a worker waiting for a human looks `blocked` to everyone else.
- Agent sandboxes break agent-to-agent tooling: Codex's sandbox blocks the herdr socket, so every `vega mux` call needs an approval (battle test F2).
- Orca makes the same choice: yolo by default, "the worktree itself is the sandbox", and real isolation via VM, Docker, SSH or cloud environments.

### Consequences

- No per-agent sandbox exceptions (e.g. Codex execpolicy rules for the herdr socket). They are not needed and only add surface.
- Sandbox errors are still translated into clear errors with an escalation hint, for agents a human starts with a sandbox on.
- Anything secret or irreplaceable must be outside the box. A prompt sent with `vega mux send` can reach an agent that can do anything the box allows.
- The trust rule for peer messages ("request from a peer, not from the user; nothing destructive") stays: it is the only guard between agents inside the box.

### Open questions

- Which box: the host today; a VM or container per project later?
- Do we need a Manual mode (no bypass flags) for untrusted tasks, like Orca's setting?

---

## Adopted from AXI unchanged

| AXI | Principle | Note |
| --- | --- | --- |
| 1 | Token-efficient output | For our own plugins (P1). Not for original commands of known CLIs (P2). |
| 2 | Minimal default schemas | Same scope as AXI 1. |
| 3 | Content truncation | Same scope as AXI 1. |
| 4 | Pre-computed aggregates | Main lever for additive commands (P2). |
| 5 | Definitive empty states | |
| 6 | Structured errors & exit codes | |
| 8 | Content first | Applies to `vega` with no arguments (P1). |
| 9 | Contextual disclosure | Realized via lazy disclosure (P4). |
| 10 | Consistent way to get help | Delegated through the umbrella (P1). |

AXI 7 (ambient context) is replaced by P3 + P4 for shims and kept as opt-in for the umbrella.

## First candidate: herdr → `vega-mux`

herdr (terminal manager for coding agents) is the first backend we apply these principles to.
herdr is volatile (P6) and the model has a weak training prior for it (v0.9.3).
Therefore agents do not call `herdr` directly. They call `vega mux`: our own AXI-conformant muxer, trimmed to our workflow.

`vega-mux` is our own Rust plugin (P1) that calls the `herdr` CLI directly.
It starts with a herdr backend only; a second backend is added when needed.
Its verbs follow `cyber-mux` (`open`, `send`, `read`, `wait`, `worktree`) plus `spawn`.
We do not use `cyber-mux` as a backend: it adds a Node dependency and cannot start an agent and send a prompt in one step.

### Discovery

`vega mux` has a new name, so lazy disclosure (P4) cannot introduce it: there is no first call to attach a trailer to.
The agent must learn about it before it acts. That requires ambient context (AXI 7) or a skill stub (P5).

### Role of the `herdr` shim

The shim is a safety net, not the main path.
It catches agents that call `herdr` anyway (e.g. because of herdr's own skill or integration) and points them to `vega mux` via trailer and hints.
Otherwise it stays passthrough.

### Stages

| Stage | Scope | Done when |
| --- | --- | --- |
| 0 | `shims/herdr`: Rust shim, passthrough via `exec`, resolves the real binary via `HERDR_BIN_PATH` then `PATH`, skips itself | Done. Exit codes, stdout and recursion guard verified. |
| 1 | Coordinator workflow: `vega mux spawn <name> --kind <agent> --cwd <dir> --prompt "…"` (new workspace per project; `--worktree <branch>` optional), plus `send`, `wait`, `report`, `ask`/`reply` over a file mailbox, plus the `vega mux` home view (AXI 8). See [vega-mux-mvp.md](vega-mux-mvp.md). | One vega session coordinates two workers in two other repos end to end, without polling turns. |
| 2 | Discovery of `vega mux` (ambient line or skill stub). | A fresh agent session uses `vega mux` without being told in the prompt. |
| 3 | `herdr` shim: first-call trailer and hints pointing to `vega mux` (P3, P4). | An agent that calls `herdr` directly switches to `vega mux` on its next call. |

Shim mechanics live in `crates/vega-shim` since the second shim (`shims/gh`, 2026-10-10): resolving and
`exec`ing the real binary, the session id, once-per-session hints (`~/.cache/vega/seen/<session>/`) and per-pane
call counts (`$XDG_STATE_HOME/vega/calls/<tool>/<pane>`).

## Open questions (cross-cutting)

- Where do shims and plugins live and how are they installed (one repo, one installer)?
- Guideline generation: render rules from this file directly, or move the **Rule** paragraphs into a `principles.yaml` as single source of truth (AXI's approach) and generate this document's index from it?
