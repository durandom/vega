---
name: vega-upgrade
description: Upgrades vega on this host in one command. Pulls the vega checkout it was installed from (fast-forward only), installs it, upgrades and checks moshi-hook where installed, and shows the changelog entries since the last look. Use when vegad mails that a vega upgrade is available, or when the user asks to upgrade vega.
---

# vega upgrade: pull, install, show the changes

```
vega upgrade     # refuses a dirty or diverged checkout and vega deploy targets
```

- Steps: `git pull --ff-only` in the checkout recorded at install time, `mise run install-cli` when the
  installed commit is not HEAD (output in `$XDG_STATE_HOME/vega/upgrade.log`), moshi-hook where installed
  (`brew upgrade`, `brew services restart`, `moshi-hook doctor`), then `vega onboard` of the new binary.
- Afterwards: summarise the changes for the user, act on `migration` entries, run `vega onboard apply` on
  their OK. `vega upgrade` never applies config itself.
- vegad proposes it once per new upstream state, to one agent pane in the vega checkout (the UAC first).
- Refusals: `dirty` (commit or stash first), `diverged` (rebase or merge yourself), `deploy_target`
  (upgrade the deploying host, then `vega deploy apply`), `no_install_record` (`mise run install-cli`
  once in the checkout).
