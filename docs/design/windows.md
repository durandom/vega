# Native Windows subset on io

Feasibility spike: vega-nic.28.3, epic vega-nic.28. Observed 2026-11-17 on
proteus, vega base `03292746`; hc source checkout `de97ea5` (installed hc 0.1.21).
Sections 1–4 record that read-only investigation. Section 5 records the later
native self-build validation on io (vega-nic.28.10).

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
| `vega-uac` | Native subset | hc adoption receiver and project agent view. Landing, pruning, mail grant management, the capacity alias and shim counters are Unix-only. Uses the same MSVC TLS build as `vega-post`. |
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
| Executables | `%LOCALAPPDATA%\Programs\vega` | installer-selected destination |
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

### Native mail client and package inventory (vega-nic.28.6)

The machine-readable build/install inventory is
[`scripts/windows-packages.json`](../../scripts/windows-packages.json): the file
is an array of Cargo `package` names and their shipped `binaries` (without
`.exe`). Windows CI reads this file; the native installer must read it too.
It ships `vega.exe`, `vega-post.exe`, `vega-skills.exe`, `vega-mux.exe` and
`vega-uac.exe`, plus their shared library packages. No daemon or shim belongs in that list.

Post's Unix entry point is `plugins/post/src/unix.rs`; the reduced native entry
point is `windows.rs`. Both include `client_common.rs` and the same retry publisher,
typed body renderer, routing, headers, reply-token and acknowledgement logic.
The Unix publisher retains history/outbox recording. Native mail publishes directly;
the Windows pointer/history worker is separate work (vega-nic.28.8).
`--features client-only` selects the reduced entry point on Unix for local fake-broker
regression tests; ordinary Unix builds retain daemon, relay and cutover behavior.

Native commands: bare post or `status`, `send` (including `--re` for replies,
`--uac`, `--host`, `--now`, `--kind`), `inbox` (`--keep`, `--show`, `--max`),
`log` (including `--json`), `done`, `keygen`, `identity enroll` and `identity verify`.
The native client rejects relay, provision, auth-config, admin keygen and legacy
cutover commands with usage errors. Inbox-only local delivery/quarantine is not
part of this client: provision a normal host role for io-win. hc identity remains
ancestry-verified; an `HC_TERMINAL_ID` environment value alone is insufficient.

### Operator runbook: fresh io-win enrollment

These are instructions for the UAC/operator **after landing**, not actions run by
the sentinel. Keep the io WSL lane and Windows lane independent throughout.

1. On Windows, install the binaries from the inventory and use the dedicated
   user profile. Set `VEGA_POST_HOST=io-win` in the client environment (and the
   future Windows worker's environment). The hc machine name must also be io-win.
   Leave `VEGA_NATS_NKEY` unset unless pointing to this Windows host's own seed.
   Never copy io's WSL seed/state or place an admin seed on io or io-win.
2. Before creating secrets, inspect `%APPDATA%\vega` and `%LOCALAPPDATA%\vega`
   with `icacls`. The policy is inherited private user-profile ACLs: the owner,
   SYSTEM and administrators may access them; unrelated users/groups must not.
   Custom XDG/seed directories require the operator to supply equally restrictive
   ACLs. This client does not pretend chmod/readonly bits establish Windows privacy.
   Seed and enrollment paths reject symlinks/reparse points in existing ancestors;
   file creation is exclusive, private Unix mode when applicable, and synced.
   Enrollment does not emit the seed. Concurrent hostile changes by the owning
   account are outside this inherited-profile policy.
3. Run `vega post keygen` on Windows. Retain the seed only in
   `%APPDATA%\vega\nats.nk`. Send **only** the printed `io-win=U...` public-key
   entry to the UAC/operator. Repeating keygen prints the same public key.
4. On the trusted Unix operator host, extend the existing complete `auth-config`
   inputs with that entry and the temporary `--create io-win` grant. Preserve all
   existing host entries, roles and grants. Deploy/reload broker authorization by
   the normal operator procedure, then run `vega post provision io-win` with the
   operator's admin credential on that trusted host. Remove the temporary create
   grant afterward. The normal host grants permit publishing only
   `vega.inbox.*.*.io-win`, fetching/acking only `VEGA_INBOX_io-win`, subscribing
   to `_INBOX_io-win.>` for request replies and writing only
   `$KV.VEGA_REGISTRY.io-win.>`; retain normal registry-info/direct-read grants.
   Do not reuse `VEGA_INBOX_io` or `_INBOX_io`, and do not grant host stream creation.
5. Write the broker endpoint as the first line of `%APPDATA%\vega\nats.url`
   (or set `VEGA_NATS_URL`). On Windows, from each local checkout, run:

   ```powershell
   $env:VEGA_POST_HOST = 'io-win'
   vega post identity enroll --checkout 'G:\src\project with spaces'
   vega post identity verify
   ```

   Repeat `--checkout` in one enrollment for multiple checkouts. Git derives v2
   `ProjectId` and owner-local `RepoLocation`; no remote filesystem is inspected.
   The immutable, synced `%LOCALAPPDATA%\vega\identity-native\enrollment.json`
   records schema 2, host, public key and resolved projects. The broker projection
   is `io-win.identity.v2`, created with compare-and-set under this host's write
   scope and read back for verification. It is CLI-managed and retained by
   registry reconciliation. No live beat/UAC claim is forged by enrollment.
   A network failure leaves the local record for an identical safe retry.
   Changed host/key/projects, imported cutover/UAC/lane state, live legacy owned
   registry keys or a divergent broker projection fail closed for operator review.
   This fresh-host flow does not create or bypass a Unix cutover completion marker.
   The future Windows worker must verify this enrollment before publication;
   existing Unix hosts continue to use the audited late-host cutover procedure.
6. In an ancestry-verified native hc agent terminal, run `vega post status`, send
   to a known proteus pane with `--host proteus`, read there with `inbox`, and
   reply using the printed timestamped `--re` command. Read/ack the reply on
   Windows and send `done` to the recorded or explicit parent. Verify `log` still
   contains acked mail. Manual inbox works now; automatic pointer delivery waits
   for vega-nic.28.8. The UAC performs this live acceptance after landing.

Rollback before a worker is enabled: stop using the native client, retain its
private seed/local enrollment for diagnosis and let the operator revoke only the
io-win public key/projection. Never delete or alter io's credentials/stream. A
changed key or project identity needs explicit operator reconciliation; deleting
an enrollment file is not a migration/rekey procedure.

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

## 5. Native source installation without admin (vega-nic.28.10)

Use **x86_64-pc-windows-msvc**, matching native Windows CI. io already has Visual
Studio Community 2022 17.14.13 with the x64/x86 C++ tools (MSVC 14.44.35207) and
Windows SDK 10.0.26100.0 at `C:\ADK` (the registered KitsRoot10). Rust and the
`cc` crate discover the registered installation; no Developer Command Prompt
or administrator access is needed. GNU would require maintaining a separate
MinGW toolchain and would differ from the CI ABI; io's existing GNU-local rally
toolchain is left alone. See the [Rust MSVC prerequisites](https://rust-lang.github.io/rustup/installation/windows-msvc.html)
and [ABI choice](https://rust-lang.github.io/rustup/installation/windows.html).

The C dependency inventory, using `cargo tree --target x86_64-pc-windows-msvc
-e normal,build` on the current checkout:

| Package | C compiler requirement |
| --- | --- |
| `vega-plugin`, `vega-herdr` | None in the Windows dependency graph. Rust compilation still requires the MSVC linker/SDK. `rusqlite` and its bundled SQLite C source are Unix-only in `vega-herdr`. |
| `vega`, `vega-skills` | No C build dependencies, but executable ports are not yet in the approved Windows package list. |
| Planned `vega-post` client and worker, `vega-mux` | `ring` 0.17.14 via async-nats/rustls/ureq compiles C/assembly using `cc`; MSVC C tools and Windows SDK headers/libs are required. Recheck after dependency extraction lands. |

`%USERPROFILE%\.cargo` already existed, with rustup 1.29.3, a GNU-local default,
and MSVC 1.88. Rustup/Cargo and Git were absent from the hc core's PATH. Its
Cargo config forces offline sources in `rally-ng-vendor` and a GNU linker under
`rally-ng-rust`. The installer therefore finds rustup explicitly and uses
`%LOCALAPPDATA%\vega-build\cargo` as a separate Cargo home; it never edits that
existing config or changes the default Rust toolchain. It installs the channel
from `rust-toolchain.toml` with the explicit MSVC host, and uses `rustup run`.
Cargo runs from the checkout drive's root with an explicit `--manifest-path`:
otherwise Cargo also loads the profile's `.cargo/config.toml` as an ancestor
of the checkout, even with a separate Cargo home. Checkout-local Cargo config
is likewise not loaded by this isolated build.
If rustup is absent, it downloads official rustup-init into a temporary file
and installs in user space with `--no-modify-path --default-toolchain none`.
Git for Windows must already be installed (the standard Program Files path is
also detected). No mise, WSL, cross-built vega executable, or privileged setup
is involved.

From a non-administrator **Windows PowerShell 5.1** on io, download the installer
outside the checkout (or copy it from a reviewed checkout), then run:

```powershell
Invoke-WebRequest -UseBasicParsing -Uri 'https://raw.githubusercontent.com/durandom/vega/main/scripts/windows-install.ps1' -OutFile "$env:TEMP\vega-windows-install.ps1"
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$env:TEMP\vega-windows-install.ps1" -NativeTls
```

io resets rustup's default Rustls HTTPS connections. `-NativeTls` selects
rustup's native TLS backend **for this process only**; verification remains
enabled. That backend is deprecated upstream, so omit the flag where the
default works. The initial install needed a retry after a transient connection
reset; an already-installed pinned toolchain is reused without a network
update. Toolchain upgrades follow changes to the repository pin.

The script clones `https://github.com/durandom/vega.git` into
`%USERPROFILE%\src\vega`, or fetches and fast-forwards an existing clean `main`
checkout. It refuses dirty trees, a different branch, and non-fast-forward
updates. `-Repository`, `-Ref`, `-Checkout`, and `-InstallDir` select alternatives;
`-SkipUpdate` deliberately builds an existing checkout as-is:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows-install.ps1 -Checkout "$env:USERPROFILE\src\vega" -SkipUpdate -NativeTls
```

The single allowlist is [scripts/windows-packages.json](../../scripts/windows-packages.json):
each entry has a Cargo `package` and explicit `binaries` (names without `.exe`).
The installer and native CI check/clippy/tests all read it. Add executables
only when their Windows ports land on main and pass native CI; an existing
package manifest alone does not demonstrate Windows readiness. Today the list
contains **only vega-plugin and vega-herdr, with no executables**. A successful
run proves a native release library build and reports zero installed executables;
it does not claim a usable vega CLI yet. The post port is vega-nic.28.6.

Builds run at BelowNormal process priority with two Cargo jobs, `--release
--locked --target x86_64-pc-windows-msvc`, and checkout-local
`target\windows-install` output. After a successful build, listed `.exe` files
are copied into `%LOCALAPPDATA%\Programs\vega` and that directory is added to
the **user** PATH once (case-insensitive and trailing-slash-aware). With no
executables the script leaves installation/PATH alone. Exit owned processes
and rerun if an in-use executable prevents copying. No services are restarted
or processes terminated. Open a new shell for PATH changes; existing hc cores
retain their environment, so use explicit paths there. Environment changes
and process priority are restored on exit.

This source installer supersedes the earlier archive-only proposal. An archive
release, private identity storage, enrollment, and pointer-worker startup remain
separate epic work. If another machine lacks MSVC/SDK, stop for the operator to
install Desktop development with C++ and a Windows SDK through Visual Studio
Installer; the script never requests elevation or modifies the system PATH.

### Native validation on io, 2026-11-17

Created `C:\Users\hild\src\vega` from main `edf13d9c`, copied the installer and
allowlist under test, and ran the script through `hc exec --on io-win` using
Windows PowerShell 5.1. It installed the pinned MSVC toolchain (rustc 1.94.1,
clippy, rustfmt) and downloaded crates into the isolated Cargo home. The final
script ran with `-SkipUpdate -NativeTls`: **exit 0**, both libraries built with
`--release --locked --target x86_64-pc-windows-msvc`. The cold release build took
57.75 seconds; the repeat build took 0.11 seconds. Cargo and rustc processes
were observed at BelowNormal priority. No executable or PATH entry was installed
because the allowlist contains libraries only. No admin/toolchain prerequisites
remain for io; adding the executable ports after they land is the remaining step.

`nice -n 10 env -u CODEX_THREAD_ID mise run check` on proteus: **exit 0**, 1,523
Rust tests passed and six skipped, plus the format/lint and script checks.
The gate was disabled by the task brief. The fresh-rustup bootstrap path and
executable copy/PATH branch await testing on a machine/allowlist that needs them;
the actual io runs used existing rustup and an existing checkout.

Host enrollment, worker startup and rollback still need the later epic work.
Use a separate io-win identity, never WSL io's seed, and send only the public
key to the operator. Do not invoke the full daemon automatically or use config
apply, deploy, `mise run install`, or Unix relay lifecycle as a Windows shortcut.

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

## 6. Native pointer worker (vega-nic.28.8)

After installing the native subset and provisioning/enrolling **io-win** with its
own nkey, run `vega-post.exe relay start` as the ordinary Windows user. From a
connected hc machine, the equivalent direct argv start is:

```text
hc exec --on io-win -- vega-post.exe relay start
```

The installed binary directory must be on the core's PATH (or use the installed
absolute executable path). No `sh`, PowerShell script, admin rights or full
vegad is required. Pin `VEGA_POST_HOST=io-win` in the Windows user's environment
or `%APPDATA%\vega\vegad.env` before starting; never reuse the WSL host identity.
The app mod first tries the Unix starter; an exact hc missing-sh launch error
selects `vega-post.exe relay start`. An exact missing-executable launch error
keeps the quiet not-provisioned state and five-minute recheck. Permission,
connection and worker failures retain the ordinary failure/retry path.

`relay start` waits for a lock/readiness handshake from a worker launched through
a temporary demand-only Task Scheduler
task using the current user's interactive token and limited run level. No
password, elevation, service, logon trigger or hc job change is involved. The
temporary definition is deleted after readiness; its running instance survives.
This needs a logged-in desktop user and permission to register user tasks;
failure is explicit. A short-lived profile-local launch file transfers the caller's
environment (including XDG overrides and PATH), is consumed before readiness, and
is also cleaned up on launch failure. Custom state directories require private ACLs.
hc's terminal jobs allow `CREATE_BREAKAWAY_FROM_JOB`, but exec jobs do not
(`repos/herdr-core/crates/herdr-core/src/platform/windows{,_exec}.rs`). Breakaway
also leaves arbitrary inherited ancestor pipe handles open, so every Windows
start uses the task boundary to isolate both process lifetime and handles.
`relay serve` runs in the foreground for diagnostics and Ctrl-C stop.
A user-state file lock (Windows native locking) lasts for the worker's lifetime;
concurrent/repeated starts succeed without creating a second delivery loop.
State and diagnostics live under `%LOCALAPPDATA%\vega\native-relay` (XDG overrides
apply). `relay status [--json]` reports the lock state and worker PID/executable.
`relay stop` asks the lock holder to stop using its per-start token, so stale PID
records never terminate unrelated processes. It works during broker/core outages;
repeated stop is a successful no-op. Foreground workers predating this protocol
need Ctrl-C once. Restart with `relay start` after a stop.

The source installer stages every executable, renames existing images aside, then
moves replacements into place. It stops/restarts a worker only when its recorded
executable belongs to the selected install directory, using the replacement binary's
protocol. Other install directories' workers are left running. Old foreground
workers without metadata require Ctrl-C before installation. Failed updates retain
renamed old images and print recovery paths; successful installs clean up old images
that are no longer mapped. The opt-in core logon autostart remains independent.

The worker polls only local live hc terminals, inspects per-pane unread counts
and ack floors, and publishes its host heartbeat and hc agent keys. The agent
keys include the project ID resolved from the local checkout, recorded lineage
and the bead from a matching spawn record when available.
It never pulls or acknowledges mail: `inbox` retains exclusive ack ownership.
It reuses `delivery.rs` pointer state and draft parsing, preserves pointer state
across worker restarts, retries broker/core failures, and rescans mail after
reconnection. Blocked/unknown targets wait; unreadable screens and visible drafts
also wait. Unlike full vegad it never stashes a draft or forces an unknown input
box. hc rechecks live/status guards at send time. No remote terminal, Unix daemon
job, UAC election, config deployment or administrative provisioning is included.

CI uses an owned Rust hc fake and temporary restricted NATS broker. Logout/core
lifetime and real native harness delivery remain acceptance work in 28.9; no live
io/io-win run is part of that implementation.

## Native UAC registration (vega-nic.28.20)

A parent can run `vega uac adopt hc:io-win/t9` for a live worker on io-win.
The target host’s `vega-uac.exe` verifies the pane and agent name through hc,
then saves the parent, mode and brief before acknowledging. The native
`vega-post` relay publishes the worker’s local project identity so the parent’s
UAC view can list it. Git must be on the relay’s PATH to resolve the checkout.
The view reads the bead from the matching spawn record
when the worker was launched with `vega mux spawn --bead`. Keep the native
relay and UAC binaries on compatible versions.

## Desktop core at logon and session-0 recovery (vega-nic.28.15)

The intended core runs as **hild in a normal desktop session**, never as an
administrator or from SSH. `hc.exe serve` is the server; `hc.exe hold` and
`herdr-core.exe connect` are separate processes. A session-0 transport process
alone is not evidence that the core is in session 0. On 2026-11-17, core PID
46296 was confirmed in desktop session 1 without changing the running core.

Register hidden user logon startup from a checkout containing both scripts:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows-install.ps1 -CoreAutostartOnly
```

Or add `-CoreAutostart` to the normal source installer command. This is **opt-in**:
vega mail clients do not require a local hc installation, and installing clients
should not silently introduce another program at logon. `-HcPath <absolute-path>`
overrides `%LOCALAPPDATA%\Programs\herdr-core\hc.exe`. The registration-only mode
does no build, update, PATH change or core start, so it can be run over SSH.
For a downloaded standalone installer, normal installation reads the helper from
the resulting checkout; registration-only mode requires the sibling helper.

The installer writes `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`
value `VegaHcDesktopCore`, pointing at hidden `wscript.exe` and
`%LOCALAPPDATA%\vega\core-autostart\logon.vbs`. WScript launches Windows
PowerShell without a window; its encoded payload refuses session 0 and an
elevated token, then runs the absolute `hc.exe server start`. No scheduled task,
service, administrator grant or machine registry entry is involved. Reinstalling
overwrites the same value/file; `hc server start` reuses an existing core and
hc's core lock prevents a second server. An inaccessible core is left alone.
Startup errors are logged to `core-autostart\last-start.log`; a session/elevation
refusal exits before invoking hc. This does not guarantee startup before an SSH
client connects, before login, or if Windows disables this Startup App.

At the **next normal desktop logon**, the user verifies (do not log out active
agents just to test):

```powershell
Get-ItemPropertyValue HKCU:\Software\Microsoft\Windows\CurrentVersion\Run VegaHcDesktopCore
Get-Content "$env:LOCALAPPDATA\vega\core-autostart\last-start.log"
vega post doctor
$core = hc server status --json | ConvertFrom-Json
Get-Process -Id $core.pid | Select-Object Id,SessionId,ProcessName
```

Expect one server with a nonzero desktop SessionId (Console is usually 1; RDP
can differ), no startup window, and `hc_core: pid ..., session ... (desktop)`.
`vega post doctor` needs no broker or hc terminal. It uses the existing-only
Windows `hc server status`, then `ProcessIdToSessionId` in vega-herdr's platform
layer. Session 0 returns `hc_core_session_zero`; pipe access denied returns
`hc_core_access_denied` with an elevated/session-0 warning and recovery hint.
Neither probe starts a core. Run `vega post doctor` directly when `status` cannot
verify its caller: doctor needs neither caller identity nor broker access.
Other probe failures are explicit, never a healthy result. This requires Windows
hc with existing-only `server status` (validated against io's 0.1.21).

To remove only the autostart (does not stop a running core):

```powershell
Remove-ItemProperty HKCU:\Software\Microsoft\Windows\CurrentVersion\Run VegaHcDesktopCore -ErrorAction SilentlyContinue
Remove-Item "$env:LOCALAPPDATA\vega\core-autostart" -Recurse -Force
```

### Recovery: user-operated, after coordinating active agents

Stopping the core interrupts shared work. Arrange a maintenance window first.
In an **elevated Windows terminal**, run:

```powershell
hc server stop
```

Close that elevated terminal. In a **normal desktop terminal as hild**, run:

```powershell
hc server start
vega post doctor
```

Files created under the SSH/elevated token can belong to
`BUILTIN\Administrators`, causing Git dubious ownership and harness login/state
problems. After reviewing the affected directory, repair ownership from an
appropriately privileged terminal, scoped to that directory:

```powershell
icacls <dir> /setowner io\hild /T /C /Q
```

`/C` continues after failures: inspect the summary and repair remaining failures.
Do not apply this indiscriminately to a whole drive, system directories, or
another user's data. Ownership repair does not reset ACLs or recover missing
credentials; verify Git and harness login again from a normal desktop terminal.

### Can SSH connect be prevented from starting the core?

Reviewed `repos/herdr-core` at `de97ea5`, especially `cli.rs` (`Connect`),
`client.rs` (`connect_starting`), `platform/windows.rs`,
`platform/windows_session.rs`, and `machines.rs` (`connect_command`). There is
no `hc connect --no-start` flag or machine-entry setting for existing-only
connect. The existing guard refuses implicit elevated SSH startup **when the
same user has a desktop session**; explicit `server start` bypasses it. With no
desktop session, implicit SSH startup can still create a session-0 core. Logon
autostart reduces that window but does not close it. io's installed 0.1.21
`connect --help` likewise offers only `--session` and `--json`.

No proteus/ganymede machine entries were changed. Proposed upstream issue:

> Add existing-only remote connect for Windows desktop hosts. When a Windows
> core is absent, SSH `hc connect` can implicitly create it in Services/session
> 0 with an elevated network token. Desktop terminals then get pipe access
> denied, harness credentials differ, and generated files can be owned by
> Administrators. Add `hc connect --no-start` (using `require_existing`) and a
> per-machine `autostart = false` setting passed by `connect_command`. Preserve
> `--session`, return a structured unavailable error with a desktop-start hint,
> and never create state, spawn a server or request UAC on this path. Test absent,
> inaccessible and healthy cores, including SSH with no logged-in desktop.

Once supported, enable existing-only connect for the **io-win entries only** on
proteus and ganymede. Do not disable implicit startup for unrelated headless hosts.

See [Native vegad on Windows](vegad-windows.md) for the sidebar milestone,
worker inventory, desktop startup, IPC and UAC health.
