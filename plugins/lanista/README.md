# vega lanista

Manages [lanista](https://github.com/b4arena/lanista), the Slayer's catalog of LLMs with prices and benchmarks.
vega installs and upgrades it from git, ships its seats allowlist (`seats.json` here) to every host, and answers
which model id to start per provider and tier. Queries on the catalog itself go to `lanista`.

Commands: `vega lanista`, `install`, `pin`, `doctor`.

`pin <lane> --tier <tier>` resolves the cwd project's registry override before global seats and
names its source host. Set with `--set <model>` (default expiry: binding provider quota reset),
optionally `--until 2h|never`; `--clear` returns to global seats. Registry read failures visibly
use global seats; mutations require broker access. No repository pin file is written.

Design: [lanista-catalog.md](../../docs/design/lanista-catalog.md).

Agent guide: `vega skills lanista`

Project identity uses the flag-day v2 protocol; deployment and exact-grant
requirements are in [the registry cutover handoff](../../docs/design/project-registry-cutover.md).
