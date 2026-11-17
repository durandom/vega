# Browser engines for vega-browser

Status: research recommendation, 2026-11-17; bead vega-nic.21.

**Keep agent-browser as vega-browser's engine.** Both projects can drive local Chromium without
an LLM, but agent-browser already supplies the command, snapshot and session contracts our
projects use. Browser-use is a useful Python automation library, not a drop-in replacement for
that contract. Do not combine engines in the production plugin without a concrete task that
needs browser-use's agent loop and a separately designed lifecycle adapter.

## Scope and evidence

This comparison covers local/self-hosted browsers only. Hosted browsers, paid APIs and cloud
agent benchmarks contribute nothing to the recommendation. No API keys were configured, no
LLM was run, and no cloud browser was started. The browser-use agent loop's task success and
local-model performance are **unmeasured**.

Read-only research clones and a temporary `uv` venv lived under
`/tmp/vega-browser-engines.L8OLQN`; none are committed. Versions matter:

| Evidence | Version / revision |
| --- | --- |
| agent-browser installed CLI, tested | 0.31.1; upstream tag `v0.31.1`, commit `ed2e10598c9064aecfaeb7cf21b540684db4be2c` |
| agent-browser upstream source, reviewed only | 0.43.0; `b06e2533ba9b3823b8a96c859d455cb55f4d7893` |
| browser-use source installed into temp venv, tested | package version 1.0.6; `197972b206d3a6ec9fb30afcf76149c07fb8ee61` (main, not the 1.0.6 release commit) |
| browser-use local CLI implementation | pinned dependency `browser-harness==0.1.13`, installed wheel source inspected |
| Browser for both probes | existing Chrome for Testing 151.0.7922.34; separate fresh headless profiles |

The upstream links below pin the reviewed sources or releases. `A` means agent-browser;
`B` means browser-use; `H` means its browser-harness dependency. Upstream 0.43 features are
not claims about installed 0.31.1. In particular, neither project should be described from
old Playwright-era architecture alone: installed agent-browser already has a Rust CDP daemon,
and browser-use 1.0 replaced its former agent/browser engine.

Local requirements come from [browser.md](browser.md),
[agent-tooling-principles.md](agent-tooling-principles.md),
[the plugin](../../plugins/browser/src/main.rs),
[the shim](../../shims/agent-browser/src/main.rs), and the complete survey in
`bd show vega-nic.18` (notes dated 2026-11-16/17).

## Comparison

| Criterion | agent-browser | browser-use (local only) | Fit for vega |
| --- | --- | --- | --- |
| Architecture | Command-at-a-time native Rust CLI and persistent Rust daemon, direct CDP; no LLM needed. [A baseline][a-baseline], [A current][a-readme] | Python library with a Pydantic AI agent/REPL; CLI delegates stdin Python to browser-harness's persistent CDP daemon, without requiring an LLM. [B CLI][b-cli], [B dependencies][b-deps] | Existing outer coding agent already plans actions; agent-browser needs no second agent loop. |
| Naming and isolation | `--session` / `AGENT_BROWSER_SESSION`; own daemon/browser for independent launches, `session list`, `close`. CDP-attached sessions still share browser state. [A sessions][a-sessions] | `BU_NAME` names a daemon; named local/CDP daemons select a dedicated tab, not an isolated cookie store in a shared browser. Python `Browser('chromium')` can launch an isolated browser. [H daemon][h-daemon], [B migration][b-migration] | Both can isolate, but their names and lifetime boundaries are not interchangeable. |
| CDP attachment | `--cdp <port-or-URL>` and `connect <port>` match 9223/9333 workflows. [A baseline][a-baseline], [A CDP][a-cdp] | `BU_CDP_URL` / `BU_CDP_WS` in CLI; `Browser(cdp_url=...)` in library. No equivalent `connect` verb. [H daemon][h-daemon], [B migration][b-migration] | Technically possible for both; explicit endpoint required to avoid changing browser/logins. |
| Headed/headless | Headless default, `--headed`, `--executable-path`; already used by review and spellkave. [A baseline][a-baseline] | Library accepts `headless` / `executable_path` for its launches; default depends on display. CLI normally discovers an existing local browser. [B migration][b-migration], [H guide][h-guide] | BU needs an explicit launch policy, not its ambient desktop discovery. |
| Profiles/auth | `--profile <path>`, state/restore and per-session settings; supports our separate launch/attach choices. [A baseline][a-baseline], [A sessions][a-sessions] | `Browser('chromium', user_data_dir=...)` persists its own profile; `Browser('chrome', profile=...)` copies a desktop profile. [B migration][b-migration] | Profile reuse is possible; copying a desktop profile is not the same as attaching brainz's shared profile. |
| Config, proxy, downloads | Native `--config`, `--proxy`, `--restore`, `--download-path`, `--json` used in surveyed scripts. [A baseline][a-baseline], [A config][a-config] | Different Python/environment configuration. 1.0 explicitly lacks local `proxy` support and rejects several former launch/storage/download options. Externally launched Chrome could supply a proxy. [B migration][b-migration] | A switch requires migration or compatibility implementation, especially parallax/spellkave. |
| Init scripts/page marking | `--init-script`; vega has verified limitations and headed-only marking rules. [A config][a-config], [local mark.rs](../../plugins/browser/src/mark.rs) | Raw CDP can register `Page.addScriptToEvaluateOnNewDocument`; no matching CLI flag in the reviewed CLI. Harness adds a horse title marker by default (`BH_TAB_MARKER=0` disables). [B CLI][b-cli], [H daemon][h-daemon] | BU requires target-scoped script installation and reattachment handling; default marker conflicts with vega policy. |
| Agent ergonomics/AXI | Compact refs, `snapshot -i`, help, JSON, familiar verbs; native stdout/exit codes can pass through the shim. [A snapshots][a-snapshots] | Arbitrary Python/CDP makes filtering and batching flexible, but code authoring, output schema and many errors belong to the caller. [H helpers][h-helpers], [H run][h-run] | Agent-browser fits P2 passthrough; BU would be a new P6 abstraction, not a renamed shim. |
| Snapshot/token cost | Built-in compact accessibility projection; 388 tokens full and 11 interactive in the probe below. | Raw AX JSON and caller filtering available; internal `AxHelpers.state()` also offers compact state for the ultrafast agent mode. [B AX][b-ax] | Compare projections, not raw JSON against compact text as an engine-efficiency claim. |
| Ownership, GC, panels | Plugin joins `session list`, owner records and process scan; close delegates to the real binary. [local owner.rs](../../plugins/browser/src/owner.rs), [plugin](../../plugins/browser/src/main.rs) | No vega pane/project ownership; daemon shutdown exists, but inventory/attribution/owned-browser cleanup need adapters. [H admin][h-admin] | Agent-browser already meets the deployed lifecycle contract. |
| Stability/release cadence | 0.43.0 Nov 12, 0.42.0 Nov 9, 0.41.2 Nov 3; fixes include session relaunch, CDP settings, snapshots and closed-tab timeouts. [A changelog][a-changelog] | 1.0.3 Nov 12, 1.0.4 Nov 13, 1.0.5 Nov 14, 1.0.6 Nov 16; 1.0 changes defaults and removes many 0.x APIs. [B releases][b-releases], [B migration][b-migration] | Both move quickly; pin and smoke-test. No evidence here establishes comparative long-run reliability. |
| Dependencies/deployment | Native Rust binary; npm distribution uses Node (0.31.1 declares Node >=24; current >=22), not a Node browser daemon. Chrome still required. [A baseline package][a-package-old], [A package][a-package] | Python >=3.11,<4; browser-harness, Pydantic AI, provider SDKs, MCP and supporting libraries are core dependencies even for non-LLM CLI use. [B dependencies][b-deps] | BU adds a managed Python environment/version matrix to the deployed Rust/agent-browser tooling. |
| License | Apache-2.0. [A license][a-license] | MIT for browser-use; dependency licensing remains a separate packaging concern. [B license][b-license] | Neither top-level license prevents local/self-hosted use; retain required notices when redistributing. |

## Per-criterion implications

### Architecture and CLI versus library

The useful distinction is control surface. Agent-browser exposes actions the outer agent can
issue and verify. Browser-use's non-LLM CLI does too, through Python helpers/raw CDP; its
library additionally owns an agent loop, tools and a persistent execution environment. This
is valuable for a Python application delegating a complete browser task, but does not remove
vega's need to attribute and close browsers. Testing the CLI does not validate that agent loop.

Browser-use 1.0's default `Browser()` is a cloud browser; local library callers must explicitly
choose `Browser('chromium')` or a CDP endpoint. The CLI's desktop auto-discovery is a separate
behavior, not that library default. A local-only adapter must fix both choices explicitly,
disable telemetry (`ANONYMIZED_TELEMETRY=false`, `BH_TELEMETRY=0`), and never fall through to
login/bootstrap paths. See [migration][b-migration] and [CLI][b-cli].

### Sessions, CDP and profiles

A name identifies a daemon, not necessarily a separate browser. In both tools, multiple
sessions attached to one CDP browser share cookies/storage. Browser-harness's named daemon
creates a dedicated tab; current agent-browser documents optional `--pin-tab` and remembered
target IDs, while unpinned `open` retains shared-active-tab behavior. This is not proof of
isolated browsing, and the newer tab-pinning contract was not exercised on installed 0.31.1.
See [CDP mode][a-cdp] and [harness attach logic][h-daemon].

Keep our explicit split: brainz attaches to 9223, parallax to its own 9333, and review/sentinels
launch their own profiles. Browser-use can attach to either port technically; neither shared
endpoint was touched in this research. For a switch, a vega-owned launcher or Python library
service must preserve headed/headless choice, executable, profile and port, and distinguish
"disconnect from an external browser" from "terminate our browser and delete our temp profile."
A CLI daemon reload alone is not a substitute for that full ownership contract.

### Survey compatibility and marking

The vega-nic.18 survey supplies concrete migration tests:

- **vega/rally-coach:** preserve `review-<pane>` and `wt-<journey>`, headed review overlays and
  independent headless Playwright lanes. A browser engine migration must not claim those lanes.
- **brainz:** preserve `vega browser ab` and the headed shared Chromium/logins. Raw CDP scripts
  in `scripts/einkauf/cdp.py` remain independent of either engine.
- **spellkave:** preserve explicit `--config`, `--cdp`, `--headed`, Chrome-for-Testing selection,
  and `AGENT_BROWSER_SESSION=tile-clip-<pid>`. CI has no vega shim; a host-only replacement
  cannot silently change scripts that also execute there. `no-scroll.sh` was the survey's
  unnamed-call exception, not evidence that all callers already complied.
- **parallax:** preserve `connect 9333`, per-site profiles and the `--restore`, `--proxy`,
  `--json`, `--headed`, `--download-path` contract used by `AgentBrowser` in
  `src/parallax/core/tracker/browser.py`. Its external launcher could retain proxy behavior,
  but that does not implement the other command/output translations.

Vega's mark is deliberately restricted to explicitly headed calls, has `VEGA_BROWSER_MARK=0`,
and is omitted for the bot-blocker launch fallback. The [existing design](browser.md) records
0.31.1 failures for `addinitscript` and first navigation through `tab new`; keep those limits
visible rather than assuming help text is implementation. Browser-use's raw CDP is sufficient
in principle, but a replacement must register scripts on the intended target before navigation,
handle new tabs/reattachment, defer DOM work, and preserve screenshot/title opt-outs. Turning
off its default horse marker is necessary but not sufficient.

### AXI, output and lifetime

P0 favors correctness and fewer turns over tokens. Browser-use's Python can batch a custom
extraction into one call; agent-browser's built-in refs avoid repeatedly inventing extraction
and click-target code. Neither upstream interface is vega's TOON contract. P2 requires the
existing shim to preserve native semantics/stdout/exit status; translating `agent-browser`
invocations into Python would violate that promise unless a fully specified compatibility
layer existed. A separately named optional backend would be clearer.

The current [real-binary resolver](../../plugins/browser/src/real.rs) bypasses the shim so
inventory/cleanup cannot steal a session's ownership. [Owner records](../../plugins/browser/src/owner.rs)
carry pane, host, project, cwd and timestamps; GC keeps live, remote, unknown and unrecorded
owners. A BU adapter needs backend-qualified IDs, inventory and process attribution (the
existing `<session>-profile` heuristic will not transfer), session-specific shutdown, and
fail-safe liveness behavior. The panels' JSON schema and colours must continue to work.
Do not treat an expired daemon or a named tab as authorization to kill a shared browser.

### Release stability, dependencies and license

The sources show active maintenance, not a controlled reliability comparison. Agent-browser's
recent session/relaunch fixes are reasons to review an upgrade separately, not to install
0.43 during this task. Browser-use's [migration guide][b-migration] documents real churn:
old BrowserSession methods, options and MCP tool names were removed or changed. An adapter
against old 0.x examples would be designed against the wrong engine. Its core dependency
pins improve reproducibility but expand the Python upgrade surface; preserve the resolved
version set for any deployment. Both top-level licenses are permissive; this evaluation does
not make claims about every transitive component.

## Hands-on: isolated, non-LLM checks

Both repos were shallow-cloned; agent-browser's baseline tag was fetched for comparison.
`nice -n 10 uv venv <scratch>/venv` and
`nice -n 10 uv pip install --python <scratch>/venv/bin/python <scratch>/browser-use`
installed the reviewed browser-use checkout, without a global install or source changes.
Python was 3.12.13. All browser probes ran under `nice -n 10`.

The probe supplied an environment containing only normal process essentials and explicit
scratch settings, excluding inherited provider keys and browser settings. It used the real
agent-browser binary to avoid altering real owner records, an explicit empty config file,
and a private `AGENT_BROWSER_SOCKET_DIR`. Browser-use used scratch `BH_HOME`, `BH_RUNTIME_DIR`,
`BH_TMP_DIR`, `BH_AGENT_WORKSPACE`, `XDG_CONFIG_HOME`, telemetry off, update checks off, and
`BH_TAB_MARKER=0`. No real config or profile was written.

Agent-browser commands, with those settings on every call:

```sh
agent-browser --config "$scratch/empty.json" --session nic21-ab \
  --executable-path "$chrome" open https://example.com
agent-browser --config "$scratch/empty.json" --session nic21-ab snapshot
agent-browser --config "$scratch/empty.json" --session nic21-ab snapshot -i
agent-browser --config "$scratch/empty.json" --session nic21-ab close
```

For browser-use, the probe launched its own Chrome process with `--headless=new`,
`--remote-debugging-port=0`, `--user-data-dir=<scratch>/bu-profile`, `--no-first-run` and
`--no-default-browser-check`. It read that profile's `DevToolsActivePort`, set
`BU_CDP_URL=http://127.0.0.1:<allocated-port>` and `BU_NAME=nic21-bu`, then piped:

```python
new_tab('https://example.com')
wait_for_load()
print(page_info())
```

Subsequent CLI invocations measured raw and filtered accessibility output:

```python
import json
print(json.dumps(cdp('Accessibility.getFullAXTree')['nodes'], separators=(',', ':')))
```

```python
for n in cdp('Accessibility.getFullAXTree')['nodes']:
    if not n.get('ignored'):
        print(n.get('backendDOMNodeId'), n.get('role', {}).get('value'),
              repr(n.get('name', {}).get('value', '')))
```

The filter is evaluation code, not a browser-use built-in snapshot command. The library also
contains an internal compact projection used by its ultrafast mode; a separate non-LLM CLI
call attempted to exercise it directly:

```python
from browser_use.ax import AxHelpers
AxHelpers(cdp).state()
```

This internal API is not a claim that ordinary `page_info()` prints a snapshot: that helper
returns URL/title/viewport information. Internal `AxHelpers` is also not a stable replacement
for agent-browser's public command surface.

Successful first-run stdout measurements:

| Output | UTF-8 bytes | cl100k_base tokens |
| --- | ---: | ---: |
| agent-browser `snapshot` | 1,488 | 388 |
| agent-browser `snapshot -i` | 29 | 11 |
| browser-use raw AX JSON | 19,369 | 6,564 |
| browser-use simple unignored-node filter | 3,285 | 820 |

The compact-library repeat failed with an `Accessibility.getFullAXTree` timeout (exit 1)
after the repeat navigation failure described below. Its output size is **unmeasured**.
This does not show that browser-use intrinsically needs more tokens: its compact projection
exists, and custom filters can be as selective as the task requires.

Each call's stdout was saved separately from stderr. Counts use UTF-8 bytes and
`tiktoken==0.14.0`, `cl100k_base`, including the trailing newline; they are reproducible token
counts for one tokenizer, not billing for a particular deployed model. The page served six
language paragraphs and a Learn more link. Full and interactive output retain different
information. The simple BU filter retains duplicate inline text; raw JSON escapes non-ASCII
and includes AX metadata. Browser viewports were not normalized, so line wrapping also affects
raw AX nodes. These measurements establish output shapes, not an engine-wide token ratio,
performance ranking or task-success claim. No screenshots/image tokens were compared.

The first cold BU open took 37.81 seconds versus 1.73 seconds for agent-browser's open;
subsequent reads took under a second. These single observations include startup and network
loading and were not a controlled latency benchmark. The first-run navigations, reads and closes
returned exit 0. A repeat intended to measure the compact library formatter first hit a
stale `DevToolsActivePort` file in the reused scratch profile (a probe-fixture error). After
changing the fixture to allocate a fresh profile each time, browser-use navigation failed
with `Page.navigate timed out after 20s waiting for the daemon`, exit 1 and a raw Python
traceback; the subsequent raw and filtered AX reads also timed out. These are observed
failures in the same repeat session, not an established cause or reliability rate. `BU_NAME=nic21-bu browser-use --reload` stopped only the isolated daemon;
the probe terminated and waited for its own Chrome process in `finally`. Agent-browser's
named `close` stopped its own launch. A process/socket inspection found no remaining probe
daemons or browsers. No global 9223, parallax 9333, other sessions or real terminal services
were used.

## Recommendation and migration cost

**Keep agent-browser.** It satisfies the surveyed local flows with the smallest change surface,
and already integrates with naming enforcement, owner records, GC and visible marks. The
hands-on result demonstrates that browser-use works locally without an LLM, but establishes
no missing capability that pays for replacing those contracts.

Costs below are engineering estimates, not measured schedules; they include code, regression
fixtures and project coordination, not only changing the executable name.

| Option | Cost and work | Decision |
| --- | --- | --- |
| Keep deployed agent-browser | No engine migration. Continue existing tests; any version upgrade needs isolated launch/attach/close, config/proxy/restore and headed-mark smoke checks. | Recommended. This document changes no runtime or pin. |
| Switch to browser-use | High: roughly 1–3 engineer-weeks for a constrained adapter and survey migrations, longer for broad flag compatibility. Build launch/lifecycle/inventory, Python packaging, output/ref/error contracts, config translation, marking, shared-browser disconnect tests; coordinate four consumer projects and review. Local proxy would stay in an external launcher. | No demonstrated benefit justifies this scope. |
| Combine | Medium: roughly 3–7 engineer-days for an explicitly opt-in Python backend with owned sessions and backend-qualified inventory, before task-specific agent-loop evaluation. Preserve agent-browser shim/consumers; never allow two engines to consume one daemon's events or drive one working tab concurrently. | Defer until a concrete Python/LLM delegation use case needs it. |

Reconsider only with a representative local task corpus (login reuse, dynamic forms, overlays,
new tabs and attachment/cleanup), identical browser conditions, task success/turn counts and
snapshot budgets. A future local-model evaluation must choose and provision that model
explicitly. No follow-up implementation bead is needed to deliver this research; no migration
is authorized by the recommendation.

## Validation

`nice -n 10 env -u CODEX_THREAD_ID mise run check`: exit 0 (1,504 Rust tests passed,
6 skipped; remaining scripted checks passed). Gate review was explicitly disabled for this
research task. No runtime code, installation, deployment or global configuration changed.

## Source index

[A baseline][a-baseline] is 0.31.1; other A source links pin the reviewed 0.43.0 checkout.
B source links pin the tested checkout. H links identify the exact 0.1.13 dependency tag;
the installed wheel's `browser_harness/{daemon,admin,helpers,run,paths}.py` and `SKILL.md`
were the local source inspected for the probe.

[a-baseline]: https://github.com/vercel-labs/agent-browser/blob/ed2e10598c9064aecfaeb7cf21b540684db4be2c/README.md
[a-package-old]: https://github.com/vercel-labs/agent-browser/blob/ed2e10598c9064aecfaeb7cf21b540684db4be2c/package.json
[a-readme]: https://github.com/vercel-labs/agent-browser/blob/b06e2533ba9b3823b8a96c859d455cb55f4d7893/README.md
[a-sessions]: https://github.com/vercel-labs/agent-browser/blob/b06e2533ba9b3823b8a96c859d455cb55f4d7893/docs/content/docs/sessions.mdx
[a-cdp]: https://github.com/vercel-labs/agent-browser/blob/b06e2533ba9b3823b8a96c859d455cb55f4d7893/docs/content/docs/cdp-mode.mdx
[a-config]: https://github.com/vercel-labs/agent-browser/blob/b06e2533ba9b3823b8a96c859d455cb55f4d7893/docs/content/docs/configuration.mdx
[a-snapshots]: https://github.com/vercel-labs/agent-browser/blob/b06e2533ba9b3823b8a96c859d455cb55f4d7893/docs/content/docs/snapshots.mdx
[a-changelog]: https://github.com/vercel-labs/agent-browser/blob/b06e2533ba9b3823b8a96c859d455cb55f4d7893/docs/content/docs/changelog.mdx
[a-package]: https://github.com/vercel-labs/agent-browser/blob/b06e2533ba9b3823b8a96c859d455cb55f4d7893/package.json
[a-license]: https://github.com/vercel-labs/agent-browser/blob/b06e2533ba9b3823b8a96c859d455cb55f4d7893/LICENSE
[b-cli]: https://github.com/browser-use/browser-use/blob/197972b206d3a6ec9fb30afcf76149c07fb8ee61/browser_use/cli.py
[b-deps]: https://github.com/browser-use/browser-use/blob/197972b206d3a6ec9fb30afcf76149c07fb8ee61/pyproject.toml
[b-migration]: https://github.com/browser-use/browser-use/blob/197972b206d3a6ec9fb30afcf76149c07fb8ee61/MIGRATING.md
[b-releases]: https://github.com/browser-use/browser-use/releases
[b-ax]: https://github.com/browser-use/browser-use/blob/197972b206d3a6ec9fb30afcf76149c07fb8ee61/browser_use/ax.py
[b-license]: https://github.com/browser-use/browser-use/blob/197972b206d3a6ec9fb30afcf76149c07fb8ee61/LICENSE
[h-daemon]: https://github.com/browser-use/browser-harness/blob/v0.1.13/src/browser_harness/daemon.py
[h-admin]: https://github.com/browser-use/browser-harness/blob/v0.1.13/src/browser_harness/admin.py
[h-helpers]: https://github.com/browser-use/browser-harness/blob/v0.1.13/src/browser_harness/helpers.py
[h-run]: https://github.com/browser-use/browser-harness/blob/v0.1.13/src/browser_harness/run.py
[h-guide]: https://github.com/browser-use/browser-harness/blob/v0.1.13/SKILL.md
