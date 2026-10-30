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
  dark|light|auto`) and migrates pi theme pairs into `[tools.pi]` (via toml_edit: comments and other keys stay).
- `$XDG_STATE_HOME/vega/theme.json`: the mode last applied, its source (`fixed`, `os`, `registry:<host>`) and
  time. State, not config.
- Due mode (`resolve`): a fixed mode wins; `auto` follows the OS; on a headless host it follows the control
  host's `<host>.theme` registry entry (fresh hosts only; inbox-only hosts: see vegad). Nothing known means
  nothing changes.
- Headless: `headless` in theme.toml, else no readable appearance (macOS: `defaults read -g
  AppleInterfaceStyle`; Linux: `gsettings … color-scheme` when `DISPLAY`/`WAYLAND_DISPLAY` is set).

## Adapters

Each reads and writes only its own key, writes only on a change (so `apply` is idempotent) and skips a tool
whose config file is missing (never creates one). JSON edits keep key order; files are replaced atomically,
through symlinks, with their mode kept.
Pi additionally creates its managed custom theme file and saves an existing pair in theme.toml;
its settings.json must already exist.

| Tool | Setting | Light / dark default | Native auto |
|---|---|---|---|
| claude | `~/.claude.json` `.theme` | `light` / `dark` (variants like `light-daltonized` count and stay) | `auto` exists but does not work (Slayer): no |
| codex | `~/.codex/config.toml` `[tui] theme` (syntax theme) | `catppuccin-latte` / `catppuccin-mocha` | no |
| pi | `~/.pi/agent/settings.json` `.theme = "vega-auto"`, `~/.pi/agent/themes/vega-auto.json` | `catppuccin-latte` / `catppuccin-mocha` | no; vega replaces the active custom theme file for live reload |
| delta | `~/.gitconfig.local` `delta.features` (swaps the theme feature, keeps others) | `mirthful-willow` / `weeping-willow` | no |
| openusage | `~/.config/openusage/settings.json` `.theme` | `Bluloco Light` / `Bluloco Dark` | no |
| gh-dash | `~/.config/gh-dash/config.yml` `theme.colors` ← `theme-<mode>.yml` (yq) | recognized by `text.primary` | no |
| herdr | `~/.config/herdr/config.toml` `[theme]` | `light_name` / `dark_name` | yes: `auto_switch = true` |

herdr's config is rendered by `vega config` (unit `herdr`) from `herdr/config.toml` ([config.md](config.md)) ([theme-symbols.md](theme-symbols.md)
"herdr's config"): the adapter is read-only and reports drift with the fix.
vega's own board keeps its fixed Catppuccin Macchiato palette (follow-up).

Pi’s default catppuccin-latte/catppuccin-mocha pair requires `pi-community-themes` to be installed.
If those themes are missing, install that package or set `[tools.pi] light/dark` to installed themes
(for example, the built-in `light`/`dark`). Built-ins are resolved from pi on PATH, standard fnm
node-versions directories, or common npm/Homebrew global prefixes, including for vegad’s minimal PATH.
A legacy `system/system` pair migrates to the defaults; explicit `[tools.pi]` overrides still win.

## vegad

- A thread reads theme.toml and the OS mode every 3 s and applies when the due mode differs from the one it
  applied last (`Watcher`). The first tick after a start applies too (idempotent), so a change while vegad was
  down is caught.
- The reconcile publishes `<host>.theme` = theme.json (no auth change: hosts own `<host>.>`), and hands the
  fresh hosts' modes to the thread for headless hosts.
- **Live Claude switch.** Claude Code reads `.theme` at start only; typing `/config theme=<mode>` switches a
  running pane. After a real change (from the mode this vegad applied, or at start from the recorded state;
  never on a restart with the same mode, never on the first apply on a host), the relay loop collects this
  host's Claude panes and types the line into each, once, when herdr reports it `idle` or `done`, the screen
  shows no dialog (`vega_herdr::dialog`), the input is empty (`delivery::draft`) and vegad holds no stashed
  draft there. A working pane waits for its next idle; a closed one is dropped; a newer change replaces the
  pending one. `live_claude = false` turns it off. The relay loop types both pointers and theme lines, so they
  never interleave.
- **Inbox-only hosts** (deimos) have no registry rights but one: `vega post auth-config ... --inbox-only deimos
  --registry-read deimos=proteus.theme` grants exactly
  `$JS.API.DIRECT.GET.KV_VEGA_REGISTRY.$KV.VEGA_REGISTRY.proteus.theme` (vega-lhc.90). Not even
  `STREAM.INFO.KV_VEGA_REGISTRY`, which async-nats sends to open the bucket: its `subjects_filter` would list
  every key. So `vega_registry::get_one` sends that one direct get as a plain request (`404` = nothing
  published). vegad there skips the reconcile and fetches `<control>.theme` every 15 s (the reconcile's
  cadence), feeding the thread as above; a failure is logged once per state change and retried every 120 s.
  `vega theme` reads the same key. Trade-off: without `<control>.beat` the entry's freshness is unknown, so an
  inbox-only host follows the last mode the control host published, also when its vegad is down.
- **Live pi switch.** The adapter copies the selected source theme into
  `~/.pi/agent/themes/vega-auto.json`, with only its `name` changed to `vega-auto`, then selects
  `vega-auto` in settings.json. An existing `light/dark` pair migrates to `[tools.pi] light/dark`
  without replacing explicit overrides. A single theme, `system`, or an unset theme uses the defaults
  above unless `[tools.pi]` overrides them. Sources are resolved from pi's installed built-in light/dark
  files (PATH first, then standard fnm and npm/Homebrew installations), the user's themes directory, and installed git/npm packages
  under `~/.pi/agent/git` and `~/.pi/agent/npm/node_modules`. Custom/package themes match the JSON
  `name`; duplicate package names use deterministic path order. This host-wide adapter does not resolve
  project-local packages or dynamically generated extension themes.
  Missing sources are reported and leave settings, preferences and the last working file alone.
  The file is replaced atomically only when its contents differ. Pi watches its custom themes directory
  and reloads the active file on replacement, so sessions already using `vega-auto` follow mode changes
  without a restart, including headless hosts following the control host. During deployment, switch
  existing sessions to `vega-auto` once with pi's theme picker; changing settings.json alone cannot
  change an already running session's selection.
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
