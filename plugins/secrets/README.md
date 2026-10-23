# vega secrets

Supplies API keys to tools, projects and agents without ever printing them. The originals stay in 1Password; each
host gets age ciphertext and decrypts only the secrets meant for it. A command or a direnv project gets just one
tool's secrets in its env, and `scan` finds keys lying in `.envrc` and `.env` files.

Commands: `vega secrets`, `run --for <tool>`, `scan`, `host` (`set`, `adopt` and `rekey` are the Slayer's).

Design: [secrets.md](../../docs/design/secrets.md).

Agent guide: `vega skills secrets`
