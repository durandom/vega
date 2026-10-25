# Theme: symbols

The `[symbols]` section of `~/.config/vega/theme.toml` (`$XDG_CONFIG_HOME/vega/theme.toml`): the symbols
of the UAC pane's label (vegad, [vegad.md](vegad.md) Badges) and of `vega board`. Read by
`vega_herdr::theme`, which `vega theme` reuses to list and edit them. The file's other sections
(dark/light and the like) belong to `vega theme`; this reader ignores them.

```toml
[symbols]
set = "nerd"          # or "emoji": the base table
uac_text = "UAC"      # optional: the word after the UAC symbol (e.g. "u▲c" with icons.uac = "")

[symbols.icons]       # optional, per topic, any string
notes = "✉"
pinned = "P"

[symbols.colors]      # optional, badge and agent kind topics, "#RRGGBB"
notes = "#2f7fd8"

[symbols.hosts]       # optional, host name -> label of its $host token, any string (e.g. "♃₃")
example = "E"

[symbols.host_colors] # optional, host name -> "#RRGGBB"
example = "#2f7fd8"
```

- A missing file or section means `set = "nerd"` without overrides.
- The config holds bare symbols; the renderer adds one space after each (a Nerd Font glyph needs
  the cell after it). The board measures each symbol, so an override may be any width.
- Mistakes are reported, the rest still applies: an unknown `set` (falls back to nerd), an unknown
  key under `[symbols]`, an unknown topic or a non-string under `[symbols.icons]`, a topic outside
  the nine colourable ones (four badges, five agent kinds; below) or a value other than strict
  `#RRGGBB` under `[symbols.colors]`, a non-string
  under `[symbols.hosts]`, a value other than strict `#RRGGBB` under `[symbols.host_colors]`, a
  file that is no TOML. vegad logs them once per change; the board shows the first in red in its top bar.
- Readers re-read the file at most every 2 s; vegad on its next label refresh. No restart.

## Topics

`vega_herdr::theme::TOPICS`, in this order. Nerd codepoints are checked against nerd-fonts'
`glyphnames.json` (md = Material Design, fa = Font Awesome). The emoji set is colourful throughout:
emoji-presentation code points, with U+FE0F where the base is text by default; each is two cells.

| Topic | Marks | nerd | emoji |
|---|---|---|---|
| `uac` | the UAC pane's marker, before UAC | md atom U+F0768 | ☢️ U+2622 U+FE0F |
| `notes` | badge: unread notifications | md email U+F01EE | 🔔 U+1F514 |
| `decisions` | badge: open slayer-decision beads | md scale_balance U+F05D1 | 🗳️ U+1F5F3 U+FE0F |
| `waiting` | badge: headed panes waiting on the user | fa hand_paper_o U+F256 | ✋ U+270B |
| `sentinel` | sidebar: prefix of a sentinel's name (vegad's display name of its children) | md cog U+F0493 | ⚙️ U+2699 |
| `claude` | sidebar: agent kind claude (`$kind`) | cod claude U+EC82 | ✳️ U+2733 U+FE0F |
| `codex` | sidebar: agent kind codex | cod openai U+EC81 | 🪢 U+1FAA2 |
| `copilot` | sidebar: agent kind copilot | cod copilot U+EC1E | ✈️ U+2708 U+FE0F |
| `pi` | sidebar: agent kind pi | md pi U+F03FF | 🥧 U+1F967 |
| `other_agent` | sidebar: any other agent kind | md robot U+F06A9 | 🤖 U+1F916 |
| `open` | bead status open | md circle_outline U+F0766 | 🔵 U+1F535 |
| `in_progress` | bead status in_progress | md circle_half_full U+F1396 | 🟡 U+1F7E1 |
| `blocked` | bead status blocked | md circle U+F0765 | 🔴 U+1F534 |
| `deferred` | bead status deferred | md snowflake U+F0717 | ❄️ U+2744 U+FE0F |
| `closed` | bead status closed | md check U+F012C | ✅ U+2705 |
| `pinned` | bead status pinned | md pin U+F0403 | 📌 U+1F4CC |
| `hooked` | bead status hooked | md hook U+F06E2 | 🪝 U+1FA9D |
| `other_status` | any other bead status | md circle_small U+F09DF | ⚪ U+26AA |
| `epic` | bead type epic | md rhombus U+F070B | 🔷 U+1F537 |
| `decision_bead` | bead type decision | md scale_balance U+F05D1 | 🗳️ U+1F5F3 U+FE0F |
| `blocked_by` | a bead blocked by another bead | md cancel U+F073A | ⛔ U+26D4 |
| `deps` | a bead's dependency count | md link_variant U+F0339 | 🔗 U+1F517 |

U+EC82 and U+EC81 came with Nerd Fonts 3.4; an older font (e.g. 3.3.0) draws a box. Update the font
(Symbols Nerd Font Mono >= 3.4 as a fallback font is enough), or override the two topics under
`[symbols.icons]` (e.g. `claude = "✳"`), or use `set = "emoji"`.

Other bead types keep their letter tags (B, F, C, S, Y, M; `·` for a task). Board chrome (collapse
arrows, list bullets, separators) is not themed.

## Colours and herdr sidebar tokens

herdr renders `display_agent` as plain text (ANSI SGR shows literally), so the UAC label cannot be
coloured there. herdr >= 0.9.0 styles custom sidebar tokens instead. On every label refresh vegad
reports, alongside `display_agent` and with the same source (`vega:vegad`) and TTL, one token per
topic on the UAC pane (`Badges::uac_tokens`):

| Token | Value | Default colour |
|---|---|---|
| `$uac` | `<uac symbol> UAC`, always | `#b56d0c` |
| `$notes` | `<notes symbol> <count>`, cleared at zero | `#2f7fd8` |
| `$decisions` | `<decisions symbol> <count>`, cleared at zero | `#9a5cf0` |
| `$waiting` | `<waiting symbol> <count>`, cleared at zero | `#d9465f` |

`[symbols.colors]` overrides the colours of these four topics. The defaults keep a contrast of at
least 3:1 on the Catppuccin Latte and Macchiato base and mantle backgrounds. `vega board` draws its
top-bar badges in the same colours. herdr keeps at most 80 characters of a token value.

vegad does not write herdr's config. Colours live in herdr's `[ui.sidebar.agents]` rows, which
`vega config` (unit `herdr`) renders from `[symbols.colors]` (see "herdr's config" below); the snippets in this
document show the shape. This snippet, with the default
colours, keeps herdr's default first row and replaces its second (`["agent"]`): on the UAC pane the
plain `agent` label (it contains the `uac_text`, here `UAC`; match yours) hides and the coloured tokens show; on every other pane the
tokens are unreported and vanish, so `agent` shows as before.

```toml
[ui.sidebar.agents]
rows = [
  ["state_icon", "machine", "workspace", "tab"],
  [
    { token = "agent", rules = [{ contains = "UAC", hide = true }] },
    { token = "$uac", fg = "#b56d0c", bold = true },
    { token = "$notes", fg = "#2f7fd8", bold = true },
    { token = "$decisions", fg = "#9a5cf0", bold = true },
    { token = "$waiting", fg = "#d9465f", bold = true },
  ],
]
```

`herdr config check` accepts it (herdr 0.9.3). With a `rows_by_agent.claude` override the second row
goes there instead, since an override replaces `rows`. Without the snippet `display_agent` shows the
uncoloured label as before. Board panes that carry the badges when no UAC pane is live keep the
plain label.

herdr colours its agent status dots from the theme palette: `yellow` working, `teal` done, `green`
idle, `red` blocked. Catppuccin's pastel yellow and teal read alike at dot size, so
`config/herdr/config.toml` overrides both under `[theme.custom.light]` and `[theme.custom.dark]`:
done turns blue (Latte `#1e66f5`, Mocha `#89b4fa`, as on herdr's mobile view) and working a stronger
amber (`#df8e1d`, `#ffb224`). herdr uses `teal` only for done; `yellow` also colours its
connecting/reconnecting and update-available notices.

## Host identity

herdr's combined multi-machine sidebar marks remote panes with its `machine` token, which can be
coloured but not given an icon. vegad therefore reports a `$host` token on every agent pane of its
own herdr, with the same source and TTL as the other tokens: the host's `[symbols.hosts]` label as
written (e.g. `♃₃` for ganymede, Jupiter III; write `<icon> ganymede` to keep the name), the bare host
name without an entry (or with `""`). The host name is
vegad's post host (`VEGA_POST_HOST`, else the hostname; lowercase, `.` as `_`); keys match it
case-insensitively. Each host labels its own panes: a remote pane's `$host` comes from the remote
vegad, so every host's theme.toml should carry the same `[symbols.hosts]` table.

Colours, as for the badges, live in herdr's rows: `vega config` (unit `herdr`) renders one rule per
`[symbols.host_colors]` entry, matching its `[symbols.hosts]` label. This snippet replaces `machine` in the first row by
`$host` (one `fg` rule per host, by `equals` on the label) and
keeps the UAC tokens row from above:

```toml
[ui.sidebar.agents]
rows = [
  [
    "state_icon",
    { token = "$host", fg = "#8a8a8a", rules = [
      { equals = "♃₃", fg = "#a08c6e" },
      { equals = "♂₂", fg = "#c0603a" },
    ] },
    "workspace",
    "tab",
  ],
  [
    { token = "agent", rules = [{ contains = "UAC", hide = true }] },
    { token = "$uac", fg = "#b56d0c", bold = true },
    { token = "$notes", fg = "#2f7fd8", bold = true },
    { token = "$decisions", fg = "#9a5cf0", bold = true },
    { token = "$waiting", fg = "#d9465f", bold = true },
  ],
]
```

`herdr config check` accepts it (herdr 0.9.3). A pane no vegad labels (vegad down, or a host
without vega) has no `$host`; keep `machine` next to it if such panes should still show theirs.

## Sidebar label

herdr rules cannot hide one token based on another, so vegad reports the second row's "what it is"
as two tokens on every agent pane of its own herdr (same source and TTL):

- `$kind`: the symbol of the pane's agent kind (herdr's `agent` field) by its topic, `claude`,
  `codex`, `copilot`, `pi`, else `other_agent`; on every agent pane, UAC and sentinels included.
  Cleared on a pane without a kind and for an empty override. A symbol that ends in a private-use
  glyph (every nerd one) gets a U+2800 braille blank after it (`theme::CELL_BLANK`): Ghostty draws a
  Nerd Font icon two cells wide when the cell after it is whitespace, as in herdr's ` · ` separator,
  and herdr trims whitespace (NBSP, thin and hair space included) from token values. U+2800 is no
  whitespace, so the glyph keeps one cell, and the separator lines up with the two-cell host label
  above it (checked in Ghostty 1.3.1 with herdr 0.9.5, vega-lhc.132). Each kind has its colour,
  overridable under `[symbols.colors]`: `claude` `#c96442` (terracotta), `codex` `#0e8f6f` (green),
  `copilot` `#8957e5` (purple), `pi` `#1f8fb0` (blue), `other_agent` `#808080` (grey); the same 3:1
  contrast as the badge colours. herdr matches the token's exact value, blank included.
- `$label`: on a sentinel `<sentinel symbol> <tab label>` (its name when the tab has no label), on any
  other agent pane its tab label (the kind's name without one, e.g. `pi`), so several agents in one
  workspace stay apart; on the UAC pane vegad clears it and the `$uac` tokens show instead.

Every agent row thus starts with its kind: `<claude> u▲c ✉ 3`, `<claude> ⚙ readable`, `<pi> nba-loop`. Tab labels
come from one `herdr tab list` per refresh. `display_agent` is reported as before, for setups
without these rows.

The first row's project is vegad's `$project` token, not herdr's `workspace`: a sentinel spawned
with `--worktree` lives in its own workspace (e.g. `sentinel/theseus-23-1`), so vegad reports the
label of its parent pane's workspace when the parent runs on this host; when the parent is remote or
gone, and on every other agent pane, the pane's own workspace label. Workspace labels come from the
same `herdr workspace list` per refresh.

This snippet puts host and project in the first row and the label or the UAC badges in the second
(no `agent`, no `tab`: `$label` carries the tab). Add the `$host` colour rules from above as needed:

```toml
[ui.sidebar.agents]
rows = [
  ["state_icon", { token = "$host", fg = "#8a8a8a" }, "$project",
   { token = "$git", fg = "#f07830", rules = [{ equals = "✓", fg = "#45922a" }] }],
  [
    { token = "$kind", fg = "#808080", rules = [
      { equals = "\U0000EC82\U00002800", fg = "#c96442" },
      { equals = "\U0000EC81\U00002800", fg = "#0e8f6f" },
      { equals = "\U0000EC1E\U00002800", fg = "#8957e5" },
      { equals = "\U000F03FF\U00002800", fg = "#1f8fb0" },
    ] },
    "$label",
    { token = "$uac", fg = "#b56d0c", bold = true },
    { token = "$notes", fg = "#2f7fd8", bold = true },
    { token = "$decisions", fg = "#9a5cf0", bold = true },
    { token = "$waiting", fg = "#d9465f", bold = true },
  ],
]
```

`$git` is the git state of the pane's cwd, as for workspaces (below): a sentinel shows its own
worktree, a remote pane its own vegad's answer; cleared outside a checkout.

Workspace rows (Spaces) carry the distinct kinds in that workspace (below).

A pane no vegad labels has neither `$host`, `$project`, `$kind` nor `$label`; keep `machine`, `workspace`
and `agent` in the rows if such panes should still show something. That is a host without vega, a
stopped vegad, or another herdr session on the host (vegad reports to the default one only, e.g. not
ganymede's `tops`). A vegad in identity quiesce (vegad.md, Fallback) still reports `$host`, `$kind`,
`$project` (the pane's own workspace label) and `$label` (tab label, else the kind; no sentinel symbol,
not cleared on the UAC); the other tokens wait for the identity.

## Workspace inbox (herdr Spaces)

herdr's Spaces panel lists workspaces. On every label refresh vegad reports the workspace tokens
`$decisions`, `$waiting` and `$notes` (`herdr workspace report-metadata`, same source and TTL, the
same values as on the UAC pane, a zero one cleared; `Badges::count_tokens`) on each workspace of a
project with a UAC record: the project's repo workspace, or one holding a pane that works on it (as
`vega uac` decides it; the first project wins). Linked worktrees (`is_linked_worktree`, the
sentinels' workspaces) get none, so the counts show once, on the main checkout.

The same call sets `$badge`: what works in the workspace, `u▲c` (`uac_name`) when it holds a UAC
pane, else the `sentinel` symbol when it holds a local sentinel; cleared otherwise.

`$kind1` through `$kind5` carry its distinct harness glyphs from `Symbols::space_kind`.
Base-set icons become compact Unicode text: Claude `✳`, Codex `֍`, Copilot `✈`, Pi `π`,
and other agents `◇`. Custom icon overrides still apply, including empty overrides.
Nerd glyphs expand in Ghostty without a following cell blank; adding that blank creates
excess spacing in the spaces row. These text glyphs need neither. `space_kind_rules`
uses their exact values with the same colours as the agents' `$kind`.
Missing or cleared slots add no spacing.
The UAC's kind comes first, else a sentinel's; remaining kinds follow pane ID order. Multiple
panes of one kind contribute once; unknown harnesses share `other_agent`. Empty symbol overrides
are omitted. All five slots are reported or explicitly cleared on every workspace refresh,
including workspaces without a checkout or agents, so exited agents leave no stale glyphs.

Separate slots let each glyph retain its colour: herdr styles a whole token, not substrings of a
combined `$kinds` value. [Herdr 0.9.5's configuration reference](https://raw.githubusercontent.com/herdrdev/herdr/v0.9.5/docs/next/website/src/content/docs/configuration.mdx)
documents custom workspace tokens and per-token rules; `herdr workspace report-metadata --help`
confirms `--token`, `--clear-token` and `--ttl-ms`. Each host's vegad reads its own herdr layout and
reports its own workspace tokens. Herdr carries them to its combined machines/spaces sidebar over
the remote connection, just like `$badge` and `$git`; the vega registry needs no new fields.

And `$git`, on every workspace with a checkout (`worktree.checkout_path`): `✓` when it is clean and
level with its upstream (or has none), else `↑<ahead>`, `↓<behind>` and `●` for local changes,
untracked files included. It replaces herdr's `git_status`, which shows only ahead/behind, is empty
when level and takes no rules. vegad runs `git --no-optional-locks status --porcelain=v2 --branch`
per checkout on each refresh (never `index.lock`, never a fetch: `↓` is as fresh as the last fetch,
as in herdr).

```toml
[ui.sidebar.spaces]
rows = [
  ["state_icon",
   { token = "$badge", fg = "#e5484d", bold = true, rules = [
     { starts_with = "\U000F0493", fg = "#45922a" }] },
   # Repeat this styled entry for $kind2 through $kind5. Use text glyph rules
   # (vega config fills them from theme.toml).
   { token = "$kind1", fg = "#808080", rules = [
      { equals = "✳", fg = "#c96442" },
      { equals = "֍", fg = "#0e8f6f" },
      { equals = "✈", fg = "#8957e5" },
      { equals = "π", fg = "#1f8fb0" },
   ] },
   "workspace",
   { token = "$git", fg = "#f07830", rules = [{ equals = "✓", fg = "#45922a" }] }],
  [{ token = "branch", rules = [{ equals = "main", hide = true }] }],
  [
    { token = "$decisions", fg = "#9d6bf0", bold = true },
    { token = "$waiting", fg = "#f07830", bold = true },
    { token = "$notes", fg = "#e0a020", bold = true },
  ],
]
```

The colours match the agents rows; hiding `main` leaves the branch only where it differs.

## herdr's config

vega owns `~/.config/herdr/config.toml` (vega-lhc.103, Slayer decision 2026-10-13; before, a chezmoi
`modify_` script in the dotfiles). The source is `herdr/config.toml` ([config.md](config.md) "Sources":
`~/.config/vega/config/herdr/config.toml` when it exists, else the repo's `config/herdr/config.toml`):
herdr's whole config (theme, keys, toasts, sidebar) with double-brace placeholders, rendered by
`vega_config::herdr` (plugins/config):

| Placeholder | From |
|---|---|
| `color.uac`, `color.notes`, `color.decisions`, `color.waiting`, `color.other_agent` | `[symbols.colors]` (defaults above) |
| `host_rules` | one `{ equals = <label>, fg = <colour> }` per `[symbols.host_colors]` host, label from `[symbols.hosts]` |
| `kind_rules` | one `{ equals = <$kind value>, fg = <colour> }` per kind `claude`, `codex`, `copilot`, `pi` (`[symbols.colors]`); none for an empty icon |
| `icon.sentinel` | the `sentinel` symbol (nerd cog when empty), private-use glyphs as `\U` escapes |
| `color.host`, `color.sentinel`, `color.git_clean`, `color.git_dirty` | constants in `vega_config::herdr` (`#8a8a8a`, `#45922a`, `#45922a`, `#f07830`) |

An unknown placeholder or a rendered file that is no TOML fails the render. Keys herdr writes itself
(`HERDR_OWNED`: `[ui] agent_panel_sort`, rewritten when the sort is toggled in the UI) keep their
live value (lowercase letters and `_` only; else the template's default, `priority`).

- `vega config diff herdr`, `vega config print herdr`: the diff, the rendered file; read-only.
- `vega config apply herdr`: refused while chezmoi manages the file (`managed`). Nothing when the file
  matches. Else writes a candidate next to the live file and runs
  `HERDR_CONFIG_PATH=<candidate> herdr config check`; on a rejection the old file stays and the issues
  are listed (exit 1). On a pass: the old file goes to `config.toml.vega-bak`, the new one is written
  atomically, then `herdr server reload-config` (a failure, e.g. no server, is reported; herdr reads the
  file on its next start). Without herdr: `skipped`, nothing written.
- `vega theme herdr [--print|--apply]` is an alias for these; `vega deploy apply` runs `vega config
  apply` on each target (column `config`; never fails the deploy).

Each host renders from its own theme.toml, so every host's `[symbols]` should match.
