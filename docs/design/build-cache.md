# Bounded Cargo build caches

Vega uses one `target/` per checkout (`mise.toml`), so worktrees cannot overwrite
one another's binaries. The user's `~/.cargo/config.toml` remains user-owned.

On macOS, Cargo defaults to `split-debuginfo = "unpacked"` when debug info is
requested. This leaves each linked executable's object files beside it in
`target/debug/deps`. Source and build identity changes can leave old objects
behind. On 2026-11-09 proteus reached 865,763 deps entries (855,155 objects);
syspolicyd enumeration during code-signing checks contributed to a desktop hang.
This is a build-cache problem: no Gatekeeper, TCC, syspolicyd or Spotlight
exceptions are appropriate.

`vega config infra` checks this checkout's `target/debug/deps` (using
`CARGO_TARGET_DIR` when configured) and warns above 50,000 entries. The scan
stops at 50,001 entries, without sorting, recursing or discovering other
worktrees. A missing target is normal; directory read errors are reported.

## Debug information

The measured choice is `debug = "line-tables-only"` plus
`split-debuginfo = "packed"` in the workspace dev profile (the test profile
inherits it). This keeps source lines in application backtrace frames and
places debug information in one dSYM bundle per executable, rather than many
loose `.o` files. Dependencies retain `debug = false`.

`debug = false` and `split-debuginfo = "off"` also stop object retention.
They preserve assertion text, expected/actual values and the panic header's
file and line, but the macOS probe loses source locations in application
backtrace frames. Packed was chosen to retain that diagnostic value.

A standalone three-build Rust probe, with a new artifact suffix each build,
confirmed the following deps counts. All modes preserved assertion text and
the panic header location; only unpacked and packed retained application
backtrace source lines. The regression test checks the configured profile
with incremental compilation enabled.

| Profile | Incremental off: entries / .o | Incremental on: entries / .o |
| --- | ---: | ---: |
| line tables, unpacked | 6 / 3 | 21 / 18 |
| debug=false | 3 / 0 | 3 / 0 |
| line tables, packed | 6 / 0 | 6 / 0 |
| line tables, off | 3 / 0 | 3 / 0 |

## Bounded maintenance

`mise run check` runs `scripts/target-cache.py` after the build commands and
regression tests, before the successful-check stamp. `mise run target-sweep`
runs it manually; `mise run target-sweep -- --dry-run` previews eviction.

The script takes an exclusive nonblocking lock on `target/debug/.cargo-lock`,
the same lock Cargo uses on macOS (verified against a live build). A running
build causes a successful skip, without waiting or deleting anything; a build starting during a sweep
waits for Cargo's lock. Release output is not swept.

Deps files with the same 16-digit Cargo unit hash form a family (`.d`, `.rlib`,
`.rmeta`, binaries, loose objects and dSYM bundles). Each incremental crate
cache directory is another family. A family is eligible only when **every
member**, including nested directories and files, has a modification time
strictly older than seven days. A fresh member protects the whole family.
Cargo can reuse an artifact without modifying it, so a rarely rebuilt active
dependency can be evicted and rebuilt later; that tradeoff is intentional.

Each invocation inspects at most 100,000 directory entries and removes at most
5,000 files, links or directories. Names and family hashes are sorted for a
deterministic order. Only complete families that fit the remaining deletion
budget are removed. A scan limit keeps incomplete families; a deletion limit
keeps whole families for the next run. Neither limit is a reason to run during
a build. The script warns when a limit prevents progress.

Only this checkout's real `target/` is accepted. A shared/global target override,
a symlinked target/profile/deps/incremental root, or a symlinked Cargo lock is
refused. Links inside an eligible family are unlinked without following them.
The script does not visit other targets, `.fingerprint`, build-script outputs,
release artifacts or the user-owned global cache.

A family larger than 5,000 entries cannot fit a run, and a directory larger
than the scan budget cannot be classified safely. If repeated sweeps cannot
reduce an oversized cache, stop builds in **that checkout**, inspect
`vega config infra`, and have its owner run `nice -n 10 mise exec -- cargo clean`
there. This is explicit recovery, never an automatic escalation of the sweep's
scope or limits. The profile change prevents new loose-object accumulation;
regular age-based eviction handles stale binary/configuration hashes.

Tests use temporary fixtures for old and fresh sibling protection, nested
incremental/dSYM caches, dry runs, both budgets, repeated bounded progress,
symlink safety and build-lock exclusion. The macOS profile regression compiles
two incremental build identities and checks zero loose objects and readable
panic/backtrace diagnostics; it fails against unpacked.

## Profile comparison and verification

Measurements for vega-htsq use this task's isolated worktree on macOS arm64,
Rust 1.94, two Cargo jobs and two nextest threads, with builds prefixed by
`nice -n 10`. Counts use `os.scandir`, not `ls`. Full checks include the Rust,
JavaScript, shell and Python checks, so these are end-to-end wall times rather
than linker benchmarks. The first run warms dependencies; subsequent profile
comparisons reuse the same checkout cache. Worktree incremental compilation is
off; the main checkout enables it. These modes must be distinguished when
interpreting deps and incremental directory growth.

The initial profile builds shared one target directory so that no other cache
was touched. Totals are cumulative; `.o` files from unpacked remain until
swept. First attempts of unpacked, debug=false and off stopped on new infra
summary/changelog/guide assertions during development and were retried; those
failed wall times are excluded from successful-check comparisons.

| Profile | First build: deps before → after | New .o | Successful full check |
| --- | ---: | ---: | ---: |
| unpacked | 0 → 3,060 | 911 | 170.99 s (warm retry) |
| debug=false | 3,060 → 3,650 | 0 | 169.92 s (warm retry) |
| packed | 3,650 → 5,918 | 0 | 183.64 s (warm); 267.13 s (first profile build) |
| off | 5,918 → 8,067 | 0 | 162.25 s (warm retry) |

References: [Cargo profiles](https://doc.rust-lang.org/cargo/reference/profiles.html#split-debuginfo),
bead vega-htsq.

## Incremental and user-owned caches

The main checkout keeps `CARGO_INCREMENTAL=1`; worktrees keep it off. The
standalone probe above exercises the main checkout's incremental mode without
building in or inspecting its target. The worktree's actual
`target/debug/incremental` had zero entries during the profile comparisons.
The fix changes debug-info packaging, not that incremental policy.

A worktree-only ignored `mise.local.toml` then enabled incremental compilation
for a full packed workspace check, emulating the main checkout without touching
it. That check passed in 263.67 s: deps went from 8,067 to 8,776 entries, with
zero new `.o` files (911 legacy unpacked objects remained). Incremental caches
contained 333 crate directories and used 2,093,064 KiB (about 2.00 GiB).
The final repeated checks use the same incremental override, two Cargo jobs
and four nextest threads. The override is removed before handoff.

At 17:47 CET on 2026-11-09, a read-only `nice -n 10 du -sk
~/.cache/cargo-target` reported 2,141,480 KiB (about 2.04 GiB), down from the
incident's reported 73 GiB. That global cache is user-owned; the maintenance
script neither changes `~/.cargo/config.toml` nor sweeps the global cache,
other worktrees, or the main checkout from a worktree.

## Twenty-check soak

After implementing the packed profile and bounded sweep, 20 consecutive full
`nice -n 10 mise run check` runs passed with incremental compilation enabled,
two Cargo jobs and four nextest threads. Before and after **every** run:

- Deps entries: **8,776 → 8,776**, below the 50,000-entry warning threshold.
- Loose objects: **911 → 911** (legacy unpacked comparison artifacts); **zero
  new `.o` files**. Fresh legacy families are deliberately kept until eligible.
- Incremental crate directories: **333 → 333**.

Full-check time: minimum 87.80 s, median 90.15 s, maximum 113.72 s; total 1878.39 s.

| Run | Full check (s) | Deps after | Exit |
| ---: | ---: | ---: | ---: |
| 1 | 103.00 | 8,776 | 0 |
| 2 | 89.74 | 8,776 | 0 |
| 3 | 102.89 | 8,776 | 0 |
| 4 | 92.63 | 8,776 | 0 |
| 5 | 94.02 | 8,776 | 0 |
| 6 | 110.48 | 8,776 | 0 |
| 7 | 113.72 | 8,776 | 0 |
| 8 | 90.56 | 8,776 | 0 |
| 9 | 89.35 | 8,776 | 0 |
| 10 | 87.98 | 8,776 | 0 |
| 11 | 88.00 | 8,776 | 0 |
| 12 | 88.40 | 8,776 | 0 |
| 13 | 89.57 | 8,776 | 0 |
| 14 | 91.69 | 8,776 | 0 |
| 15 | 88.22 | 8,776 | 0 |
| 16 | 90.74 | 8,776 | 0 |
| 17 | 87.80 | 8,776 | 0 |
| 18 | 102.61 | 8,776 | 0 |
| 19 | 88.01 | 8,776 | 0 |
| 20 | 88.98 | 8,776 | 0 |

The temporary incremental override was removed after the soak. The worktree
returns to its normal incremental-off policy; the main checkout remains
incremental-on. The sweep itself removed no fresh comparison artifacts; the
fixture tests demonstrate age-based eviction and the safety/budget boundaries.

After the soak, incremental caches still had 333 crate directories and used
2,551,080 KiB (about 2.43 GiB), versus 2,093,064 KiB before: +458,016 KiB
(about 447 MiB). Thus the soak establishes stable deps and incremental **crate
directory counts**, not a hard incremental byte cap. The sweep ages unused
crate cache families; active incremental contents remain compiler-managed.

After removing the override, a final ordinary-worktree `mise run check` passed
(exit 0) with `CARGO_INCREMENTAL=0`. The built `vega config infra` also exited 0
and reported `cargo-target: ok: 8776/50000 deps entries`. Final deps remained
8,776, with the same 911 legacy comparison objects and zero new loose objects.
