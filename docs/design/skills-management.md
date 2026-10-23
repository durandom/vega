# Skills management: research and options

Date: 2026-10-10. Star counts, versions and release dates are snapshots from that day.

## Summary

You do not need a new package manager. You need a small, deterministic layer that does three things: keep one
git-tracked global library, read a per-repo manifest with a SHA-pinned lockfile, and add each selected skill to each
harness's directory as its own relative symlink, or as a copy where symlinks fail. Existing tools already do most of
this. **getsentry/dotagents** (`agents.toml` + `agents.lock`, symlinks into `.claude/skills`) and **rulesync**
(SHA-pinned `rulesync.lock`, generated copies) come closest, and **`gh skill`** is the only installer that pins
commit SHAs. The convention has settled on `.agents/skills` as the neutral location: Codex, pi and Antigravity read
it. **Claude Code does not**, so every design needs a per-skill link or copy into `.claude/skills`. The idea of
installing skills on demand from a queryable database has strong support for *per-project scoping*. Selection
accuracy falls beyond ~30 visible skills, and the harness listing budgets cut descriptions at about the same size.
The *on-demand install from a large catalog* part carries real supply-chain risk and runs into reload limits in the
harnesses. Jev works well as an advisory ranker, but it is hosted, closed, waitlisted and weeks old. It should not
sit in the core loop. **Recommendation (Options for vega):** build a thin `vega skills` wrapper (Option B), add local
keyword search (selection S2), and keep a Jev or SkillRanker hook as an optional, opt-in experiment.

## Harnesses: where skills are read from

All four harnesses implement the agentskills.io format: `SKILL.md` with required `name` (≤64 chars, matches the
directory) and `description` (≤1024 chars), plus progressive disclosure. The spec defines **no install paths, no
registry and no version field** ([spec](https://agentskills.io/specification)). The harnesses fill that gap
differently.

| Harness | Global | Project | Reads `.agents/skills`? | Symlinks | Per-project disable | Packages/plugins |
|---|---|---|---|---|---|---|
| Claude Code | `~/.claude/skills` (+ `synced/`) | `.claude/skills`, walks up to repo root; nested lazily | **No** (zero hits in [llms-full.txt](https://code.claude.com/docs/llms-full.txt)) | Per-skill links documented | `skillOverrides` in settings; `Skill(name)` deny rules | Marketplaces, `enabledPlugins` at user/project/local scope ([install](https://code.claude.com/docs/en/plugins/install)) |
| Codex CLI | `~/.agents/skills`, `/etc/codex/skills` | `.agents/skills`, CWD up to repo root | **Yes** (primary) | Followed | `[[skills.config]] enabled=false` in `~/.codex/config.toml` (path-keyed) | Plugins; also reads `.claude-plugin/marketplace.json` ([plugins](https://developers.openai.com/plugins/build/plugins)) |
| pi | `~/.pi/agent/skills`, `~/.agents/skills` | `.pi/skills`, `.agents/skills` (after trust) | **Yes** | Not documented for plain dirs | `-path` / `!pattern` in `.pi/settings.json` (inferred) | `pi install npm:/git:` with pins ([packages](https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/docs/packages.md)) |
| Antigravity / `agy` | `~/.gemini/config/skills`, CLI `~/.gemini/antigravity-cli/skills` | `.agents/skills` (legacy `.agent/skills`) | Workspace only | Not documented | Not documented | `agy plugin install` ([docs](https://antigravity.google/docs/skills)) |

Sources: [Claude Code skills](https://code.claude.com/docs/en/skills), [Codex skills](https://learn.chatgpt.com/docs/build-skills),
[pi skills](https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/docs/skills.md),
[Antigravity skills](https://antigravity.google/docs/skills).

The facts that shape a design:

- **Claude Code needs its own entry.** A per-skill symlink `~/.claude/skills/<name> -> …` works and is documented.
  Earlier versions regressed when the *whole* `~/.claude/skills` or `.claude` directory was a symlink
  ([#38051](https://claudeissues.com/issue/38051-user-level-skills-not-loaded-when-claude-skills-is-a-symlink-regression-since-v2),
  [#36659](https://claudeissues.com/issue/36659-skills-not-shown-in-autocomplete-when-claude-is-a-symlink)).
  Discovery goes only one level deep ([#20755](https://claudeissues.com/issue/20755-feature-request-recursive-skill-discovery-in-claude-skills)).
- **Worktrees.** The Claude Code project-skill walk stops at the worktree root. From v2.1.277, a worktree without
  `.claude/skills` falls back to the main checkout's skills ([skills docs](https://code.claude.com/docs/en/skills)).
  Relative links committed in the repo resolve correctly in every worktree. Absolute links leak the main checkout.
- **Antigravity global is the odd one out.** Global skills live under `~/.gemini/…`, not `~/.agents/skills`. There is
  also a reported bug where global skills are missing from the system prompt
  ([forum](https://discuss.ai.google.dev/t/bug-report-agent-is-unaware-of-global-skills-due-to-missing-path-in-system-prompt/127260)).
- **Duplicates.** pi keeps the first skill it finds and warns. Codex shows both. Claude Code ranks enterprise >
  personal > project. Linking the same skill globally *and* per project therefore gives inconsistent results across
  harnesses.
- **Reload.** Claude Code watches skill directories, but a new top-level skills directory needs `/reload-skills`.
  Codex and agy need a new session after an install. Installing a skill mid-task only works reliably in Claude Code,
  and only when `.claude/skills` already exists.

## Listing budgets and accuracy argue for per-project scoping

Every visible skill costs context on every turn. Claude Code spends **1% of the context window** on the listing
(tunable with `skillListingBudgetFraction`) and caps each entry at 1,536 chars. When the listing overflows, it drops
descriptions of the least-invoked skills first ([skills docs](https://code.claude.com/docs/en/skills)). Codex spends
**2% or 8,000 chars** and shortens or omits entries with a warning
([Codex skills](https://learn.chatgpt.com/docs/build-skills)). Selection accuracy has a similar limit. Flat selection
stays above 90% at ≤20 skills, degrades past ~30, and drops to half its maximum at **κ ≈ 84–92 skills**. Hierarchical
routing holds 72–85% where flat selection reaches 45–63% ([arXiv 2601.04748](https://arxiv.org/html/2601.04748v2)).
Those experiments used GPT-4o-class models, and current models probably do better. The conclusion still holds: past a
few dozen skills, a global library should be *queried*, not *loaded*. That supports the core of the "skill database"
idea.

Retrieval is not solved either. The best hybrid ranker over 690 skills reaches 73.5% top-5, and 62.8% when the user
describes a symptom ([arXiv 2608.06196](https://arxiv.org/html/2608.06196v1)). The best off-the-shelf retriever
scores 0.665 NDCG@10 ([arXiv 2605.05726](https://arxiv.org/html/2605.05726v1)). A small, curated library beats
clever search over a large one.

## Existing tools

### Installers and package managers

| Tool | Manages | Scope | Link vs copy | Pinning / lock | Harnesses | License | Maturity |
|---|---|---|---|---|---|---|---|
| [vercel-labs/skills](https://github.com/vercel-labs/skills) (`npx skills`, skills.sh) | Skills | Project default, `-g` global | Canonical `.agents/skills` + per-agent symlinks; `--copy` | Ref (branch/tag) + hash; `update` goes to latest commit, **no SHA pin**; `skills-lock.json` restore is experimental ([local-lock.ts](https://github.com/vercel-labs/skills/blob/main/src/local-lock.ts)) | ~75+ incl. claude-code, codex, pi, antigravity(-cli) | MIT | 33.6k★, v1.7.2 (2026-10-09), ~729 open issues; **telemetry on by default** |
| [`gh skill`](https://cli.github.com/manual/gh_skill_install) | Skills | `--scope project\|user`, `--dir` | Copy (local installs; inferred for remote) | **Tag or commit SHA** (`--pin`), tree SHA in frontmatter `metadata.github-*`; `update` skips pinned | 49 agents incl. claude-code, codex, pi, antigravity-cli | MIT (gh) | Preview, gh ≥ 2.90 ([changelog](https://github.blog/changelog/2026-04-16-manage-agent-skills-with-github-cli/)) |
| [getsentry/dotagents](https://github.com/getsentry/dotagents) | Skills, MCP, hooks, subagents, plugins | Global default (`~/.agents`), `--project` | `.agents/skills` symlinked into `.claude/skills` etc. | `agents.toml` + `agents.lock`, `@ref` pins | Claude, Codex, Cursor, Copilot CLI, Grok, VS Code, OpenCode, Pi (**no Antigravity listed**) | MIT | ~250★, no releases; `sync`, `doctor --fix` |
| [rulesync](https://github.com/dyoshikawa/rulesync) | Rules, MCP, commands, subagents, skills, hooks | Project, `--global` | Generates **copies** (symlinked inputs get resolved); fetched skills gitignored | `sources` → `rulesync.lock` **SHA**, `--frozen` for CI ([declarative sources](https://github.com/dyoshikawa/rulesync/blob/main/docs/guide/declarative-sources.md)) | ~70 targets | MIT | ~1.5k★, v29.0.0 (2026-10-08), very fast-moving |
| [ruler](https://github.com/intellectronica/ruler) | Rules, MCP; skills experimental | Project, `~/.config/ruler` | Copies, gitignored | None for skills | ~32 | MIT | ~2.9k★, "Beta Research Preview" |
| [microsoft/apm](https://github.com/microsoft/apm) | Instructions, skills, prompts, agents, hooks, MCP | Not stated | Not stated | `apm.yml` + `apm.lock.yaml` (resolved tree + hashes) | Copilot, Claude, Codex, Cursor, Gemini, … | MIT | ~4k★, v0.33.0 |
| [skillpm](https://github.com/sbroenne/skillpm) | Skills via npm | Project | Copy to `.agents/skills` + links | npm semver + `package-lock.json` | Several | MIT | 14★ |
| [skillshare](https://skillshare.runkids.cc/docs/concepts/sync-modes) | Skills | Global source dir | Per-skill link (merge), dir link, copy | None found | Claims 60+ | MIT | Docs unverified; fast-changing |
| [openskills](https://github.com/numman-ali/openskills), [skillport](https://github.com/gotalab/skillport) | Skills + AGENTS.md/MCP shim | Project / global | Copy / served | Re-fetch only | Non-native harnesses | Apache-2.0 / MIT | openskills stale since 2026-01 |

The field has settled on three patterns. The first is a **git-source installer** with a canonical `.agents/skills`
plus links (`npx skills`, dotagents, `gh skill`). The second is a **generator** with a lockfile and gitignored copies
(rulesync, ruler). The third is a **shim** for harnesses without native skills support, which matters less now that
all four of your harnesses read SKILL.md. Provenance is stored either in frontmatter (`gh skill`, which changes the
file) or in an external lockfile (everyone else). The two do not interoperate.

### Harness-native distribution

Claude Code and Codex plugin marketplaces can distribute skills with pins (`ref`/`sha`) and project-level enablement.
Codex also reads `.claude-plugin/marketplace.json`, so one marketplace repo can serve both
([Codex plugins](https://developers.openai.com/plugins/build/plugins),
[CC marketplaces](https://code.claude.com/docs/en/plugin-marketplaces)). The limits: Claude Code project scope
*enables* a plugin but each user still has to *install* it, plugin skills are namespaced (`plugin:skill`), pi and agy
use different package formats, and auto-update is off for third-party marketplaces.

### Dotfile managers

Stow or chezmoi can deploy a global `~/.agents/skills` from a dotfiles repo, with per-skill links into the harness
directories ([nibzard](https://nibzard.com/unified-skills)). chezmoi's symlink mode **copies executables and
templates** ([target types](https://www.chezmoi.io/reference/target-types/)), so a skill with `scripts/*.sh` ends up
half linked and half copied. `.chezmoiexternal` archives can pin a tagged skills repo
([format](https://www.chezmoi.io/reference/special-files/chezmoiexternal-format/)). Dotfile managers do **nothing for
per-project selection**.

### Link vs copy

| Concern | Per-skill symlink | Copy |
|---|---|---|
| Single source of truth | Yes | No, needs re-sync (drift) |
| Teammates / CI | Only if target is in-repo and relative | Yes |
| Windows | Breaks by default (`core.symlinks=false`, [scivision](https://scivision.dev/git-windows-symlink)) | Works |
| Worktrees | Relative OK; absolute leaks main checkout | Independent per worktree |
| Agent edits | Write through to source (useful, risky) | Isolated |
| Ownership / cleanup | The link is the marker | Needs lockfile or marker file |

The rule the field converges on: use **links on personal machines** and **copies for anything committed and consumed
by others**. In both cases the source is pinned by a lockfile.

## Selection: Jev and other decision models

**Jev** is TypeSafe AI's hosted "System One" decision model. It takes state plus typed questions (`Choice`, `Score`,
yes/no `Noul`) and returns calibrated probabilities, not prose
([launch post](https://typesafe.ai/blog/introducing-system-one-models-and-jev)). One model is available
(`jev-1.13.0`). Pricing is $0.042 per million input tokens with free output. Limits are 64k tokens per request and
text-only input ([models](https://docs.typesafe.ai/models.md)). There are **no open weights and no self-hosting**.
Access is early-access with a waitlist, and signups were reportedly paused in late September 2026
([Copes](https://flaviocopes.com/jev-coding-agents/)). The launch date is unclear (Sept 15 or Oct 9, 2026). The speed
and cost claims ("193.6x faster") come from the vendor's own evals, which the authors say may be biased.

TypeSafe's **skill-suggestion cookbook** ranks 182 skills in two calls: a wide `Choice` to pick the top 12, yes/no
gates, a rerank of the top 3 on description plus 700 chars of the body, and abstention below 0.30. It returns at most
one hint. On 488 requests with a Haiku 4.5 agent, wrong skill loads fell from **16.8% to 7.3%** and needless loads
from 9.8% to 4.0% ([cookbook](https://docs.typesafe.ai/cookbooks/skill_suggestion.md)). The eval was run by the
vendor, and there is no embeddings baseline and no independent replication.
**[SkillRanker](https://github.com/Dicklesworthstone/skillranker)** (`sr`, Rust, ~132★, MIT plus an
"OpenAI/Anthropic Rider" whose terms were not read) packages this as a Claude Code prompt hook
(`sr install-hook claude`). The hook is advisory and runs in shadow mode by default, `--offline` makes no network
calls, and a BM25 prefilter applies above 254 skills.

Two caveats matter for your idea. **Neither Jev nor SkillRanker installs anything.** They pick among skills that are
*already present*. The gain also comes mostly from the **abstain gate and a rerank on fuller text**, which a local
BM25 or embedding ranker plus a small LLM can do too. Several MCP servers expose `search_skills` / `install_skill`
over catalogs of thousands of skills, for example
[skills-mcp](https://github.com/gengirish/skills-mcp) and [Qdrant-based skills-mcp](https://github.com/jignesh-ponamwar/skills-mcp).
`npx skills find` searches skills.sh. All of them depend on the agent knowing it should search, and on unvetted
catalogs.

## How vega does it today

| Piece | Mechanism |
|---|---|
| vega's own guides | One global stub `skills/vega/SKILL.md` (pointer). Real guides are compiled into each plugin as `const SKILL` and served by `vega skills <name>` (runs `vega-<name> --skill`, 5s timeout). `assert_guide` tests check the frontmatter. |
| Global install | `scripts/install-cli.sh:107-112`: `~/.agents/skills/vega -> <main checkout>/skills/vega`; `~/.claude/skills/vega -> ../../.agents/skills/vega`. |
| Project skills | `.agents/skills/{axi,beads}` committed as copies; `.claude/skills/axi` relative link. |
| Third-party global | `~/.agents/skills`: agentsview-finding-history, caveman, git-assisted-by, worktrunk. `~/.claude/skills`: mix of links, copies, a generated copy, `find-docs` (Claude only), `synced/`. |

`vega skills` already implements "query globally, load on demand" for vega's own tools: one stub in the listing, with
the guide served at runtime. That is the pattern to generalize.

Gaps:

1. **beads is invisible to Claude Code** in this repo because there is no `.claude/skills/beads` link.
2. **No Codex wiring** was found for the vega stub. Current Codex reads `~/.agents/skills`, so the stub probably
   already reaches Codex. This has not been verified on the installed Codex version. Antigravity global (`~/.gemini/…`)
   and pi are not wired up explicitly.
3. **Mixed global state** (links, copies, generated files). `git-assisted-by` has **already drifted** (Sep 1 vs
   Sep 19 copies), and nothing checks for drift.
4. **axi and beads are vendored without a source or pin**, so the only way to update them is by hand.
5. Guides are only as fresh as the installed binary. `open` and `board` are installed by `mise run install` but not
   by `install-cli.sh` or `vega deploy`, so they are missing on deployed machines.

## Options for vega

All options keep one global library as a git repo (for example your dotfiles, or a `skills` repo), cloned to the
same path on every machine by `vega deploy`.

### A. Adopt an existing manager as the backend

Use **dotagents** (closest fit: manifest, lock, global and project modes, symlinks into `.claude/skills`, pi and
Codex support) or **`gh skill`** (SHA pins). vega would only document the setup and add one `vega deploy` step.

- ✅ No code to maintain. A manifest and lockfile exist today.
- ❌ dotagents is ~250★ with no releases and lists no Antigravity target. `gh skill` is in preview, writes provenance
  into SKILL.md and copies instead of linking. `npx skills` has no SHA pin, telemetry on by default, and a reported
  bug where it skips the Claude symlink ([#851](https://github.com/vercel-labs/skills/issues/851)).
- ❌ Adds a Node or another runtime dependency to a Rust/mise toolchain, plus a second state model next to beads and
  vega.

### B. Thin `vega skills` wrapper (manifest + lock + materialize)

A small Rust plugin following AXI that does only the following:

- **Library:** `~/.agents/skills/<name>` as the canonical global store, plus an index built from frontmatter.
- **Per-repo manifest:** `.agents/skills.toml` lists `name` and `source` (`library`, or `owner/repo@ref:path`).
  `.agents/skills.lock` records the **commit SHA plus a content hash**, combining the strongest parts of `gh skill`
  and rulesync.
- **Materialize:** copy into `.agents/skills/<name>` (committed, works for CI, teammates and Windows), then add a
  **relative per-skill link** `.claude/skills/<name> -> ../../.agents/skills/<name>`. Codex, pi and agy read
  `.agents/skills` directly.
- **Global:** per-skill links from `~/.claude/skills/<name>`, `~/.gemini/config/skills/<name>` and
  `~/.gemini/antigravity-cli/skills/<name>` into `~/.agents/skills`. Leave `synced/` and tool-local entries alone.
- **`vega skills lib doctor`:** reports missing links, drift (hash mismatch), duplicates across global and project
  scope, and unpinned vendored skills. Run it from `mise run check` or a git hook.

- ✅ Deterministic and shell-script-sized. Fixes gaps 1–4 directly. No new runtime. Fits P5 and AXI.
- ✅ Copies in the repo plus links for Claude give single-source behavior inside the repo and reproducibility outside it.
- ❌ It is a ~10th implementation of the same idea. Fetching remote sources (git, auth, sparse checkout) is where the
  effort goes. Keep v1 limited to `library` plus git SHA sources, or shell out to `git`.
- ❌ Copies in the repo can drift from the library. `doctor` has to catch that.

### C. Serve, don't install (extend `vega skills`)

Generalize the existing stub pattern: one `skills` stub in every harness, with `vega skills find <query>` /
`vega skills <name>` printing any library skill's body on demand, as `npx skills use` does.

- ✅ Zero per-repo state. The listing stays tiny however large the library grows, which addresses the budget and
  accuracy limits. Same mechanism vega already uses.
- ❌ Skills drop out of harness-native discovery: the description is not in the listing, so the agent has to think to
  ask. No slash command. Relative `scripts/` and `references/` paths have to resolve to the library path. Weak for
  skills that should trigger automatically (beads, axi).

### D. Plugin marketplace

Publish the library as a Claude Code marketplace, which Codex also reads, and enable plugins per project in
`.claude/settings.json` / `.codex/config.toml`.

- ✅ Native pinning (`ref`/`sha`), native enable/disable per project, no linking.
- ❌ Users still install manually even when a plugin is enabled per project. Skills get namespaced (`plugin:skill`).
  pi and agy need separate packaging. This is heavy for a single-user library.

### Selection layer (orthogonal to A–D)

| Approach | How | Pros | Cons |
|---|---|---|---|
| **S1. Harness-native listing** | Keep each repo to ≤ ~20–30 skills via the manifest; use `skillOverrides: name-only` for rarely used ones | Free, no moving parts | Selection is manual; listing budget still truncates |
| **S2. Local keyword search** | `vega skills find <query>`: BM25 or keyword over frontmatter and the start of the body, offline; human or agent then runs `vega skills add` | Deterministic, private, fast; fits the "database" idea | Mediocre on symptom-style queries (~63% top-5 even for good rankers); agent must call it |
| **S2b. Local embeddings** | Same, with a local embedding model | Better semantic recall | Model download, non-determinism across versions |
| **S3. Jev / SkillRanker hook** | `sr install-hook claude`, advisory hint per prompt, abstain gate | Best evidence of a measurable gain (16.8% → 7.3% wrong loads, vendor eval) | Hosted, closed, waitlisted, network call per prompt, Claude Code only, ranks present skills, does not install |

### Recommendation

**Recommendation: B + S1 + S2. Treat S3 as an optional, time-boxed experiment. Reject automatic on-demand installs.**

1. Fix the gaps now with B's materialize and doctor parts: link beads for Claude, pin axi and beads in a lockfile,
   replace the drifted global copies with links. Little of this needs new design.
2. Before writing the fetch code, **spend a day testing dotagents (A)**. If it handles your global and project
   layout and can be extended to agy, use it as the backend behind `vega skills`, and build only what it lacks.
3. Keep installs **explicit and pinned**. `vega skills find` *suggests* skills, and a human or agent runs
   `vega skills add`, which writes the manifest and lock as a reviewable diff. This keeps the "global database"
   without letting a session pull unreviewed instructions into a repo.
4. Use C only for long-tail reference skills that never need to trigger automatically.

## Risks

- ⚠️ **Supply chain / prompt injection.** A skill is executable instructions, often with `scripts/`. Installing
  automatically from catalogs of 7–570k unvetted skills
  ([SkillsMP review](https://thegtmdirectory.com/tools/skillsmp)) gives any repo author a way to inject prompts into
  your agents. Mitigations: install only from an allowlist (your library plus named orgs), pin SHAs, show a diff on
  every add or update, never install from inside a running agent without your confirmation, and let `doctor` flag
  `scripts/` changes.
- ⚠️ **Vendor lock-in and availability (Jev).** A closed, weeks-old API with paused signups must not sit in the core
  path. Run it only advisory, in shadow mode, and fail open (no hint), as SkillRanker already does.
- ⚠️ **Claude Code does not read `.agents/skills`.** Any design built only on `.agents/skills` silently leaves out
  your main harness. This is already the case for beads. Make `doctor` enforce the Claude links.
- **Antigravity global path and symlink behavior are undocumented**, and global skills have a reported discovery bug.
  Verify on your machines before relying on them.
- **Duplicates across global and project scope** behave differently per harness (Claude precedence, pi first-wins,
  Codex shows both). Do not link the same skill at both levels.
- **Mid-session installs** need `/reload-skills` (Claude, new directory) or a restart (Codex, agy). "On demand"
  realistically means at session start or on the next session.
- **Fast-moving ground.** rulesync jumped to v29, Claude Code changed worktree skill loading in v2.1.277, and Codex
  docs moved hosts. Write down the versions any design depends on.

## Open questions

1. Where does the global library live: the dotfiles repo, or a dedicated `skills` repo with tags?
2. Windows: is it in scope? If not, use relative links everywhere and copy only for committed repo content.
3. Does installed Codex on your machines pick up `~/.agents/skills/vega`? Check with `/skills` in Codex.

## Addendum 2026-10-11: skills.sh vs agent-plugins.org

Decision input: skills are personal; vega should distribute them across machines and projects via an existing ecosystem.

| | skills.sh (`vercel-labs/skills`) | agent-plugins.org |
|---|---|---|
| What | CLI + public search API (`skills.sh/api/search`) | Packaging spec v1.0.0 (`plugin.json` + `skills/` + `mcp.json`); no CLI, no registry |
| Unit | one skill (SKILL.md folder) | plugin bundle (skills + MCP; hooks/commands only as client extensions) |
| Maintainer | Vercel; MIT; v1.7.2 (2026-10-09), ~33.7k stars | TSC: Amazon, Cursor, Microsoft, OpenAI, Vercel (not Anthropic); CC-BY-4.0 / Apache-2.0; tag v1.0.0 2026-08-06 |
| Installer | `skills add/update/remove/find`, `experimental_install` | none in spec; Vercel's npm `plugins` (v1.3.4, predates v1.0.0, source repo 404) installs via each client's plugin system |
| Scope | global (`-g`) or project | `plugins --scope user/project/local` |
| Link vs copy | single copy in `.agents/skills`, symlinked into agent dirs; `--copy` | unknown (delegates to client plugin systems) |
| Harnesses | ~79 incl. Claude Code, Codex, pi, Antigravity | listed: VS Code, Cursor, Copilot, Codex, Kiro, others; **not Claude Code, pi, Antigravity** |
| Lock / pin | `~/.agents/.skill-lock.json`, `skills-lock.json`: source + ref + content hash; no commit-SHA pin (update = latest) | optional semver `version` only; no lockfile; signing/provenance "future" |
| Private sources | any git repo / local path | any directory / git repo / `marketplace.json` |
| Search | yes (anonymous JSON API, `SKILLS_API_URL` override) | none |
| Telemetry | on by default (`DO_NOT_TRACK`) | `plugins` CLI: on by default |

Local state (2026-10-11): `skills` 1.5.24 via Homebrew; global lock tracks caveman, worktrunk, find-docs; `skills-lock.json` exists in ~10 repos (vega: axi) with `computedHash` but no ref, so restore is not reproducible.

Assessment: agent-plugins.org is a format, not a tool, and misses the harnesses in use here (Claude Code, pi, agy). skills.sh is the only working piece for install + lock + search. Its gap is pinning: vega would add the commit SHA (e.g. personal skills repo at a tag, or a vega-side lock) and a `doctor` drift check.

## Decision 2026-10-11

- Personal skills live in an own git repo: a curated collection of good skills.
- Install on demand, global or per project, with skills.sh as the engine.
- Skills that are not installed stay usable through vega (`vega skills <name>` serves them from the library, like the plugin guides today).
- Third-party skills are vendored (copied) into the library repo with skills.sh (`skills add <upstream> --copy`); its `skills-lock.json` records the upstream source and hash, so upstream changes show up as a reviewed diff. Projects and machines install only from the library repo, whose commit is the pin.
- The vega plugin (`skills/vega` stub, `vega skills`) stays in the vega repo.
- The library repo is not hard-wired: its remote is a vega setting; vega clones and manages it in a vega-owned directory. Which remote (GitHub, private, local) does not matter to vega.
- Paths: the library remote is a setting in `~/.config/vega`; the clone lives in `$XDG_DATA_HOME/vega/skills` (default `~/.local/share/vega/skills`). A git checkout is data, and dotfile managers often sync `~/.config` wholesale.

### Finding 2026-10-11: `skills use` covers "use without installing"

`skills` 1.7.2 has `use <source> --skill <name>` (also `owner/repo@skill`): it prints a prompt wrapping the full SKILL.md and copies the skill's supporting files to a temp dir, naming that dir so relative paths (`references/`, `scripts/`) resolve. Verified against a local library path (`npx skills use <dir> --skill axi`). `skills add <dir> --list` lists a library's skills with descriptions (human-formatted); `skills list --json` gives installed skills as JSON. So `vega skills <name>` can delegate to `skills use <clone> --skill <name>` for library skills; vega only needs its own listing (reading frontmatter is trivial).

### Decision 2026-10-11: one command, `vega skills`

The library hooks into the existing guide plugin; no second command (`vega lib`) to avoid confusion. One entry point for agents, one guide, global stub unchanged.

Revised the same day (vega-62l): the plugin is `vega skills` (binary `vega-skills`), since it manages many skills.
`vega skill` stayed as an alias in the umbrella until vega-svj (below) dropped it. `scripts/install-cli.sh` removes
a stale `vega-skill` binary.

Revised again (vega-svj): library management moved under `vega skills lib`; `vega skills` serves only plugin
guides. The table shows the current commands.

| Use case | Command | Effect |
|---|---|---|
| Agent: what do I know? | `vega skills`, `vega skills lib` | index of plugin guides; index of library skills (name, installed, description) |
| Agent: guide for X now | `vega skills X`, `vega skills lib show X` | plugin: `vega-X --skill`; library: header with the skill's dir in the clone, then SKILL.md |
| Project should always have X | `vega skills lib install X` | copy into the repo (`.agents/skills` + Claude link), committed |
| Machine should always have X | `vega skills lib install X -g` | global install |
| Drop X | `vega skills lib remove X [-g]` | uninstall |
| Add a third-party skill to the library | `vega skills lib add <owner> -s X` | `skills add <owner> --copy` in the clone, commit (no push) |
| Refresh vendored skills | `vega skills lib update [X...]` | re-vendor from the owner in the library's `skills-lock.json`, commit (no push) |
| New machine | `vega skills lib sync` (also from `vega deploy`) | clone/pull library, restore global skills |
| Drift | `vega skills lib doctor` | diverged copies, missing Claude links (e.g. beads today) |
| Clean up a project | `vega skills lib list [-g]` | every visible skill: origin, agent dirs, status, suggested action |

Rules (until vega-svj): `install`, `remove`, `sync`, `doctor`, `list` were reserved and could not be skill names; on a name clash a vega plugin won over a library skill. With `vega skills lib` there is no shared namespace, so neither rule applies any more. Skill selection via Jev or search stays out of scope for now.

### Implementation 2026-10-11 (vega-8su.1)

- Settings in `$XDG_CONFIG_HOME/vega` (default `~/.config/vega`): `skills-remote` (library git URL, first line) and
  `skills-global` (one skill per line; `install -g` adds, `remove -g` drops, `sync` restores). `~/.config/vega` is
  not dotfile-managed (it holds the NATS keys), so a new machine needs both files copied over; `vega skills lib sync`
  says so when the remote is missing.
- Clone: `$XDG_DATA_HOME/vega/skills`. `install` clones on first use but never pulls; only `sync` pulls
  (fast-forward only, re-pointing origin when the setting changed). The clone's commit is the pin.
- Engine: `VEGA_SKILLS_ENGINE`, else `skills` on PATH, else `npx -y skills`; always `DO_NOT_TRACK=1`. Installs pass
  `--agent claude-code codex -y`: without `--agent`, `-y` installs into every detected agent (e.g. an `agent/skills`
  copy for Eve). The global lock (`~/.agents/.skill-lock.json`) does not record local sources, hence `skills-global`.
- Library layout: skills under `skills/`, `.agents/skills/` (where `skills add --copy` vendors) and top-level
  directories, first one wins on a duplicate name.
- `vega skills` index: plugins, then servable library skills with `installed` (`project`, `global`, `both`, `no`);
  reserved names and plugin clashes are hidden there and reported by `doctor`.
- `doctor` (exit 1 on findings): `not_configured`, `not_cloned`, `origin_differs`, `uncommitted_changes`,
  `reserved_name`, `shadowed_by_plugin`, `missing_claude_link`, `broken_link`, `drift` (installed copy differs
  from the clone), `not_installed` (global set). Project scope is the git root of the working directory.
- `vega deploy apply` runs `vega skills lib sync` on the target; column `skills` is `ok`, `unset` or `failed`, never fatal.

### Implementation 2026-10-11 (vega-62l): `vega skills lib list`

- One row per skill name visible in the project (git root) or, with `-g`, under HOME, from `.agents/skills`,
  `.claude/skills` and the agent dirs doctor checks (`.pi/skills`, `.codex/skills`; global `.pi/agent/skills`,
  `.codex/skills`). Dot entries are skipped; an agent dir that is (a link to) `.agents/skills` is not counted.
- `origin` from the first dir holding it: `broken` (dangling link), `upstream` (lock entry from a source other than
  the library: `skills-lock.json`, global `~/.agents/.skill-lock.json`), `link`, `library` (name in the library),
  `local`. Global `~/.claude/skills/synced` is one row `claude.ai`.
- `status` reuses doctor's per-scope checks (`check_scope`); `action` is the first finding's fix, else `keep` for
  library skills, `vega skills lib install NAME` when the library has the name, else `candidate for library`
  (replaced by vega-1oz below).
- Pure read; exit 0.

### Implementation 2026-10-11 (vega-1oz): skills owned by a project repo

Rule (vega-skills README @ db83531): every skill has one owner repo. A project that owns a skill keeps the source
in `skills/<name>/` (visible; skills.sh skips hidden dirs), links `.agents/skills/<name>` -> `../../skills/<name>`
and `.claude/skills/<name>` -> `../../.agents/skills/<name>`; the library vendors it with
`skills add <owner/repo> --copy`. Projects install only from the library.

- `list` (project scope): origin `own` for an `.agents/skills/<name>` link resolving to the repo's own
  `skills/<name>/` (source `skills/<name>`). Action `keep` when the library has it, else
  `add to library: (cd <clone> && skills add <owner/repo> --copy -s <name>)`, owner from `git remote get-url origin`
  (GitHub URLs shortened to `owner/repo`).
- Other rows without a finding: `vega skills lib install NAME` when the library has the name, else `add to library`
  with the owner from the lock entry (`upstream`), the link target's repo remote (`link`), or `<owner/repo>`.
  `candidate for library` is gone.
- `install NAME` in the owner repo: `owned_here` error (exit 1); skills.sh would replace the live source.
- `doctor`: an `own` link is no `drift`. `library_stale` when the library copy differs from `skills/<name>`; the
  fix is the `skills add ... --copy` command, not `skills update`: skills.sh 1.5.24 updates a project skill by
  re-running `skills add <source> --skill <name> -y`, dropping `--copy` and the agent selection.
- `install` (project scope) rewrites the new `skills-lock.json` entries: skills.sh records the clone as
  `sourceType: local` with a path relative to the project (`../../../../.local/share/vega/skills/...`), valid only
  on one machine at one checkout depth. vega sets `source` to the library remote (`owner/repo` and `sourceType:
  github` for GitHub, else `git` with `sourceUrl`) and `skillPath` to the skill's path in the library.
- `list`: a lock entry is from the library when its source is the remote (as written or as `owner/repo`) or a path
  that resolves, against the project, into the clone; such rows are `library`, action `keep`.

### Implementation 2026-10-11 (vega-svj): `vega skills lib`

- `vega skills` lists only the vega plugin guides; `vega skills NAME` prints one. An unknown NAME that is a library
  skill gets the hint `vega skills lib show NAME`.
- No backward compatibility: the old top-level `install|remove|list|sync|doctor` exit 2 with a usage error naming
  the `vega skills lib` form; the `vega skill` alias (`vega_plugin::ALIASES`) is gone. `vega deploy apply` runs
  `vega skills lib sync`.
- `vega skills lib`: the library index (`name,installed,description`), every library skill; reserved names and
  plugin clashes no longer exist, nor do doctor's `reserved_name` and `shadowed_by_plugin`.
- `lib show NAME`: prints a header (`Base directory for this skill: <dir>`, absolute with links resolved, a
  read-only note, `library: <remote> @ <commit>`), then SKILL.md unchanged. No `skills use` (vega-g26): its temp
  copy was new per call, named only after the body, and could vanish mid-workflow.
- `lib add OWNER -s NAME...` and `lib update [NAME...]` run `skills add OWNER --copy -s NAME --agent universal -y`
  in the clone (`universal` writes only `.agents/skills`), retry with `--full-depth` on failure, then commit
  (`Add NAME from OWNER`, `Update NAME from their owners`). They never push; the output names `git -C <clone> push`.
  Refused on a dirty clone; a failed run resets the clone. `update` takes the owner from the clone's
  `skills-lock.json`; skills the library owns (`skills/NAME`) have none (`not_vendored`).
- Fixes in `doctor`/`list` name the new commands: `library_stale` is `vega skills lib update NAME` when the library
  vendors the skill (push the owner first), else `vega skills lib add OWNER -s NAME`; the `add to library` action is
  `vega skills lib add OWNER -s NAME`.
