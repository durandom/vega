# Secrets: API keys and credentials for tools and agents across hosts

Status: built (first version), 2026-10-11. Bead: vega-hnh. Decided with the Slayer one question at a time; the
decision log is in the bead's notes. Code: `crates/vega-secrets` (manifest, age, lookup),
`plugins/secrets` (`vega secrets`).

## Problem

API keys are a general problem, not a per-tool one. `lanista fetch` skips Artificial Analysis on
proteus because `ARTIFICIAL_ANALYSIS_KEY` is not set; `vega bd init` needs a SQL credential for
titan:3306 to create a project's database (vega-lhc.76, [beads-sync.md](beads-sync.md)). Each host
must get the keys it needs without a human typing a password, and without a value ever reaching
logs, argv, the NATS registry, `vega post` or an agent's output.

## Decisions

| # | Topic | Decision |
| --- | --- | --- |
| 1 | Source | chezmoi + age, the dotfiles' existing pattern. Rejected: 1Password as the delivery source, macOS Keychain (Mac only, filled per host by hand). |
| 2 | Delivery | Decrypt on demand, no plaintext files. `vega secrets run --for <tool> -- <cmd>` injects the tool's keys into that command's env; vega plugins use a Rust lookup (value in memory only). |
| 3 | Storage | A chezmoi-managed manifest without values, plus one ciphertext file per secret. |
| 4 | Hosts | One age identity per host; each secret is encrypted only to its hosts. |
| 5 | Admin, originals | Admin host = proteus. The values are also kept in 1Password (items tagged `vega`), the original for humans; `set` reads them with `op read`. |
| 6 | Inventory | `vega secrets` shows this host's view; values are never printed. |
| 7 | Rotation, audit | Rotate in 1Password, then `set`; a local audit log per host plus git and 1Password history. |

### No prompt, therefore no security boundary

Host identities have no passphrase, like the chezmoi `key.txt` today: agents and vegad never wait
for the Slayer. The same property lets any process of the user decrypt everything its host may
read, including every agent (P7, "secure the box, not the agent"). Per-tool scopes are therefore
hygiene: an agent does not get every key into its env or output by accident. The real boundaries
are the host (a host holds only its own secrets) and the box.

## Layout

```
~/.config/vega/secrets.toml          manifest (chezmoi-managed, plaintext, no values)
~/.config/vega/secrets/<name>.age    ciphertext (chezmoi-managed, plain file, not encrypted_)
~/.config/vega/age/host.key          this host's age identity (0600, never leaves the host,
                                     not in chezmoi)
$XDG_STATE_HOME/vega/secrets/audit.log
```

The `.age` files live in the chezmoi source as ordinary files (`chezmoi add` makes them
`dot_config/vega/secrets/private_<name>.age`), not with the `encrypted_` prefix: `chezmoi apply`
copies ciphertext (0600) and never writes plaintext (verified with chezmoi against a throwaway
source).
The chezmoi `key.txt` and its own encrypted files stay as they are.

### Manifest

```toml
admin = ["proteus"]

[hosts.proteus]
recipient = "age1..."
[hosts.ganymede]
recipient = "age1..."

[secrets.artificial-analysis]
env = "ARTIFICIAL_ANALYSIS_KEY"
for = ["lanista"]
hosts = ["proteus", "ganymede"]
op = "op://Private/Artificial Analysis/credential"
# written by set and rekey, never by hand:
encrypted_to = ["proteus", "ganymede"]
rotated = "2026-10-11"

[secrets.titan-dolt]
env = "VEGA_TITAN_DOLT_PASSWORD"
for = ["vega-bd"]
hosts = ["proteus", "ganymede"]
op = "op://Private/titan dolt vega/password"
```

- `for`: the tools that may receive the secret (`--for <tool>`, the lookup's caller name).
- `hosts`: the hosts that may decrypt it. Admin hosts are added as recipients of every secret,
  so `rekey` can always decrypt.
- `encrypted_to`: the recipients of the current ciphertext. age's X25519 stanzas do not name
  their recipient, so `set` and `rekey` record it here.
- `op`: an `op://` reference, not a value. Optional.

## Commands

| Command | Who | What |
| --- | --- | --- |
| `vega secrets` | anyone | Home view (below). |
| `vega secrets run --for <tool> -- <cmd> [args]` | agents, scripts | Decrypt the tool's secrets for this host, exec `<cmd>` with them added to its env. |
| `vega secrets set <name> [--env E --for T --hosts H --op REF]` | Slayer, admin host | Read the value (`op read` if the secret has an `op` ref, else a hidden TTY prompt, else stdin if it is not a TTY), encrypt, write `.age` and the manifest entry to the chezmoi source, then `chezmoi apply` those two paths. Never a value argument. |
| `vega secrets scan [dir] [--for T]` | agents | Secret-like variables in a checkout's `.envrc`/`.env*`: names, how held, git-tracked, in vega. |
| `vega secrets adopt <file> <VAR>... --for T` | UACs, admin host | Move the values into vega secrets (dedupe by value, 1Password item `<name>-api`), rewrite the file to `use vega T`. |
| `vega secrets rekey` | Slayer, admin host | Decrypt every secret with the admin host's identity and re-encrypt it to its current recipients (after a host was added or removed). |
| `vega secrets host init [--admin]` | Slayer, new host | Create `host.key` if missing (idempotent); print the public key and the `host add` line for the admin host. `--admin` on the first host starts the manifest with it as admin. |
| `vega secrets host add <host> <age1...> [--secret <name>]...` | Slayer, admin host | Record the host's public key, add it to the named secrets' `hosts` and rekey those. |
| `vega secrets --skill` | agents | The guide. |

Committing and pushing the chezmoi source stays with the Slayer.

### Rust lookup

```rust
let pw = store.get("titan-dolt", "vega-bd")?;   // Secret: no Display, Debug prints "***"
```

`get` checks the manifest (`for` lists the caller, `hosts` lists this host), decrypts in process
with the `age` crate, appends to the audit log and returns a wrapper that does not print. A
consumer that passes the value to another program uses its env (e.g. `DOLT_CLI_PASSWORD` for
`dolt sql`), never argv.

### Errors (structured, AXI 6)

`unknown_secret`, `not_for_tool` (the tool is not in `for`), `not_for_host`, `missing` (no `.age`),
`undecryptable` (not encrypted to this host: run `vega secrets rekey` on the admin host),
`no_identity` (run `vega secrets host init`), `not_admin`, `no_tty` (set without a TTY, a pipe or
an op ref). Each names the fix; none contains a value.

## Inventory: `vega secrets`

```
host: proteus (admin)  identity: ok
secrets[2]{name,env,for,hosts,here,rotated}:
  artificial-analysis,ARTIFICIAL_ANALYSIS_KEY,lanista,proteus+ganymede,ok,2026-10-11
  titan-dolt,VEGA_TITAN_DOLT_PASSWORD,vega-bd,proteus+ganymede,missing,-
help: vega secrets set titan-dolt   # on proteus, reads op://...
```

- `here`: `ok`, `missing`, `undecryptable` (`hosts` lists this host, `encrypted_to` does not),
  `not_for_host`. `ok` is checked without decrypting (file present, host in `encrypted_to`).
- Only this host. Other hosts are not queried over NATS or `vega post`; run `vega secrets` there.
  `encrypted_to` shows which hosts can decrypt each secret once their chezmoi is up to date.
- `--op` (admin host only): compare with `op item list --tags vega` (metadata only) and report
  tagged items without a manifest entry and entries whose item is gone.

## Projects: direnv (vega-fsh)

Decided with the Slayer, 2026-10-11 (log in the bead's notes):

- A project is a tool: its `.envrc` says `use vega <tool>` and direnv exports the secrets whose
  `for` lists `<tool>`, and only those. An agent shell in that checkout gets that project's keys,
  not every key (hygiene, see "No prompt" above). Rejected: `vega secrets run` wrappers in each
  project's tasks (stricter, but a rewrite per project).
- The helper `use_vega` (`plugins/secrets/direnv/vega.sh`, installed by `install-cli.sh` to
  `~/.config/direnv/lib/vega.sh`) is `direnv_load vega secrets run --for <tool> -- direnv dump`:
  values pass through direnv's own dump file, never stdout or argv. Every `cd` into the checkout
  is an audit line. Without vega installed it logs an error and loads nothing.
- 1Password: one API Credential item per key, title `<name>-api` (titles match
  case-insensitively, `openai` collided with the `OpenAI` login), tag `vega`, value in field
  `credential`. The old `cli-api-keys` item stays until nothing reads it.
- Migration: each project's UAC runs `vega secrets scan` in its checkout, then `vega secrets adopt
  <file> <VAR>... --for <tool>` on proteus. `adopt` reads the value from the file (or its `op://`
  ref) in process and dedupes: same env and value as a secret adds the tool to its `for`; the same
  value under another env name does that too and writes an alias after the use line
  (`export JIRA_PERSONAL_TOKEN="$JIRA_API_TOKEN"`); same value as the item `<name>-api` reuses it;
  else it creates the item. It removes the lines, adds
  `use vega <tool>` to the `.envrc` next to the file, and `chezmoi add`s the changes. A lock file
  serialises concurrent adopts. The Slayer runs `direnv allow` (trust prompt) and commits the
  chezmoi source.
- `set` accepts a secret without `--for`: stored, given to no tool yet (the split 1Password
  items, loaded so that `adopt` finds them by value).
- `.env` files read by apps keep working when the app runs in the direnv shell: dotenv loaders do
  not override variables already in the env. Service secrets of apps deployed elsewhere stay with
  those projects.

## Cross-host and onboarding

Each host fetches ciphertext from the chezmoi repo itself (`chezmoi update`). Secrets never travel
over the NATS registry or `vega post`, and no private key travels at all.

New host (e.g. the machine that replaces deimos):

1. On the new host: `chezmoi init/apply`, install vega, `vega secrets host init`. The command prints
   the public key.
2. On proteus: `vega secrets host add <new> <age1...> --secret <name>` for the secrets it needs,
   commit, push.
3. On the new host: `chezmoi update`; `vega secrets` shows `ok`.

Headless hosts (titan, a Linux box) take the same steps: a host identity needs no unlock, and no
1Password service account is needed, because hosts never talk to `op`.

Host lost or compromised: remove it from `[hosts]` and every `hosts` list, `rekey`, then rotate
the secrets it held (its old ciphertext is in git history).

## Rotation and audit

- Rotation: change the value in 1Password, `vega secrets set <name>` on proteus (one Touch ID for
  `op read`), commit, push, `chezmoi update` on the hosts. `rotated` records the date.
- Audit: each `run` and `get` appends `time,secret,tool,caller` to
  `$XDG_STATE_HOME/vega/secrets/audit.log` (caller: `CLAUDE_CODE_SESSION_ID`, `HERDR_PANE_ID` or
  pid). Changes are in the chezmoi git log, value history in 1Password.
- No expiry dates or reminders (two secrets; YAGNI).

## Rules for agents

- Use `vega secrets run --for <tool> -- <cmd>`; never read `.age` files or `host.key`, never print
  an env that holds a secret, never pass a value as an argument.
- Never run `set`, `rekey` or `op`; ask the Slayer to type the command (`! vega secrets
  set <name>` works in a Claude pane).

## First version

- `crates/vega-secrets`: manifest, host identity, encrypt/decrypt (`age` crate), `get`, audit.
- `plugins/secrets`: home, `run`, `set`, `rekey`, `host init`, `--skill`.
- Consumers: lanista (`vega secrets run --for lanista -- lanista fetch`; `vega lanista` shows
  `aa_key: env | vega secrets <state> | missing`) and `vega bd` (`get("titan-dolt", "vega-bd")`, vega-lhc.76).
- `set`, `rekey`, `host init --admin` and `host add` end with `chezmoi add` of the changed files;
  committing and pushing stays with the Slayer.
- Not in the first version: Windows (Io).

## Bootstrap on proteus (the Slayer)

```
vega secrets host init --admin
vega secrets set artificial-analysis --env ARTIFICIAL_ANALYSIS_KEY --for lanista \
  --op 'op://<vault>/<item>/<field>'
vega secrets set titan-dolt --env VEGA_TITAN_DOLT_PASSWORD --for vega-bd --hosts proteus,ganymede --op 'op://<vault>/<item>/password'
chezmoi git -- add -A && chezmoi git -- commit -m 'vega secrets' && chezmoi git -- push
```
