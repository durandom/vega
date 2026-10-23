# vega onboard

Brings a host up to vega and tells it what changed after an upgrade. It lists the required and optional infra with
the next step per gap, applies the default agent config without replacing the user's own files, and shows the
changelog entries since this host last looked.

Recommends Destructive Command Guard (`dcg`), checking its executable and the
Claude/Codex PreToolUse hook registrations with install/repair guidance. Onboard
does not install the binary or register dcg hooks. `apply` writes missing
`dcg.config` and `dcg.packs` defaults and keeps existing differing files.

Onboard treats NATS and Dolt as configured only when their endpoint environment
variable or local file is set; a Tailscale installation alone is insufficient.
See [Service endpoints](../../docs/design/config.md#service-endpoints) before
upgrading existing hosts.

Commands: `vega onboard`, `vega onboard apply`, `vega onboard changes`.

Design: [onboarding.md](../../docs/design/onboarding.md), [upgrade.md](../../docs/design/upgrade.md).

Agent guide: `vega skills onboard`
