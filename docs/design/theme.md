# vega theme: one dark/light mode for every TUI

Status: built, 2026-10-12. Bead: vega-lhc.86 (epic vega-lhc). Replaces dotfiles `bin/theme-sync`.

## Problem

The OS switches between dark and light; Ghostty follows by itself (`theme = "dark:…,light:…"`), the TUIs
inside it do not. dotfiles `bin/theme-sync` (bash, jq, yq) set Claude Code, delta, pi, openusage and gh-dash,
but only WezTerm's appearance hook called it; since the move to Ghostty nothing does. Headless hosts (io-wsl)
have no appearance of their own.

## Shape

| Part | Where |
|---|---|
| Preferences, state, OS detection, adapters, watcher logic | `crates/vega-theme-sync` |
| CLI `vega theme` | `plugins/theme` |
| Watcher thread, registry entry, live Claude switch | `plugins/post/src/theme.rs` (vegad) |
| Registry key `<host>.theme` | `crates/vega-registry` |

- `~/.config/vega/theme.toml` (`$XDG_CONFIG_HOME`): what the user wants. `mode = "auto"|"dark"|"light"`
  (default auto), `control` (default `proteus`), `headless` (default: detected), `live_claude` (default true),
  `[tools.<tool>] light/dark/enabled` for theme names and opt-out. vega writes `mode` (`vega theme
  dark|light|auto`) via toml_edit; comments and other keys stay.
- `$XDG_STATE_HOME/vega/theme.json`: the mode last applied, its source (`fixed`, `os`, `registry:<host>`) and
  time. State, not config. `theme-request.json` records a live Claude change before tool
  application; `theme-live.json` records its recipients, last successfully sent mode, pending flag,
  error, attempt count and retry time. Watcher and relay own separate files to avoid lost updates.
  `vega theme` shows these per-pane receipts (unknown before the first send); they record successful
  transport delivery, not an acknowledgement from Claude. The registry entry includes the journal as `claude`.
- Due mode (`resolve`): a fixed mode wins; `auto` follows the OS; on a headless host it follows the control
  host's `<host>.theme` registry entry (fresh hosts only; inbox-only hosts: see vegad). Nothing known means
  nothing changes.
- Headless: `headless` in theme.toml, else no readable appearance (macOS: `defaults read -g
  AppleInterfaceStyle`; Linux: `gsettings … color-scheme` when `DISPLAY`/`WAYLAND_DISPLAY` is set).

## Adapters

Each reads and writes only its own key, writes only on a change (so `apply` is idempotent) and skips a tool
whose config file is missing (never creates one). JSON edits keep key order; files are replaced atomically,
through symlinks, with their mode kept.
For fixed themes pi additionally creates its managed custom theme file;
its settings.json must already exist.

| Tool | Setting | Light / dark default | Native auto |
|---|---|---|---|
| claude | `~/.claude.json` `.theme` | `light` / `dark` (variants like `light-daltonized` count and stay) | `auto` exists but does not work (Slayer): no |
| codex | `~/.codex/config.toml` `[tui] theme` (syntax theme) | `catppuccin-latte` / `catppuccin-mocha` | no |
| pi | `~/.pi/agent/settings.json` `.theme = "vega-auto"`, `~/.pi/agent/themes/vega-auto.json` | `catppuccin-latte` / `catppuccin-mocha` | yes: `system` (also unset), or a `light/dark` pair; fixed themes use vega-auto |
| delta | `~/.gitconfig.local` `delta.features` (swaps the theme feature, keeps others) | `mirthful-willow` / `weeping-willow` | no |
| openusage | `~/.config/openusage/settings.json` `.theme` | `Bluloco Light` / `Bluloco Dark` | no |
| gh-dash | `~/.config/gh-dash/config.yml` `theme.colors` ← `theme-<mode>.yml` (yq) | recognized by `text.primary` | no |
| herdr | `~/.config/herdr/config.toml` `[theme]` | `light_name` / `dark_name` | yes: `auto_switch = true` |

Legacy herdr config is host-managed; the adapter is read-only and reports drift
with the native `auto_switch` setting as the fix. The TUI config unit was retired
in vega-0h4c.4. Herdr-app themes are managed by [config.md](config.md).

Pi’s default catppuccin-latte/catppuccin-mocha pair requires `pi-community-themes` to be installed.
If those themes are missing, install that package or set `[tools.pi] light/dark` to installed themes
(for example, the built-in `light`/`dark`). Built-ins are resolved from pi on PATH, standard fnm
node-versions directories, or common npm/Homebrew global prefixes, including for vegad’s minimal PATH.
Native `system` and pairs (including `system/system`) are preserved, even with `[tools.pi]` names configured.

## vegad

- A thread reads theme.toml and the OS mode every 3 s and applies when the due mode differs from the one it
  applied last (`Watcher`). The first tick after a start applies too (idempotent), so a change while vegad was
  down is caught.
- The reconcile publishes `<host>.theme` = theme.json (no auth change: hosts own `<host>.>`), and hands the
  fresh hosts' modes to the thread for headless hosts.
- **Live Claude switch.** Claude Code reads `.theme` at start only; typing `/config theme=<mode>` switches a
  running pane. After a real change (from the mode this vegad applied, or at start from the recorded state;
  never on the first apply on a host; a restart resumes previously requested work), the relay loop collects this
  host's Claude panes and types the line into each, once, at once when herdr reports it `idle`, `done` or
  `working` (Claude runs a slash command typed during a turn; it is sent queued), the screen shows no dialog
  (`vega_herdr::dialog`), the input is empty (`delivery::draft`) and vegad holds no stashed draft there. A
  blocked or unknown pane waits; a closed one is dropped; a newer change replaces the
  pending one. A send failure stays pending with its error and exponential backoff
  (4 s initially, capped at 120 s), retried on the 2 s relay tick. With no pending work,
  pane status refreshes on the 15 s registry cadence. The durable request survives a restart
  between tool application and event delivery. Successful backend inventories collect recipients once
  per change and remove closed/non-Claude panes; unavailable backends leave their work intact.
  `live_claude = false` pauses live delivery. The relay loop types both pointers and theme lines, so they
  never interleave.
- **Inbox-only host** (puck) has no registry rights but one: `vega post auth-config ... --inbox-only puck
  --registry-read puck=proteus.theme` grants exactly
  `$JS.API.DIRECT.GET.KV_VEGA_REGISTRY.$KV.VEGA_REGISTRY.proteus.theme` (vega-lhc.90). Not even
  `STREAM.INFO.KV_VEGA_REGISTRY`, which async-nats sends to open the bucket: its `subjects_filter` would list
  every key. So `vega_registry::get_one` sends that one direct get as a plain request (`404` = nothing
  published). vegad there skips the reconcile and fetches `<control>.theme` every 15 s (the reconcile's
  cadence), feeding the thread as above; a failure is logged once per state change and retried every 120 s.
  `vega theme` reads the same key. Trade-off: without `<control>.beat` the entry's freshness is unknown, so an
  inbox-only host follows the last mode the control host published, also when its vegad is down.
- **Native pi switch.** Pi's `system` theme is generated from terminal colors (no source JSON),
  and `light/dark` theme pairs select from terminal appearance. An unset theme defaults to `system`.
  The adapter reports `native` and leaves settings, preferences and managed files alone for these.
  This follows terminal appearance, including on headless hosts; fixed vega mode does not override it.
- **Managed pi switch.** For a fixed theme or an existing `vega-auto` selection, the adapter copies
  the selected source into `~/.pi/agent/themes/vega-auto.json`, changing only its name, and selects
  `vega-auto`. `[tools.pi]` names override the default catppuccin pair. Sources come from built-in
  files, user themes, and installed git/npm packages. Missing sources preserve the last working file
  and settings. Atomic replacement of the active custom theme triggers pi's live reload; existing
  sessions must select `vega-auto` once. Project-local and generated extension themes are not resolved.
- Codex: `/theme` is an interactive picker, no argument form; no live switch.

## Risks

- `~/.claude.json` is rewritten by Claude Code all the time; a read-modify-write can lose a write that lands in
  between. It happens only on a mode change (writes are skipped when unchanged), as with theme-sync.
- The live line goes into an agent pane: guarded as above, but a screen vegad misreads could still take it.

## Dotfiles follow-up (dotfiles UAC)

Only after `vega theme` is installed and vegad runs the new binary on proteus:

1. Delete `bin/executable_theme-sync` and its `.chezmoiignore` line (`!bin/theme-sync`).
2. `dot_wezterm.lua` (around line 701): remove the appearance hook's `~/bin/theme-sync` call.
3. `dot_config/herdr/modify_config.toml`: `[theme] auto_switch = true` (keep `light_name`/`dark_name`; pick the
   pair to match Ghostty, e.g. `catppuccin-latte`/`catppuccin-mocha`).
4. Comments that name theme-sync: `dot_config/gh-dash/theme-{light,dark}.yml` (now `vega theme`),
   `dot_gitconfig.tmpl:94` (`delta.features` in `~/.gitconfig.local` is set by `vega theme`),
   `dot_config/ghostty/config:8`.
