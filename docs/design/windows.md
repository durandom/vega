# Native Windows subset on io

Feasibility spike: vega-nic.28.3, epic vega-nic.28. Observed 2026-11-17 on
proteus, vega base `03292746`; hc source checkout `de97ea5` (installed hc 0.1.21).
This is a design and read-only investigation, not a Windows deployment.

## Recommendation

Build a native Windows subset: `vega.exe`, `vega-skills.exe`, a client subset of
`vega-post.exe`, a target preparation/record subset of `vega-mux.exe`, and a
small Windows-local pointer worker reusing post's delivery protocol. Keep model
selection and orchestration on proteus/ganymede. Use native hc over Windows SSH
and the distinct host/machine name **io-win**; retain **io** for WSL.

Include post status, send, inbox, log, done/replies, host key generation and v2
identity enrollment; local hc identity and parent records; minimal host/agent
registry publication; and guarded pointer delivery. Initially exclude full
vegad jobs, config apply, deploy, mods, shims, local TUI/app sockets, worktree
creation, CI/Beads/git sync, theme, secrets CLI, PagerDuty and self-upgrade.
`vega skills` can index/read available plugins; skill-library linking needs a
separate Windows policy and is outside the first subset.

This is feasible, but not a recompile: only four of 41 workspace packages passed
the unmodified MSVC check here. The ordered follow-ups estimate 52 engineering
hours plus operator availability; allow additional time for native acceptance
and dependency extraction. The first runtime validation can reject assumptions
before most implementation work begins.

## 1. Reaching Windows and starting harnesses

### Observed, without starting or configuring anything on io

| Probe from proteus | Result |
| --- | --- |
| `ssh -G io` | user `hild`, hostname `io.tailc66a3b.ts.net` |
| `ssh -o BatchMode=yes -o ConnectTimeout=10 io 'where hc'` | `C:\Users\hild\AppData\Local\Programs\herdr-core\hc.exe` |
| `hc.exe --version` through that SSH login | 0.1.21 |
| `where codex`, then `codex --version` | `C:\ProgramData\orrery\codex\0.166.0\bin\codex.exe`, 0.166.0 |
| `codex login status` | Logged in using ChatGPT |
| `where claude`, then `claude --version` | `C:\Users\hild\.local\bin\claude.exe`, 2.1.296 |
| `claude auth status` | `loggedIn: true`, `authMethod: claude.ai`; no credentials copied |
| `dir G:\` | drive available; `G:\rally-ng-work` exists |
| `where sh` | not found on SSH PATH |
| explicit `dir` probes | `C:\Program Files\Git\bin\sh.exe` and `bash.exe` exist |
| `where git`; `where node`; `where npm` | Git at `C:\Program Files\Git\cmd\git.exe`; node/npm not found |
| `tasklist /FI "IMAGENAME eq hc.exe"` and equivalent for `herdr-core.exe` | no matching processes at probe time |
| proteus `hc machine ls` | machine `io` targets `hild@io-wsl.tailc66a3b.ts.net`, connected, hc 0.1.21 |

The SSH alias and hc machine name refer to different operating systems. WSL's
lack of a G: mount does not prevent native Windows SSH from seeing G:. Windows
PowerShell 5.1 and herdr standalone 0.9.3 are user-supplied facts; the SSH command
shell responded with cmd-style errors to a Unix command. Do not assume the SSH
login shell is PowerShell.

No machine entry was added, no terminal was launched, and no hc connection was
opened on Windows: hc commands can auto-start the core, prohibited for this
spike. Consequently **PATH inside an hc terminal, actual harness startup with a
G: cwd, ConPTY behavior, and logout survival remain unverified**. SSH version and
login probes do not prove these. No inference is made about ganymede's SSH keys.
A combined PowerShell diagnostic was rejected by the local command safety hook
because it could not verify runtime expansion; simpler direct executable/cmd
read-only probes supplied the evidence without changing that policy.

### Connection choice and source evidence

Use direct Windows OpenSSH, already reachable from proteus. hc's
[machines guide](https://github.com/herdrdev/herdr-core/blob/de97ea5/docs/machines.md)
describes SSH and byte-preserving `--via`; its
[machines.rs](https://github.com/herdrdev/herdr-core/blob/de97ea5/crates/herdr-core/src/machines.rs)
retries a native Windows connection after a non-POSIX shell failure. The native
line invokes `cmd /d /c` and the installer path
`.\AppData\Local\Programs\herdr-core\herdr-core.exe connect` (that file was
also confirmed present). It does not need `sh` for the connection itself.

The documented `--via 'wsl.exe -d Ubuntu -e sh -c'` goes **Windows to WSL**.
It does not implement the reverse WSL-to-Windows transport. The latter would
need working interop and a wrapper carrying the appended remote command and
protocol bytes unchanged, through another shell/quoting boundary. It is not
needed here and was not tested. Avoid piping the binary protocol through
PowerShell text processing. On Windows, hc documents PowerShell 7 for quoted
`--via` commands because 5.1/cmd can mangle embedded quotes.

The hc [README](https://github.com/herdrdev/herdr-core/blob/de97ea5/README.md)
supports Windows x86_64, defaults interactive shells to pwsh or Windows
PowerShell, and requires `sh` for templates/quoted shell commands. Prefer direct
program argv for initial harness launches. hc's
[security guide](https://github.com/herdrdev/herdr-core/blob/de97ea5/docs/security.md)
says Windows uses named pipes restricted to the current account and SYSTEM;
state inherits directory ACLs, and CLI calls can start a core. SSH account and
core must both be the intended non-elevated `hild` account.

### User-run validation commands (not executed by this spike)

In a new **non-administrator PowerShell** on io, start only an owned validation
terminal. Adding Git's bin directory below changes this process only; it enables
shell templates if needed. Direct argv avoids that dependency for these probes.

```powershell
$env:Path = "C:\Program Files\Git\bin;" + $env:Path
$hc = "$env:LOCALAPPDATA\Programs\herdr-core\hc.exe"
& $hc run --bg --name windows-spike --cwd 'G:\rally-ng-work' powershell.exe -NoProfile
```

In that new hc terminal, run:

```powershell
Get-Location
Get-Command hc,codex,claude,sh -ErrorAction SilentlyContinue | Select-Object Name,Source
codex --version
codex login status
claude --version
claude auth status
```

Run `codex` and `claude` separately from this terminal and have each report its
cwd and execute a harmless native Windows command. Exit each normally. Resolve
any trust/login/sandbox dialogs personally. A changed parent PATH does not
refresh an already-running core; inspect the actual terminal environment and
use explicit executable paths during validation. Do not restart a shared core
or touch another user's/agent's terminals to fix it.

After the user has started the Windows core, on proteus (and separately on
ganymede with an SSH target that reaches Windows):

```sh
ssh -o BatchMode=yes -o ConnectTimeout=10 hild@io.tailc66a3b.ts.net 'hc --version'
hc machine add hild@io.tailc66a3b.ts.net --name io-win --temp
hc exec --on io-win --cwd 'G:\rally-ng-work' cmd.exe /d /c cd
hc run --bg --on io-win --name windows-codex --cwd 'G:\rally-ng-work' 'C:\ProgramData\orrery\codex\0.166.0\bin\codex.exe'
hc run --bg --on io-win --name windows-claude --cwd 'G:\rally-ng-work' 'C:\Users\hild\.local\bin\claude.exe'
hc machine rm io-win
```

Do not use `--install` or `machine update`. Record each returned terminal ID;
inspect/attach/exit only those owned terminals, and check SSH disconnect and
reconnect before removing the temporary entry. `machine rm` forgets a local
connection and does not stop remote programs. These starts require the user;
they are not evidence of a completed test. Keep a permanent `io-win` connection
only when the operator elects to onboard the host.

## Harness installation, updates and authentication

Use the working orrery Codex 0.166.0 for the first baseline. Do not replace or
upgrade its `C:\ProgramData\orrery` files from a user installer: its provenance
and update ownership were not audited. For an independently updated user lane,
prefer the official standalone installer, then explicitly select the user
binary in spawn configuration; leave the managed copy intact. Node/npm are
absent from SSH PATH, so npm adds no benefit here.

The official [Codex CLI instructions](https://learn.chatgpt.com/docs/codex/cli.md)
provide this same command for install and update, from a new PowerShell:

```powershell
powershell -ExecutionPolicy ByPass -c "irm https://chatgpt.com/codex/install.ps1 | iex"
```

The [installer](https://chatgpt.com/codex/install.ps1), read but not executed,
defaults its visible binaries to `%LOCALAPPDATA%\Programs\OpenAI\Codex\bin`.
Run as the ordinary user. Confirm selection in a new terminal; use an explicit
path if the machine-level orrery PATH wins:

```powershell
Get-Command codex -All | Select-Object Source
& "$env:LOCALAPPDATA\Programs\OpenAI\Codex\bin\codex.exe" --version
# Only if login is needed after selecting the installation:
& "$env:LOCALAPPDATA\Programs\OpenAI\Codex\bin\codex.exe" login
# Alternative for a remote terminal without a usable browser callback:
codex login --device-auth
codex login status
```

`login --device-auth` is confirmed by io's 0.166.0 help. Existing login status
requires no re-login now. Binary installation without admin does not prove
sandbox setup will avoid elevation: the official
[Windows sandbox guide](https://learn.chatgpt.com/docs/windows/windows-sandbox)
distinguishes setup options. Leave permissions and any sandbox approval to the
user; do not disable sandboxing to make a launch pass.

Keep the existing user-local Claude installation. Its official
[setup guide](https://code.claude.com/docs/en/setup) recommends native installation,
which supports background updates; use `claude update` for an explicit upgrade.
Only reinstall if missing or broken. From non-administrator PowerShell:

```powershell
# Install/repair only when needed:
irm https://claude.ai/install.ps1 | iex
# Upgrade an existing native installation:
claude update
Get-Command claude -All | Select-Object Source
claude --version
# Only if authentication is needed:
claude auth login
claude auth status
```

`claude auth login` is confirmed by the installed help. The current login is
already valid according to the CLI; no model request was made to test quota or
service access. Git Bash is present and can support Bash tools; current Claude
documentation also describes a PowerShell fallback. Preserve each tool's normal
user credential store; never copy WSL credentials or tokens into the document.

## 2. MSVC compilation and Unix inventory

Installed the Rust standard-library target on proteus, with no Cargo config
change. Ran `nice -n 10 cargo check --workspace --target x86_64-pc-windows-msvc`
(exit 101), then the same check with `-p <package>` for every workspace package.
The table below records all 41 results. This was ordinary `cargo check`, not
`--all-targets`; integration tests have additional Unix-only fixtures.

A check does not link the final executable, but dependency build scripts still
compile native code. `ring` failed on missing `assert.h`, bundled SQLite on
missing `stdlib.h`, using Apple's clang for MSVC without the Windows SDK/C runtime.
These are cross-toolchain failures, not proof those dependencies cannot support
Windows. See Cargo's [native build-script example](https://doc.rust-lang.org/cargo/reference/build-script-examples.html).
Use a Windows MSVC CI runner for definitive checks/builds; do not change the main
build's Cargo configuration to suppress these dependencies or fake success.

| Package | Exit | First observed blocker(s) |
| --- | --- | --- |
| `vega-secrets` | 101 | Unix imports |
| `vega-secrets-cli` | 101 | Unix imports |
| `vega-build` | 0 | Passed |
| `vega-plugin` | 101 | Unix imports |
| `vega-service` | 101 | SQLite C build |
| `vega-herdr` | 101 | SQLite C build |
| `vega-notify` | 101 | ring C build, SQLite C build |
| `vega-registry` | 101 | ring C build, SQLite C build |
| `vega-uber-msg` | 0 | Passed |
| `vega-orchestration` | 101 | Unix imports, ring C build, SQLite C build |
| `vega-ci-watch` | 101 | SQLite C build |
| `vega-shim` | 101 | Unix imports |
| `herdr-shim` | 101 | Unix imports |
| `gh-shim` | 101 | Unix imports |
| `tea-shim` | 101 | Unix imports |
| `vega-bd-sync` | 101 | SQLite C build |
| `vega-bd` | 101 | SQLite C build |
| `bd-shim` | 101 | Unix imports, SQLite C build |
| `vega-ci` | 101 | SQLite C build |
| `vega` | 101 | Unix imports |
| `vega-transcript` | 0 | Passed |
| `vega-skills` | 101 | Unix imports |
| `vega-mux` | 101 | ring C build, SQLite C build |
| `vega-lanista` | 101 | ring C build, SQLite C build |
| `vega-theme-sync` | 101 | SQLite C build |
| `vega-post` | 101 | Unix imports, ring C build, SQLite C build |
| `vega-gate` | 101 | Unix imports, SQLite C build |
| `vega-onboard` | 101 | Unix imports, ring C build, SQLite C build |
| `vega-config` | 101 | Unix imports, ring C build, SQLite C build |
| `vega-history` | 101 | ring C build, SQLite C build |
| `vega-deploy` | 101 | SQLite C build |
| `vega-uac` | 101 | Unix imports, ring C build, SQLite C build |
| `vega-capacity` | 101 | SQLite C build |
| `vega-janitor` | 101 | ring C build, SQLite C build |
| `vega-review` | 101 | SQLite C build |
| `vega-browser` | 101 | Unix imports, SQLite C build |
| `agent-browser-shim` | 101 | Unix imports, SQLite C build |
| `vega-schedule` | 0 | Passed |
| `vega-theme` | 101 | Unix imports, ring C build, SQLite C build |
| `vega-uber` | 101 | Unix imports, ring C build, SQLite C build |
| `git-shim` | 101 | Unix imports, SQLite C build |

### Source inventory and treatment

Reproduce with `rg -n 'std::os::unix|cfg\(unix\)' crates/ plugins/ shims/`.
There were no literal `cfg(unix)` hits in that scan; most imports are unconditional,
including imports inside tests. A runtime `cfg!(...)` early return does not stop
Windows type-checking Unix code. The scan is a lower bound: `libc`, `std::os::fd`,
`ps`, `/bin/sh`, launchd/systemd, shell scripts and filesystem assumptions also
need attention. Files below are relative to the repository root.

| Crate/plugin | Unix sites and required treatment |
| --- | --- |
| `crates/vega-plugin` | `src/lib.rs`: executable permission bits. Use platform-specific executable discovery, strip `.exe` from plugin names and append it for dispatch. Keep Unix mode checks. |
| `crates/vega-herdr` | `src/socket.rs`: UnixStream/UnixListener to herdr-app. Compile out the TUI/app transport in the Windows subset; use hc CLI JSON RPC, which owns named pipes. Do not replace a Unix socket path with a pipe string. |
| `crates/vega-herdr` | `src/lib.rs`: CommandExt process groups, negative-PID kill, `parent_id`, `ps` ancestry; `src/project.rs`: process groups, kills and `/bin/sh`. Introduce Windows process-tree termination (job objects or equivalent), Win32 process ancestry and argv-based Git calls. Preserve deadlines and reject spoofed HC_TERMINAL_ID; trusting an environment variable alone is insufficient. |
| `crates/vega-herdr` | `src/launch.rs`, `src/history.rs`: private modes, raw descriptors/flock. Use ACL-restricted files and Windows lock/replacement primitives; isolate history capture when omitted. Preserve exclusive creation and crash recovery. Tests in lib/project/routes need portable fixtures. |
| `crates/vega-secrets` | `src/lib.rs`: OpenOptionsExt and permission modes. Exclude secrets storage from the minimal dependency graph; if retained, implement Windows private ACL semantics, not no-op chmod. |
| `crates/vega-shim` | `src/lib.rs`: MetadataExt device/inode, executable modes and CommandExt exec. Exclude shims initially. A future port needs Windows file identity for recursion avoidance, executable lookup and spawn/wait exit propagation. |
| `crates/vega-theme-sync` | `src/files.rs`: permission-preserving replacement; `src/pi.rs` tests: inode checks, symlinks. Exclude theme jobs/dependency from the subset; a future port must preserve ACLs and link targets. |
| `plugins/post` | `src/auth.rs`: 0700/0600 key generation and inspection; `src/identity.rs`: O_NOFOLLOW, exclusive private temp files. Implement restrictive ACLs, reparse-point defenses and Windows atomic replacement before accepting host secrets or cutover state. |
| `plugins/post` | `src/service.rs`: MetadataExt, exec, fd inheritance, fcntl/flock, setsid and signals. Exclude Unix lifecycle and self-reexec; implement a single Windows worker instance, explicit user startup, Windows lock ownership and controlled shutdown. |
| `plugins/post` | `src/keychain.rs`: Unix socket file type; `src/pagerduty.rs`: modes. Compile out these modules, not merely skip at runtime. Unit/integration fixtures throughout `src/` and `tests/` also use Unix permissions, symlinks and listeners. Port relevant client/worker fixtures; retain Unix-only tests behind compile-time guards. |
| `plugins/gate` | `src/start.rs`: permissions and pre-exec; `tests/common/mod.rs`, `tests/warmup.rs`: Unix fixtures/status. Remove from the client dependency graph; Windows gate execution is outside this subset. |
| `plugins/browser` | `src/real.rs`: executable modes and symlinked shim directories; tests use shell fixtures. Exclude. A later port needs Windows executable lookup and non-admin directory/launcher strategy. |
| `plugins/config` | `src/lib.rs`, `src/evidence.rs`, `src/infra.rs`: permissions/private files; tests/profiles/infra: symlinks and executable fixtures. Exclude config apply/infra/evidence from the Windows subset; later use ACL-aware files and explicit Windows installation policy. |
| `plugins/review` | `src/main.rs`: DirBuilderExt/OpenOptionsExt and CommandExt. Exclude; later use ACL directories, portable child lifecycle and shell-free argv. |
| `plugins/secrets` | `src/main.rs`: exec; `src/adopt.rs`: modes. Exclude; later spawn/wait preserving exit status and private ACLs. |
| `plugins/uac` | `src/main.rs`: CommandExt; `src/land/tests.rs`: executable fixtures. Exclude native UAC/landing initially; keep coordinator on Unix. |
| `plugins/deploy` | `src/main.rs`: chmod installed binaries; `tests/checkout.rs`: executable fixtures. Exclude; Windows packaging is a separate user install path. |
| `shims/bd` | `src/main.rs`: exec; recursion/sync tests: Unix symlinks, modes, CommandExt. Exclude all shims initially. |
| Tests only in the scanned files | `crates/vega-bd-sync`, `crates/vega-registry`; `plugins/service`, `skills`, `bd`, `notify`, `lanista`, `mux`, `uber`, `janitor`, `onboard`, `orchestration`; `shims/tea`, `agent-browser`. Use Rust helper executables for retained Windows tests; replace permission/symlink assertions with Windows equivalents, and guard genuinely Unix-only tests. These entries do not assert their production dependencies are portable. |

The source scan exposes blockers hidden behind the first C build failure. The
umbrella and skills are small ports, but post is monolithic: its
[manifest](../../plugins/post/Cargo.toml) pulls gate, onboard, lanista, notify,
theme, Beads, history and other jobs even for `inbox`. Extract a common client and
minimal worker or gate whole modules/dependencies behind features; avoid a forked
mail protocol. `crates/vega-herdr` also needs a reduced hc/state/project surface.

## 3. NATS, paths and the Windows identity

NATS's network transport is not tied to Unix sockets. The current client uses
`async-nats 0.50`, Tokio and nkeys; this spike did not complete its native TLS
build, so Windows connectivity/authentication is an acceptance test, not a pass.
Preserve protocol, authenticated subjects, reply tokens, acknowledgement and
archive behavior in [post](../../plugins/post/README.md).

Today `plugins/post/src/main.rs` (`config_dir`, `seed_path`) and
`crates/vega-herdr/src/state.rs` consult XDG variables then `HOME`, not Windows
`USERPROFILE`/AppData defaults. PowerShell's `$HOME` is not proof that a child
process has an environment variable named HOME. Proposed defaults:

| Purpose | Windows default | Existing override retained |
| --- | --- | --- |
| Executables | `%LOCALAPPDATA%\Programs\vega\bin` | installer-selected destination |
| Config and nkey | `%APPDATA%\vega\nats.url`, `nats.nk`, `vegad.env` | `XDG_CONFIG_HOME`, `VEGA_NATS_URL`, `VEGA_NATS_NKEY` |
| Runtime state/records | `%LOCALAPPDATA%\vega` | `XDG_STATE_HOME` (vega subdirectory) |
| User home | `%USERPROFILE%` | nonempty `HOME` |

Centralize these rules instead of adding an environment workaround in each
command. Private nkey, lineage and identity files must have appropriate ACLs.
Unix mode 0600 is not a Windows privacy guarantee. Windows canonical paths can
have `\\?\` prefixes, drive letters, backslashes and case differences; keep
owner-local G: locations distinct from normalized project IDs. Test Git
worktrees and paths with spaces even though creating worktrees is deferred.

Enroll `io-win` with its **own** host nkey, inbox stream, reply prefix, state and
v2 registry projection. Set `VEGA_POST_HOST=io-win` for both client and worker;
match hc's connection name. Do not reuse `io`'s seed or publish WSL's locations
as Windows locations. Admin-side auth/provision and the late-host identity
procedure belong to the UAC/operator; see
[project-registry-cutover.md](project-registry-cutover.md#late-host-rejoin).
Never put an admin seed on io. The restricted inbox-only role would quarantine
cross-host replies without explicit pair grants; use the appropriate normal
host role for this bidirectional sentinel lane, or account for those grants
explicitly. Merely adding an hc machine creates no vega registry entry.

## 4. Pointer delivery: physical capability versus current ownership

**hc can send text to a Windows terminal from another host; today's vegad will
not automatically deliver io-win mail through proteus's hc connection.**
The hc remote API is OS-neutral and accepts qualified `machine/tN` addresses.
`crates/vega-herdr/src/hc.rs` maps sends to `terminal.send`; this is source-level
capability, not a live Windows send test. The blockers are ownership and state:

- `plugins/post/src/main.rs::serve` subscribes to `vega.inbox.{host}.>` and reads
  that host's stream. Proteus's host credential is not io-win's receiver.
- `plugins/post/src/terminals.rs::local_terminal` deliberately excludes qualified
  remote terminal addresses from local discovery, registry and delivery state.
- Ancestry, local project paths, parent records, locks and v2 identity belong to
  the Windows host. A Unix worker must not attempt to open `G:\...` locally.

A new delegated Unix relay could be built, but would need separate io-win
credentials/ACLs, state and ownership election, remote identity/project queries,
remote hc status/screen/send, reconnect reconciliation and protection against
two relays delivering the same inbox. Changing only the local-terminal filter
or setting `VEGA_POST_HOST=io-win` on an ordinary proteus daemon is incorrect.
This is an alternative architecture, not existing functionality.

Recommend the small **Windows-local worker**, with hc CLI transport, normal
host credentials and minimal registry publication. Reuse `delivery.rs` draft
and blocked-state rules, pointer coalescing, pending retries and inbox-read
acknowledgement; test both Codex and Claude screen/status shapes. Start it
explicitly under the user for the first release; do not bring in full vegad
lifecycle or jobs. If the epic forbids any resident Windows process, it must
choose and fund the delegated-relay alternative; manual `vega post inbox`
works only after the client port and is not automatic pointer delivery.

## 5. Installation without admin

Ship a Windows x86_64 release archive containing only the subset's `.exe` files
and guides. The future installer should verify the release, write to
`%LOCALAPPDATA%\Programs\vega\bin`, offer a user-only PATH entry, and create
private config/state directories. Use copies of the vega skill stub for the
chosen harness, not privileged symlinks. Detect in-use executables and perform
updates at a controlled stop; do not port Unix exec-over-self behavior verbatim.
No Windows build toolchain is required for an end user of prebuilt binaries.

The user can prepare the **future** archive destination and PATH with this
PowerShell recipe after the release exists (these commands were not run):

```powershell
$vegaBin = Join-Path $env:LOCALAPPDATA 'Programs\vega\bin'
New-Item -ItemType Directory -Force -Path $vegaBin | Out-Null
# Copy the verified release's .exe files here; no such release exists yet.
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (($userPath -split ';') -notcontains $vegaBin) {
    [Environment]::SetEnvironmentVariable('Path', "$vegaBin;$userPath", 'User')
}
$env:Path = "$vegaBin;" + $env:Path
$env:VEGA_POST_HOST = 'io-win'
# After private storage and keygen have been ported:
vega post keygen
```

Only the public key printed by keygen goes to the operator for provisioning.
The worker startup command, host enrollment and rollback runbook must be
implemented/tested in the follow-ups before use; `vega post relay start` today
has Unix-only lifecycle code and is not a working Windows installation recipe.
Existing herdr-app automatic startup on connected hc hosts must not invoke the
full daemon on io-win; capability detection must select the minimal worker or
report it unavailable. Do not use `mise run install`, config apply or deploy
on Windows as a shortcut.

## Ordered follow-ups and risks

All follow-ups are children of vega-nic.28, have `discovered-from:vega-nic.28.3`,
and each blocks on its predecessor. Estimates are engineering minutes, not
elapsed delivery promises; each bead includes concrete acceptance and
`mise run check`. Type task/feature lets normal tier selection apply.

| Order | Bead | Work | Estimate |
| --- | --- | --- | --- |
| 1 | vega-nic.28.4 | Validate native Windows hc and authenticated harnesses on io | 120 min |
| 2 | vega-nic.28.5 | Extract Windows platform foundation and executable discovery | 480 min |
| 3 | vega-nic.28.6 | Build the native Windows post client subset and host identity | 720 min |
| 4 | vega-nic.28.7 | Support Windows target preparation for remote hc mux spawns | 480 min |
| 5 | vega-nic.28.8 | Add a minimal native Windows pointer delivery worker | 960 min |
| 6 | vega-nic.28.9 | Package and accept the Windows subset without administrator access | 360 min |

The main risks are untested live ConPTY/status behavior, core environment
inheritance, Windows process ancestry and child cleanup, ACL/reparse-point
semantics, and post's broad dependency graph. Keep normal Unix regression tests
while adding native Windows tests; a cross-check cannot validate these runtime
contracts. Manual login/trust/sandbox steps and operator-side host provisioning
are prerequisites, not actions delegated to a sentinel. A Windows host sleeping
or disconnecting must leave mail pending and registry state stale, not falsely
report success. The existing WSL lane remains a distinct host throughout.

## Validation of this document change

All 41 per-package MSVC checks completed; four passed and 37 exited 101 as
recorded above. No build configuration or production source was changed.
The required native command `nice -n 10 env -u CODEX_THREAD_ID mise run check`
completed with **exit 0**: 1,519 Rust tests passed, six skipped, and the remaining
script/format/lint checks passed. Gate start is disabled by
the spike brief. No install, merge, push, deploy, remote configuration or
terminal interaction was performed. The spike bead remains open for the UAC.

## Platform foundation (vega-nic.28.5)

The first Windows-clean package set is `vega-plugin` and `vega-herdr`.
`vega-herdr::platform` owns host paths, process isolation, ancestry, locking and
replacement. XDG overrides remain supported. Windows defaults are `USERPROFILE`
for home (nonempty `HOME` overrides it), `APPDATA` for config, `LOCALAPPDATA` for state and
`LOCALAPPDATA/cache` for cache; each consumer appends its existing application
subdirectory. Plugin discovery recognizes PATHEXT (including `.exe`) and removes
the executable suffix from plugin names.

hc uses its existing newline-delimited JSON-RPC CLI transport (`hc connect`),
which supports native Windows named pipes internally. Vega does not connect to
those pipes itself. The herdr-app Unix socket client, SQLite daemon wait store
and cutover migration code are compiled only on Unix.
The remaining shared clients compile on both platforms.

Windows ancestry uses ToolHelp process snapshots and checks parent/child
creation times to reject reused parent PIDs. Owned bounded commands start
suspended, enter a kill-on-close Job Object, then resume their primary thread.
This prevents descendants escaping between spawn and job assignment. Assignment
failure kills the suspended child and fails the call. Cleanup closes only that
owned job; it never targets an hc server or an unrelated terminal.

Ask mutations use native exclusive byte-range locks on Windows, released when
the handle closes; Unix keeps its existing flock contract. State replacement
uses MoveFileExW with replace-existing and write-through, without a delete gap;
Unix keeps rename. Windows private-file mode is deliberately a documented
no-op: files inherit the current user's profile directory ACL, since Unix mode
bits/Windows readonly flags cannot express equivalent confidentiality. Owners
of custom XDG directories must provide their own restrictive directory ACLs.

The `windows` CI job runs native all-targets check/clippy and tests for both
packages. Cross-checking on macOS validates Rust types and lints but cannot
execute the MSVC test binaries or prove native file/process behavior.
