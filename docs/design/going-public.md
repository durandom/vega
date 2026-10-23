# Going public: audit of durandom/vega

Status: working-tree scrub complete, 2026-10-22 (vega-lhc.186); original audit: vega-lhc.185.
No visibility change, history rewrite or GitHub settings change was made by the scrub.
This report names files, commits and kinds only, never values.

Original audit scope: all 605 commits reachable from any local or remote ref (33 branches, 147 merges), the working tree
at `df09d8b`, and the 10 GitHub pull requests (their `refs/pull/*` stay reachable after any rewrite).

## Verdict

**The working tree is scrubbed. S1 remains a publication blocker for the old history.**
GitHub keeps `refs/pull/<n>/head` for the 10 existing PRs, so a filter-repo plus force push does not
remove that exposure. The Slayer chose a **new** public repository with a single commit of the
scrubbed tree, keeping this repository and its old history private (vega-lhc.188).

## Go/no-go list

1. **Open S1:** keep the old history and PR refs private; publish only a fresh snapshot in the new
   repository (vega-lhc.188). The private credential inventory and its path references are removed
   from this tree; preserve any still-needed operational content only in private storage.
2. **Done I1–I4:** endpoints require local configuration, docs use placeholders, the agentsview
   sidecar is generic, and PagerDuty/NATS examples use fixtures.
3. **Done L1–L3:** vscode uses Apache-2.0; the copied authoring page is an upstream link; AXI and
   Beads have source attribution and MIT licenses; caveman styles use new Apache-2.0 wording.
4. **Done CI:** `.github/workflows/ci.yml` is in the tree, including secrets and check jobs.

Items marked "acceptable public" need no action.

## Secrets

Original audit scans, gitleaks 8.30.1, redacted:

- `gitleaks git --log-opts=--all`: 455 commits scanned (non-merge commits; merges add no content of their
  own), **no leaks**.
- `gitleaks dir .` (working tree): **no leaks**.

Manual checks (history, `git log --all -p -S/-G`):

- NATS nkey seeds (`SU…`), age secret keys, `.env`/`vegad.env` files, `.nk` files: none ever committed.
  `spike/nats/gen-auth.sh` (deleted) wrote seeds to a state dir outside the repo.
- `op://` references in code and docs are 1Password refs, not values. Acceptable public.
- No file was ever added and deleted to hide a value: the 9 files deleted in history were checked
  (`.beads/config.yaml`, `.beads/metadata.json`, `TODO.md`, `spike/nats/gen-auth.sh`, code files).

## Findings

| ID | Where | Kind | Severity | Recommendation |
| --- | --- | --- | --- | --- |
| S1 | Private credential inventory in old history (added 5f4d4ef, last 421c0b9) | Personal credential and project map, without values | High | **Tree removal done; history safeguard open.** Keep old history private and publish the fresh scrubbed snapshot. Any retained operational map belongs in private storage. |
| I1 | Historical compiled defaults in registry, bd-sync, config, janitor, post and bd; docs and `.beads/PRIME.md` | Private service addresses, first 29362b3 | Medium | **Done in this tree.** Compiled endpoints removed; environment/local endpoint files provide addresses, docs use placeholders. See [service endpoints](config.md#service-endpoints) and CHANGELOG 67 before upgrading hosts. Old addresses remain only in private history. |
| I2 | `config/agentsview/LOCAL-SETUP.md` | Former personal ops note | Medium | **Done.** Generic example replaces personal content; private deployment instructions use the local source override documented there. |
| I3 | `docs/design/pagerduty.md`, `plugins/post/src/pagerduty/tests.rs` (first a44796e) | Former real service id and account examples | Low | **Done.** Service IDs and account/incident links use fixtures. Shipped changelog blocks retain their historical service-name example. |
| I4 | `docs/design/vega-mux-broker-spike.md` (first bb81f94) | Former real NATS user public key | Low | **Done.** Replaced by `U…`; no real key remains in that example. |
| I5 | `docs/design/lanista-catalog.md`, `plugins/post/src/quarantine.rs`, `plugins/mux/src/main.rs`, `plugins/post/src/{auth,main}.rs`, `TODO.md` (deleted); first b8c45c1, the tag line in `TODO.md` 4870aae | A host is described as corp-managed; the deleted `TODO.md` also names its Tailscale tag | Low | **Acceptable public**: explains the inbox-only design, no employer named. Optional: drop the host name. |
| I6 | Host names (proteus, ganymede, titan, deimos, triton, Io) throughout docs, code and tests | Machine names | Info | **Acceptable public**: not addressable without the tailnet. |
| I7 | `/Users/mhild`, `~/src/<project>` paths in tests and docs; names of other own projects (spellkave, lanista, orrery, b4arena, racing) | Personal paths and project names | Info | **Acceptable public**: own public identity; no private details of those projects outside S1. `docs/design/janitor.md` names an employer product in one line: optional scrub. |
| I8 | Commit authors: Marcel Hild, Christoph Görn (both `@b4mad.net`), `pi-rewind <rewind@pi>`; `git.b4mad.industries/goern/rs.cms#103` in `crates/vega-ci-watch` | People names and emails | Info | **Acceptable public**: both are the project's contributors. Other emails are `example.*` fixtures. |
| I9 | `docs/design/vega-mux-broker-spike.md`, `TODO.md` (history) | Known weaknesses of the broker auth model (forged sender via push consumers, stream flooding) | Info | **Acceptable public**: the broker is tailnet-only; documenting limits is fine. |
| I10 | `.beads/` | Only `PRIME.md`, `README.md`, `.gitignore` are tracked; the issue DB never was (`metadata.json` with a project UUID was, deleted in 00281d8) | Info | **Acceptable public.** I1 scrub of `PRIME.md` is done. |
| I11 | Screenshots, images, PDFs | None ever committed | — | — |

## Licensing

| ID | Where | Finding | Recommendation |
| --- | --- | --- | --- |
| L0 | `LICENSE` | Apache-2.0 at the root since ccfc8a4; `plugins/board` keeps MIT (`plugins/board/LICENSE`, derived from herdr-beads) | **Acceptable public.** All 35 crates set `license` in their `Cargo.toml`. |
| L1 | `vscode/package.json` | License now `Apache-2.0` | **Done.** Consistent with the root license. |
| L2 | `docs/skill_best_practices.md` | Former copied Anthropic page is now an upstream link | **Done.** No verbatim page remains in the tree. |
| L3 | AXI/Beads skill directories and `config/claude/output-styles/caveman-*.md` | MIT skill sources/licenses recorded; replacement styles written for this repository under Apache-2.0 | **Done.** Skill headers name their source and adjacent license. Old caveman provenance was unverified, so that wording was replaced rather than attributed speculatively. |
| L4 | `docs/design/agentcraft-research.md` | Describes a proprietary package; quotes only its license line, no code | **Acceptable public.** |
| L5 | `repos/` | gitignored, never committed | — |

## CI

`.github/workflows/ci.yml` runs on every push and pull request, with read-only repository permissions
and SHA-pinned actions. The two required checks at the public move are `secrets` and `check`:

- `secrets` checks out full history (`fetch-depth: 0`) and runs gitleaks 8.30.1 from the mise pin:
  `gitleaks git --config .gitleaks.toml --log-opts=--all --redact --exit-code 1`. This scans all fetched
  refs, including PR history, rather than just the current diff. Findings fail the job; logs redact
  values. `.gitleaks.toml` extends all default rules. No fixture exclusions are currently necessary;
  future allowlist entries must identify a specific synthetic fixture and rule, never whole source
  directories or commits. There is no historical baseline suppressing existing findings.
- `sh scripts/gitleaks.test.sh` proves clean history passes and a synthetic GitHub token committed and
  then deleted in a temporary repository still fails with exit 1 and rule `github-pat`. The temporary
  repository is removed on exit; no token is committed to vega.
- `check` runs `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`, `cd vscode && node --test`, and the shell checks in `mise run check`:
  `githooks/wt-guard.test.sh`, `shims/activate.test.sh`, `scripts/vegad-refresh.test.sh`,
  `scripts/readmes.test.sh` (README/plugin/shim inventory), and `scripts/inline-md.test.sh`.
  Rust comes from `rust-toolchain.toml`; cargo runs under `nice` and `RUST_TEST_THREADS=1` serializes
  tests that modify process environment. NATS tests launch their own local JetStream broker using
  `nats-server` 2.15.0 from mise; no tailnet broker or herdr installation is needed. Live forge tests
  explicitly report `ignored` with the authenticated-service requirement; the installed-`bd` parser
  test and opt-in identity dry run also remain ignored. The catalog subprocess test is ignored only
  at the top level and is invoked by its parent test.

At the move, the **UAC** enables GitHub **Secret scanning** and **Push protection** in the new public
repository's security settings before the first public push. Configure the main-branch ruleset to
require the `secrets` and `check` status checks before merging. These settings supplement CI: push
protection can block supported secrets before they land, while gitleaks inspects fetched Git history.
Verify scanning even where public-repository scanning is enabled automatically; see GitHub's
[secret scanning setup](https://docs.github.com/en/code-security/how-tos/secure-your-secrets/detect-secret-leaks/enable-secret-scanning)
and [push protection setup](https://docs.github.com/en/code-security/how-tos/secure-your-secrets/prevent-future-leaks/enable-push-protection).
This task changes no GitHub settings.

### Local Linux proof

Use a disposable Ubuntu 24.04 container (Git 2.43 or newer is needed by the worktree tests), a full
clone of the branch, build-essential, pkg-config, libssl-dev, curl, CA certificates, Node.js, and
Python 3 (the fake herdr fixture uses it; GitHub's Ubuntu runner already includes it).
Install mise in the container, then run the following in the clone with mise on `PATH`:

```sh
mise trust
mise install nats-server gitleaks
# Container-only Rust setup; CI uses the runner's rustup instead.
mise use -g rust@1.94
mise exec rust@1.94 -- rustup toolchain install
mise exec rust@1.94 -- rustup show active-toolchain
git config --global user.name ci
git config --global user.email ci@example.invalid
mise exec -- gitleaks git --config .gitleaks.toml --log-opts=--all --redact --exit-code 1
mise exec -- sh scripts/gitleaks.test.sh
RUST_TEST_THREADS=1 nice mise exec rust@1.94 -- mise run check
```

The checkout and cache wrappers are not emulated by this dry run; every check command runs inside
Linux. Git 2.39 in the earlier `rust:1.94-bookworm` audit image was too old for
`git worktree add --orphan`, so the proof uses Ubuntu 24.04 instead.

2026-10-22 verification: all of the above commands passed in an Ubuntu 24.04 Podman container
(aarch64, Git 2.43.0, Node 18.19.1, Rust 1.94.1). `mise run check` exited 0, including local NATS
tests; the clean-history gitleaks scan exited 0, and the deleted synthetic secret produced the
expected exit 1. No herdr or bd executable was installed in the container.

## Follow-ups

- Working-tree scrub (I1–I4, L1–L3): implemented here, vega-lhc.186; validation and gate results
  are recorded on that bead. Preserve host-local configuration before deploying.
- New-repo publication and private archive: vega-lhc.188, not done here. The chosen fresh snapshot
  excludes old history; no history rewrite was performed by the scrub.
