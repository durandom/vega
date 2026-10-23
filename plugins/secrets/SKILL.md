---
name: vega-secrets
description: Supplies API keys and credentials to tools, projects and agents on this host without printing them, by injecting only one tool's secrets into a command's or direnv's env, and moves keys out of .envrc/.env files. Use when a tool needs an API key or password (lanista fetch, vega bd), when a key is reported missing, when a checkout's .envrc or .env holds keys, or when onboarding a new machine.
---

# vega secrets: keys for tools, never in output

The Slayer keeps the originals in 1Password; each host gets age ciphertext through chezmoi and decrypts it with its own identity. A secret names the tools it is for and the hosts that may decrypt it.

- `vega secrets`: this host's secrets with env var, tools, hosts and state (`ok`, `missing`, `undecryptable`, `not_for_host`). No values.
- `vega secrets run --for <tool> -- <cmd> [args]`: runs the command with that tool's secrets in its env. Example: `vega secrets run --for lanista -- lanista fetch`.
- Projects (direnv): `use vega <tool>` in the `.envrc` exports that tool's secrets. `vega secrets scan [dir]` lists a checkout's `.envrc`/`.env*` secret variables (names, `literal`/`op`/`ref`/`empty`, git-tracked, already in vega). `vega secrets adopt <file> <VAR>... --for <tool>` (admin host) moves them in: reuses a secret or 1Password item with the same value, else creates the item `<name>-api`; removes the lines, adds `use vega <tool>`. Then ask the Slayer to run `direnv allow` (a trust prompt; never run it yourself). A tracked file with a literal: report it, the key needs rotating.
- vega plugins (Rust): `vega_secrets::Store::from_env()?.get("<name>", "<tool>")` returns a value that prints as `***`. Hand it to programs via env, never argv.

Rules:
- Never print, echo or log a value, never pass one as an argument, never read `~/.config/vega/secrets/*.age` or `~/.config/vega/age/host.key`, never run `env` or `printenv` inside `vega secrets run`.
- `set`, `rekey`, `host add` and `op` are the Slayer's. When a secret is missing or undecryptable, report the hint from `vega secrets` and ask the Slayer; do not work around it.

New host: `vega secrets host init` prints its public key; on the admin host the Slayer runs `vega secrets host add <host> <age1...> --secret <name>`, commits and pushes the chezmoi source; then `chezmoi update` here.
