# tea shim

Passes every `tea` (Forgejo/Gitea CLI) call through to the real binary; nothing is denied, as no polling loop is known
for tea yet.

- **Hint:** the third `tea pulls` or `tea api` within 10 minutes gets a one-time hint (per session) at `vega ci watch`.
- **Counts:** calls per pane, shown by `vega uac status`.
- **Install hint:** when tea is missing, says how to install it (`brew install tea`, then `tea login add`).

Design: [ci-watch.md](../../docs/design/ci-watch.md) (tea shim).

Agent guide: `vega skills ci`
