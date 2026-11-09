// Agents in grouped or priority views; parents take their most urgent sub-agent's place.
// A viewed done agent stays in Done until left.

import { View, div, image, svg } from "gpui-kit";
import { v_flex, h_flex, InputState, Input } from "gpui-base";
import { agents, terminals, panes, focused, spaces, run, theme, shared, setShared, setAgentOrder, openOverlay, closeOverlay, props, window, layout, exec, home } from "herdr";

function find(node, id) {
  if (node?.id === id) return node;
  for (const child of node?.children ?? []) {
    const found = find(child, id);
    if (found) return found;
  }
  return null;
}

// The sidebar's rail goes by its width, not a flag: dragging its edge narrows it too.
const narrow = () => (find(layout(), "sidebar")?.size?.px ?? Infinity) < 160;

// To the rail (on macOS wide enough for the traffic lights), or back to the width it had.
function toggleSidebar() {
  const width = find(layout(), "sidebar")?.size?.px;
  if (!narrow()) setShared("sidebar.width", width);
  // Mods edited before the rail went by width still read the flag.
  setShared("sidebar.collapsed", !narrow());
  run("layout.resize", { id: "sidebar", px: narrow() ? (shared("sidebar.width") ?? 268) : window()?.platform === "macos" ? 76 : 44 });
}

// vega's default symbol colours (crates/vega-herdr/src/theme.rs COLORS) plus git
// (plugins/config/src/herdr.rs GIT_CLEAN_COLOR / GIT_DIRTY_COLOR).
const DEFAULT_COLORS = {
  uac: "#b56d0c", notes: "#2f7fd8", decisions: "#9a5cf0", waiting: "#d9465f",
  claude: "#c96442", codex: "#0e8f6f", copilot: "#8957e5", pi: "#1f8fb0", other_agent: "#808080",
  git_clean: "#45922a", git_dirty: "#f07830",
};

// [symbols.colors], [symbols.hosts], [symbols.host_colors] of theme.toml: flat `key = "value"` lines.
function parseSymbols(text) {
  const out = {};
  let section = null;
  for (const line of text.split("\n")) {
    const head = line.match(/^\s*\[symbols\.(colors|hosts|host_colors)\]\s*$/);
    if (head) { section = head[1]; out[section] = {}; continue; }
    if (/^\s*\[/.test(line)) { section = null; continue; }
    const kv = section && line.match(/^\s*([\w-]+)\s*=\s*"([^"]*)"/);
    if (kv) out[section][kv[1]] = kv[2];
  }
  return out;
}

// A machine's $host label: the one vegad wrote on any of its terminals.
const labelOfMachine = (machine, live) => live.find(x => x.address.startsWith(`${machine}/`) && x.labels?.["vega.host"])?.labels["vega.host"] ?? machine;

const clean = (s) => (s ?? "").replace(/^[^\p{L}\p{N}]+/u, "").trim();

function leadPanes(all) {
  const leads = new Map();
  for (const p of all) {
    if (!p.terminal) continue;
    const current = leads.get(p.tab);
    // First agent, else first focused terminal, else first terminal in the tab.
    if (!current || (!current.program?.agent && (p.program?.agent || (!current.focused && p.focused)))) leads.set(p.tab, p);
  }
  return leads;
}

function title(tab, l) {
  const prog = l?.program?.name;
  const live = clean(l?.title);
  const telling = live && live !== prog && !/^[~/]|^[A-Za-z]:\\/.test(live) && !live.includes("@") && live.toLowerCase() !== l?.program?.key;
  return (tab.name === prog || !tab.named) && telling ? live : tab.name;
}
const RANK = { blocked: 4, done: 3, working: 2, idle: 1 };
// A sub-agent without a pane isn't in agents(): its raw word, error counting as
// blocked. It's never done: it reports to its parent, and the parent's done is the news.
const kidStatus = (k, listed) => {
  const s = listed.get(k.address) ?? k.status;
  return s === "error" ? "blocked" : s === "done" ? "idle" : s;
};
const SORTS = [
  // The first is the default: UAC projects across machines, then spaces (projectTree).
  { id: "project", label: "UAC / spaces", icon: "layers" },
  { id: "priority", label: "Priority", icon: "clock" },
  { id: "grouped", label: "Grouped by space", icon: "layers" },
];
const sortOf = () => SORTS.find((s) => s.id === shared("vega-sidebar.view")) ?? SORTS[0];
const SECTION = { 4: "Needs you", 3: "Done", 2: "Working", 1: "Idle" };
const SECTION_STATUS = { 4: "blocked", 3: "done", 2: "working" };

function duration(ms) {
  const m = Math.floor(ms / 60000);
  if (m < 1) return "<1m";
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  return h < 24 ? `${h}h` : `${Math.floor(h / 24)}d`;
}

// The agent's icon from the app's program table: a one-color glyph or a picture.
function icon(p, size, color) {
  if (!p?.icon) return null;
  return p.tint ? svg(p.icon).size(size).flex_none().text_color(color) : image(p.icon).w(size).h(size).flex_none();
}

export default class Agents extends View {
  init(_props, cx) {
    if (props()?.mail) {
      this.mail = "Loading mail…";
      cx.spawn(async (cx) => {
        try {
          const target = props().mail;
          const slash = target.indexOf("/");
          const on = slash < 0 ? undefined : target.slice(0, slash);
          const pane = slash < 0 ? target : target.slice(slash + 1);
          const args = props().notes ? ["post", "notes", "--json"] : ["post", "log", "--pane", pane, "--full", "--json"];
          const r = await exec("vega", args, {on});
          if (r.code !== 0) throw new Error(r.stderr || r.stdout || "Mail unavailable");
          this.mail = props().notes
            ? r.stdout.split("\n").filter(s => s.trim()).map(s => JSON.parse(s))
            : JSON.parse(r.stdout).messages;
        } catch (e) { this.mail = String(e); }
        cx.notify();
      });
      return;
    }
    if (props()?.sortMenu || props()?.spaceMenu) return;
    this.folded = new Set();
    // Each agent's last state and when it changed (ms), kept across restarts.
    this.changed = JSON.parse(localStorage.getItem("changed") ?? "{}");
    this.pendingPins = JSON.parse(localStorage.getItem("pins") ?? "[]");
    if (!this.pendingPins.length) localStorage.removeItem("pins");
    this.migratingPins = new Set();
    cx.timer.every(30000, (cx) => cx.notify());
    try { this.order = JSON.parse(localStorage.getItem("project-order") ?? "[]"); } catch (_) { this.order = []; }
    cx.spawn(async (cx) => {
      try {
        const r = await exec("cat", [`${home()}/.config/vega/theme.toml`]);
        if (r.code === 0) { this.symbols = parseSymbols(r.stdout); cx.notify(); }
      } catch (_) {}
    });
  }

  // Stamps agents whose state changed. Done and idle count as one: looking at a
  // finished agent turns done into idle, and looking isn't the agent doing anything.
  track(list, now) {
    let dirty = false;
    for (const a of list) {
      const s = a.status === "done" ? "idle" : a.status;
      if (this.changed[a.address]?.s === s) continue;
      this.changed[a.address] = { s, at: now };
      dirty = true;
    }
    if (!dirty) return;
    // Gone and old: the list alone is empty right after a start.
    const listed = new Set(list.map((a) => a.address));
    for (const [k, c] of Object.entries(this.changed)) if (!listed.has(k) && now - c.at > 30 * 86400000) delete this.changed[k];
    localStorage.setItem("changed", JSON.stringify(this.changed));
  }

  // Keep offline pins until their terminal returns, and keep each pending until
  // core echoes the label so a failed write doesn't erase the old pin.
  migratePins(all) {
    const listed = new Map(all.filter((t) => t.state === "live").map((t) => [t.address, t]));
    const pending = this.pendingPins.filter((address) => {
      const terminal = listed.get(address);
      if (!terminal) return true;
      if (terminal.labels?.["herdr.pinned"]) return false;
      if (!this.migratingPins.has(address)) {
        this.migratingPins.add(address);
        run("terminal.pin", { terminal: address, pinned: true });
      }
      return true;
    });
    if (pending.length === this.pendingPins.length) return;
    this.pendingPins = pending;
    if (pending.length) localStorage.setItem("pins", JSON.stringify(pending));
    else localStorage.removeItem("pins");
  }

  since(a) {
    return this.changed[a.address]?.at ?? 0;
  }

  // Looking at a done agent makes it idle; it stays with Done until you leave
  // it rather than drop away under you. Any other change moves it at once.
  hold(list) {
    const cur = list.find((a) => a.focused);
    const same = cur && cur.address === this.held?.address;
    // Arriving, or any change but done turning idle, sets what it was.
    if (!same || cur.status !== "idle") this.held = cur ? { address: cur.address, rank: RANK[cur.status] ?? 0 } : null;
  }

  rank(a) {
    const sawDone = a.address === this.held?.address && this.held.rank === RANK.done && a.status === "idle";
    return sawDone ? RANK.done : RANK[a.status] ?? 0;
  }

  sortMenu(t) {
    const on = sortOf().id;
    return v_flex()
      .w_full()
      .p(5)
      .bg(t.popover)
      .text_size(12.8)
      .child(div().px(8).pt(4).pb(6).text_size(11).text_color(t.dim).child("Sort agents"))
      .children(SORTS.map((s) => h_flex()
        .id(`sort-${s.id}`)
        .h(31)
        .px(8)
        .gap(9)
        .rounded(7)
        .items_center()
        .text_color(t.foreground)
        .hover((el) => el.bg(t.hover))
        .on_click(() => {
          setShared("vega-sidebar.view", s.id);
          closeOverlay(props().overlay);
        })
        .child(svg(`mods/workspace-chrome/${s.icon}.svg`).size(14).flex_none().text_color(t.muted))
        .child(div().flex_1().child(s.label))
        .when(s.id === on, (el) => el.child(svg("mods/workspace-chrome/check.svg").size(14).flex_none().text_color(t.foreground)))));
  }

  openSpaceMenu(space, cx) {
    cx.stop_propagation();
    openOverlay({ mod: "vega-sidebar", kind: "popup", width: 260, height: 100,
      props: { spaceMenu: { id: space.id, name: space.name } } });
  }

  spaceMenu(t) {
    const space = props().spaceMenu;
    return v_flex().w_full().p(8).gap(8).bg(t.popover).text_color(t.foreground).text_size(12.8)
      .on_key_down((e, cx) => {
        if (e.keystroke === "escape") {
          cx.stop_propagation();
          closeOverlay(props().overlay);
        }
      })
      .child(this.renameInput
        ? v_flex().gap(6)
          .child(div().text_size(11).text_color(t.dim).child("Rename space"))
          .child(Input.new(this.renameInput).h(32).px(8).rounded(6).border_1().border_color(t.input).bg(t.background))
        : div().id("rename-space").h(31).px(8).rounded(7).cursor_pointer().hover(el => el.bg(t.hover))
          .on_click((_e, cx) => {
            this.renameInput = InputState.new({ value: space.name });
            this.renameInput.on("submit", () => {
              const name = this.renameInput.value().trim();
              if (!name) return;
              run("space.rename", { id: space.id, name });
              closeOverlay(props().overlay);
            });
            // Only the explicit Rename action takes the keyboard; render never does.
            this.renameInput.focus();
            cx.notify();
          })
          .child("Rename"));
  }

  peek(addr) {
    run("terminal.peek", { terminal: addr });
  }

  // Live sub-agents of `addr`, depth first, each with the tree lines in front of it.
  tree(all, addr, lead = []) {
    const kids = all.filter((k) => k.parent === addr && k.state === "live" && !k.labels?.["vega.worktree-owner"]);
    return kids.flatMap((k, i) => {
      const last = i === kids.length - 1;
      return [
        { t: k, lines: [...lead, last ? "└" : "├"] },
        ...this.tree(all, k.address, [...lead, last ? "" : "│"]),
      ];
    });
  }

  subRow(t, s, shown, compact) {
    const k = s.t;
    const st = kidStatus(k, this.listed);
    const pic = icon(k.program, 14, t.foreground);
    const on = k.address === shown;
    return h_flex()
      .id(`sub-${k.address}`)
      .pl(39)
      .pr(8)
      .rounded(8)
      .h(26)
      .flex_none()
      .gap(0)
      .items_center()
      .text_size(12.5)
      .when(on, (el) => el.bg(t.raised))
      .hover((s) => s.bg(t.hover))
      .on_click(() => this.peek(k.address))
      .children(s.lines.map(l => div().self_start().mt(-4).w(9).h(17).mr(4).flex_none()
        .when(Boolean(l), el => el.border_l(1.5).border_color(t.input))
        .when(l === "└" || l === "├", el => el.border_b(1.5).rounded_bl(6))))
      .when(Boolean(pic), el => el.child(div().mr(8).child(pic)))
      .child(div().flex_1().truncate().text_color(on ? t.foreground : t.muted).child(k.name || k.template || k.program?.name || k.address))
      .child(this.vegaIcons(k.address, t))
      .child(this.status(t, { status: st }, compact));
  }

  // compact: a ! for blocked where a header already says "Needs you".
  status(t, a, compact = false) {
    const color = t[a.status] ?? t.dim;
    if (a.status === "working") return svg("mods/workspace-chrome/spinner.svg").size(12).mx(1).flex_none().text_color(t.working).spin(900);
    if (a.status === "blocked" && compact) return div().size(14).flex_none().rounded(4).bg(t.blocked + "26").text_size(10.5).font_bold().text_color(t.blocked).flex().items_center().justify_center().child("!");
    if (a.status === "blocked") return div().flex_none().px(6).h(18).rounded(5).bg(t.blocked + "26").text_size(10.5).font_semibold().text_color(t.blocked).flex().items_center().child("Needs you");
    // Idle: a ring; done: a dot.
    if (a.status === "idle") return div().size(7).mx(3).flex_none().rounded(4).border(1.5).border_color(t.idle ?? t.dim);
    return div().size(7).mx(3).flex_none().rounded(4).bg(a.status === "done" ? color : t.dim + "00");
  }

  // The space a terminal shows in: its herdr.space label, machine-qualified like spaces() ids.
  spaceOfTerminal(term) {
    const id = term.labels?.["herdr.space"];
    if (!id) return null;
    const slash = term.address.indexOf("/");
    return slash < 0 ? id : `${term.address.slice(0, slash)}/${id}`;
  }

  // UAC projects in the Slayer's order: one group per project with a UAC on any machine, holding
  // that project's agents on every machine (UAC first, its sentinels as branches). Then the
  // spaces without a UAC. Symbols as in the herdr TUI sidebar (docs/design/theme-symbols.md).
  projectTree(t, all, list, shown, now) {
    const short = value => value?.replace(/\/$/, "").split("/").at(-1)?.replace(/\.git$/, "");
    const live = all.filter(x => x.state === "live");
    const labelsOf = address => this.vegaTerminals.get(address)?.labels ?? {};
    const uacs = new Map();
    for (const x of live) if (x.labels?.["vega.role"] === "uac" && x.labels?.["vega.project"]) uacs.set(short(x.labels["vega.project"]), x);
    const projectOfSpace = new Map();
    for (const x of live) {
      const sp = this.spaceOfTerminal(x), p = short(x.labels?.["vega.project"]);
      if (sp && p && uacs.has(p) && !projectOfSpace.has(sp)) projectOfSpace.set(sp, p);
    }
    const projectOfAgent = a => {
      const p = short(labelsOf(a.address)["vega.project"]);
      return uacs.has(p) ? p : projectOfSpace.get(a.space) ?? null;
    };
    const saved = this.order ?? [];
    const projects = [...uacs.keys()].sort((a, b) => {
      const i = saved.indexOf(a), j = saved.indexOf(b);
      return (i < 0 ? Infinity : i) - (j < 0 ? Infinity : j) || a.localeCompare(b);
    });
    const C = this.colors();
    const kindOf = (k) => {
      const key = (labelsOf(k.address)["vega.harness"] || k.program?.key || k.app || "").toLowerCase();
      for (const [name, glyph] of [["claude", "✳"], ["codex", "֍"], ["copilot", "✈"], ["pi", "π"]]) if (key.includes(name)) return [glyph, C[name]];
      return ["◇", C.other_agent];
    };
    const host = (address) => {
      const label = labelsOf(address)["vega.host"];
      return label ? div().w(22).flex_none().text_right().text_size(12).text_color(C.hosts[label] ?? t.dim).child(label) : div().w(22).flex_none();
    };
    const tokens = (address, uac) => this.vegaIcons(address, t, uac ? ["uac", "kind", "git"] : ["uac", "kind"]);
    const agentRow = (a, subs, depth, lines = []) => {
      const l = labelsOf(a.address);
      const uac = l["vega.role"] === "uac";
      const [glyph, color] = kindOf(a);
      const st = depth ? { status: kidStatus(a, this.listed) } : a;
      const when = depth ? 0 : this.since(a);
      const name = uac ? "" : depth
        ? (l["vega.label"]?.replace(/^[^\p{L}\p{N}]+/u, "") || a.name || a.template || a.title || a.address)
        : [this.rowLabel(a), clean(a.title), a.program?.name].find(n => n && !/^\d+$/.test(n)) || "";
      const on = a.address === shown;
      return h_flex()
        .id(depth ? `sub-${a.address}` : `ag-${a.address}`)
        .pl(18).pr(6).h(24).flex_none().gap(6).items_center().rounded(6)
        .when(on, el => el.bg(t.selected))
        .cursor_pointer().hover(s => s.bg(t.hover))
        .tooltip([a.title, l["vega.role"]].filter(Boolean).join(" · ") || a.address)
        .on_click(() => depth ? this.peek(a.address) : run("pane.focus", { pane: a.pane }))
        .children(lines.map(x => div().self_start().mt(-4).w(9).h(16).mr(2).flex_none()
          .when(Boolean(x), el => el.border_l(1.5).border_color(t.input))
          .when(x === "└" || x === "├", el => el.border_b(1.5).rounded_bl(6))))
        .child(this.status(t, st, true))
        .when(uac, el => el.child(div().flex_none().font_semibold().text_color(C.uac).child(l["vega.uac"] || "u▲c")))
        .child(div().flex_none().text_color(color).child(glyph))
        .when(Boolean(name), el => el.child(div().min_w(0).flex_shrink_1().truncate().text_color(a.status === "idle" ? t.muted : t.foreground).child(name)))
        .child(tokens(a.address, uac))
        .child(div().flex_1())
        .when(!depth && subs.length > 0 && this.folded.has(a.address), el => el.child(div().text_size(11).text_color(t.dim).child(`+${subs.length}`)))
        .when(Boolean(when), el => el.child(div().flex_none().text_size(11).text_color(t.dim).child(duration(now - when))))
        .child(host(a.address));
    };
    const withTree = (a) => {
      const subs = this.tree(all, a.address);
      return [agentRow(a, subs, 0), ...(this.folded.has(a.address) ? [] : subs.map(s => agentRow(s.t, [], 1, s.lines)))];
    };
    const badge = (key, glyph, n, action, gid) => div().id(`total-${gid}-${key}`).flex_none().text_size(11).font_semibold().text_color(C[key])
      .tooltip(`Project ${key}`).child(`${glyph} ${n}`)
      .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
      .on_click((_e, cx) => { cx.stop_propagation(); action(); });
    const out = [];
    for (const p of projects) {
      const gid = `uac-${p}`;
      const uac = uacs.get(p);
      const ul = uac.labels ?? {};
      const project = ul["vega.project-id"];
      const totals = this.vegaTotals(uac.address, project) ?? {};
      const members = list.filter(a => projectOfAgent(a) === p)
        .sort((a, b) => (labelsOf(b.address)["vega.role"] === "uac") - (labelsOf(a.address)["vega.role"] === "uac"));
      const open = !this.folded.has(gid);
      const git = ul["vega.git"];
      out.push(h_flex().id(gid).pl(2).pr(6).pt(8).h(30).flex_none().gap(6).items_center().rounded(6)
        .when(this.dragFrom === p && this.dragOver !== p, el => el.opacity(0.5))
        .when(Boolean(this.dragFrom) && this.dragOver === p && this.dragFrom !== p, el => el.border_t(2).border_color(t.accent))
        .on_mouse_down("left", () => { this.dragFrom = p; this.dragOver = p; })
        .on_mouse_move((_e, cx) => { if (this.dragFrom && this.dragOver !== p) { this.dragOver = p; cx.notify(); } })
        .on_mouse_up("left", (_e, cx) => {
          const from = this.dragFrom;
          this.dragFrom = null; this.dragOver = null;
          if (from && from !== p) this.move(projects, from, p);
          else if (from === p) open ? this.folded.add(gid) : this.folded.delete(gid);
          cx.notify();
        })
        .child(div().flex_none().text_size(12).text_color(t.dim).child("⠿"))
        .child(div().min_w(0).flex_shrink_1().truncate().font_semibold().text_color(t.foreground).child(p))
        .when(Boolean(git), el => el.child(div().flex_none().text_size(11.5).text_color(git === "✓" ? C.git_clean : C.git_dirty).child(git)))
        .child(div().flex_1())
        .when(totals.notes >= 1, el => el.child(badge("notes", "✉", totals.notes, () => openOverlay({ mod: "vega-sidebar", kind: "window", key: `vega-notes-${project}`, width: 700, height: 500, props: { mail: uac.address, notes: true } }), gid)))
        .when(totals.decisions >= 1, el => el.child(badge("decisions", "⚖", totals.decisions, () => openOverlay({ mod: "vega-inbox", kind: "window", key: "vega-inbox", keep: true, width: 620, height: 520, props: { project } }), gid)))
        .when(totals.waiting >= 1, el => el.child(badge("waiting", "✋", totals.waiting, () => run("agent.focus", { terminal: totals.waiting_panes?.[0]?.pane?.replace(/^hc:/, "") || uac.address }), gid)))
        .when(!open, el => el.child(svg("mods/workspace-chrome/chevron-right.svg").size(11).flex_none().text_color(t.dim))));
      if (open) for (const a of members) out.push(...withTree(a));
    }
    const all_spaces = spaces();
    const rest = all_spaces.filter(s => !projectOfSpace.has(s.id));
    if (rest.length) {
      out.push(div().px(6).pt(12).pb(2).text_size(11).font_medium().text_color(t.dim).child("Spaces"));
      for (const sp of rest) {
        out.push(h_flex().id(`space-${sp.id}`).pl(18).pr(6).h(24).flex_none().gap(6).items_center().rounded(6)
          .when(sp.active, el => el.bg(t.selected))
          .cursor_pointer().hover(s => s.bg(t.hover))
          .on_click(() => run("space.select", { id: sp.id }))
          .on_mouse_down("right", (_e, cx) => this.openSpaceMenu(sp, cx))
          .child(div().min_w(0).flex_shrink_1().truncate().text_color(t.muted).child(sp.name))
          .child(div().flex_1())
          .child(div().w(22).flex_none().text_right().text_size(12).text_color(C.hosts[sp.machine ? labelOfMachine(sp.machine, live) : this.localHost(live)] ?? t.dim)
            .child(sp.machine ? labelOfMachine(sp.machine, live) : this.localHost(live))));
        for (const a of list.filter(a => a.space === sp.id && !projectOfAgent(a))) out.push(...withTree(a));
      }
    }
    // Agents whose space this window doesn't know yet still show.
    const known = new Set(all_spaces.map(s => s.id));
    for (const a of list.filter(a => !known.has(a.space) && !projectOfAgent(a))) out.push(...withTree(a));
    return out;
  }

  // The local machine's $host label: the one vegad wrote on a local terminal.
  localHost(live) {
    return live.find(x => !x.address.includes("/") && x.labels?.["vega.host"])?.labels["vega.host"] ?? "";
  }

  // Drag one project above another; the order stays across restarts.
  move(projects, from, to) {
    const order = projects.filter(p => p !== from);
    order.splice(order.indexOf(to), 0, from);
    this.order = order;
    try { localStorage.setItem("project-order", JSON.stringify(order)); } catch (_) {}
  }

  // Colours from ~/.config/vega/theme.toml [symbols.colors] and [symbols.host_colors], else
  // vega's defaults (crates/vega-herdr/src/theme.rs COLORS). Host colours are keyed by label.
  colors() {
    const s = this.symbols ?? {};
    const c = { ...DEFAULT_COLORS, ...(s.colors ?? {}) };
    const hosts = {};
    for (const [name, label] of Object.entries(s.hosts ?? {})) if (s.host_colors?.[name]) hosts[label] = s.host_colors[name];
    return { ...c, hosts };
  }

  vegaTotals(address, project) {
    const machine = address?.includes("/") ? address.split("/")[0] : "";
    return (shared("vega.badges") ?? []).find(r => r.project === project &&
      (r.machine || "") === machine && Date.now()/1000-r.at <= 60);
  }

  // uac and kind are drawn before the name, as in the TUI row.
  vegaIcons(address, t, skip = ["uac", "kind"]) {
    const labels = this.vegaTerminals?.get(address)?.labels ?? {};
    const focus = () => run("agent.focus", {terminal: address});
    const inbox = () => openOverlay({mod: "vega-inbox", kind: "window", key: "vega-inbox", keep: true,
      width: 620, height: 520, props: {project: labels["vega.project-id"]}});
    const mail = (notes) => openOverlay({mod: "vega-sidebar", kind: "window", key: `vega-mail-${address}`,
      width: 700, height: 500, props: {mail: address, notes}});
    const controls = [
      ["uac", "UAC", focus], ["kind", "Harness", focus],
      ["mail", "Agent inbox (archive; does not acknowledge mail)", () => mail(false)],
      ["notes", "Notifications", () => mail(true)],
      ["waiting", "Waiting panes", focus], ["decisions", "Decisions", inbox],
      ["git", "Git changes", () => { focus(); run("layout.show", {id:"panel"}); run("layout.select", {stack:"panel-body", child:"changes"}); }],
    ];
    return h_flex().gap(4).flex_none().children(controls.filter(([key]) => !skip.includes(key) && /[^\s\u2800]/u.test(labels[`vega.${key}`] ?? "")).map(([key, tip, action]) =>
      div().id(`vega-${address}-${key}`).text_size(11).text_color(t.muted).cursor_pointer().tooltip(tip)
        .child(key === "attention" ? "✋" : labels[`vega.${key}`])
        .on_mouse_down("left", (_e,cx) => cx.stop_propagation())
        .on_click((_e,cx) => {cx.stop_propagation(); action();})));
  }

  render(cx) {
    const t = theme();
    if (props()?.mail) return v_flex().size_full().p(16).gap(12).bg(t.chrome).text_color(t.foreground)
      .child(h_flex().justify_between().child(`Mail · ${props().mail}`)
        .child(div().id("close-mail").cursor_pointer().child("Close").on_click(() => closeOverlay(props().overlay))))
      .child(v_flex().id("mail-body").flex_1().overflow_y_scrollbar().gap(12)
        .children(Array.isArray(this.mail) ? (this.mail.length ? this.mail.map((m,i) => v_flex().id(`mail-${i}`).gap(5).p(10).bg(t.surface)
          .child(div().text_size(11).text_color(t.muted).child(`${m.from || m.host || ""} · ${m.time || ""} · ${m.read === false || m.unread ? "Unread" : "Read"}`))
          .when(Boolean(m.title), el => el.child(div().font_semibold().child(m.title)))
          .child(div().child(m.text ?? m.body ?? ""))) : [div().child("No mail.")]) : [div().child(this.mail)]));
    if (props()?.sortMenu) return this.sortMenu(t);
    if (props()?.spaceMenu) return this.spaceMenu(t);
    const collapsed = narrow();
    const sort = sortOf();
    let list = agents();
    this.listed = new Map(list.map((a) => [a.address, a.status]));
    const all = terminals();
    this.vegaTerminals = new Map(all.map(t => [t.address, t]));
    // A worktree's placeholder shell holds the checkout for its sentinel; the TUI never listed it.
    const placeholder = (address) => Boolean(this.vegaTerminals.get(address)?.labels?.["vega.worktree-owner"]);
    list = list.filter(a => !placeholder(a.address));
    this.migratePins(all);
    const shown = focused()?.agent;
    const now = Date.now();
    this.track(list, now);
    this.hold(list);
    const byPriority = sort.id === "priority";
    const urgency = new Map(list.map((a) => [a.address, Math.max(this.rank(a), ...this.tree(all, a.address).map((s) => RANK[kidStatus(s.t, this.listed)] ?? 0))]));
    const rankOf = (a) => urgency.get(a.address);
    if (byPriority) list = [...list].sort((x, y) => rankOf(y) - rankOf(x) || this.since(y) - this.since(x) || y.seq - x.seq);
    // A listed child belongs to its visible ancestor's tree, even in another space.
    const listed = new Set(list.map(a => a.address));
    list = list.filter(a => {
      const seen = new Set([a.address]);
      let parent = this.vegaTerminals.get(a.address)?.parent;
      while (parent && !seen.has(parent) && this.vegaTerminals.get(parent)?.state === "live") {
        if (listed.has(parent)) return false;
        seen.add(parent); parent = this.vegaTerminals.get(parent)?.parent;
      }
      return true;
    });
    const pinned = list.filter((a) => a.pinned);
    const rest = list.filter((a) => !a.pinned);
    list = [...pinned, ...rest];
    setAgentOrder(list.flatMap(a => [a.address, ...this.tree(all, a.address).filter(s => listed.has(s.t.address)).map(s => s.t.address)]));

    const withKids = (a, i) => {
      const subs = collapsed ? [] : this.tree(all, a.address);
      const r = row(a, i, subs);
      if (this.folded.has(a.address)) return [r];
      return [r, ...subs.map((s) => this.subRow(t, s, shown, byPriority))];
    };

    // The fold chevron; folded, the count too, colored by the most urgent one.
    const fold = (a, subs) => {
      const sts = subs.map((s) => kidStatus(s.t, this.listed));
      const top = sts.reduce((m, s) => ((RANK[s] ?? 0) > (RANK[m] ?? 0) ? s : m), null);
      const open = !this.folded.has(a.address);
      return div()
        .id(`fold-${a.address}`)
        .h(18)
        .px(3)
        .gap(3)
        .flex()
        .items_center()
        .rounded(4)
        .text_size(11)
        .text_color(open ? t.dim : (t[top] ?? t.dim))
        .cursor_pointer()
        .hover((s) => s.bg(t.hover).text_color(t.foreground))
        .on_click((_e, cx) => {
          cx.stop_propagation();
          open ? this.folded.add(a.address) : this.folded.delete(a.address);
          cx.notify();
        })
        .child(svg(`mods/workspace-chrome/chevron-${open ? "down" : "right"}.svg`).size(11).flex_none())
        .when(!open, (el) => el.child(String(subs.length)));
    };

    // A row reads "space \u00b7 detail": the detail is a tab someone named or the
    // agent's own title, never an automatic tab number unless two rows would
    // otherwise read the same.
    // Read the pane snapshot once: every host read copies the whole list into JS.
    const leads = leadPanes(panes());
    const tabLabels = new Map(spaces().flatMap(s => s.tabs).map(tab => [tab.id, title(tab, leads.get(tab.id))]));
    const labels = new Map(list.map(a => [a.address, tabLabels.get(a.tab) ?? (a.title || a.program?.name || "")]));
    const label = (a) => labels.get(a.address);
    const detail = new Map(list.map(a => [a.address, /^\d+$/.test(label(a)) ? null : label(a)]));
    const seen = {};
    for (const a of list) { const k = `${a.space_name}\u0000${detail.get(a.address) ?? ""}`; seen[k] = (seen[k] ?? 0) + 1; }
    for (const a of list) {
      if (detail.get(a.address) == null && seen[`${a.space_name}\u0000`] > 1) detail.set(a.address, label(a));
    }

    const row = (a, i, subs = []) => {
      const pic = icon(a.program, 16, t.foreground);
      const more = detail.get(a.address);
      // An agent on another machine (`vps/t7`) says which.
      const machine = a.address.includes("/") ? a.address.split("/")[0] : null;
      if (collapsed) {
        return h_flex()
          .id(`ag-${a.address}`)
          .h(26)
          .px(8)
          .flex_none()
          .items_center()
          .gap(4)
          .hover((s) => s.bg(t.hover))
          .on_click(() => run("pane.focus", { pane: a.pane }))
          .child(div().flex_1().min_w(0).whitespace_nowrap().text_right().text_color(t.muted).child(String(i + 1)))
          .child(h_flex().w(14).flex_none().items_center().justify_center().child(this.status(t, a, true)));
      }
      const when = this.since(a);
      const idle = a.status === "idle" && !a.focused;
      const isPinned = a.pinned;
      const pin = div()
        .id(`pin-${a.address}`)
        .flex_none()
        .p(2)
        .rounded(4)
        .text_color(t.dim)
        .cursor_pointer()
        .hover((s) => s.bg(t.hover).text_color(t.foreground))
        .tooltip(isPinned ? "Unpin" : "Pin to the top")
        .on_click((_e, cx) => {
          cx.stop_propagation();
          run("terminal.pin", { terminal: a.address, pinned: !isPinned });
          // The row moves away from under the pointer, and no hover-out comes.
          this.hovered = null;
          cx.notify();
        })
        .child(svg(`mods/workspace-chrome/${isPinned ? "pin-off" : "pin"}.svg`).size(13));
      // Under a space header the space goes without saying; an automatic tab number says nothing.
      // Without a space header above (priority, pinned) the row names its space.
      const full = byPriority || isPinned;
      const name = full ? a.space_name || label(a) : more ?? (/^\d+$/.test(label(a)) ? a.program?.name || label(a) : label(a));
      // One line as in the TUI: status · u▲c · harness · name · vega tokens.
      const vlabels = this.vegaTerminals.get(a.address)?.labels ?? {};
      return h_flex()
        .id(`ag-${a.address}`)
        .px(6)
        .h(26)
        .flex_none()
        .rounded(6)
        .items_center()
        .gap(6)
        .when(a.focused, (el) => el.bg(t.selected))
        .cursor_pointer()
        .hover((s) => s.bg(t.hover))
        .on_hover((on, cx) => {
          if (on) this.hovered = a.address;
          else if (this.hovered === a.address) this.hovered = null;
          cx.notify();
        })
        .on_click(() => run("pane.focus", { pane: a.pane }))
        .tooltip(more ? `${a.space_name} · ${more}` : a.space_name || label(a))
        .child(this.status(t, a, true))
        .when(Boolean(vlabels["vega.uac"]), el => el.child(div().id(`vega-${a.address}-uac`).flex_none().text_size(12).text_color(t.ansi1 ?? t.blocked)
          .on_mouse_down("left", (_e,cx) => cx.stop_propagation())
          .on_click((_e,cx) => {cx.stop_propagation(); run("agent.focus", {terminal: a.address});})
          .child(vlabels["vega.uac"])))
        .when(Boolean(pic), el => el.child(div().id(`agent-fold-${a.address}`)
          .when(subs.length > 0, el => el.on_click((_e, cx) => {
            cx.stop_propagation();
            this.folded.has(a.address) ? this.folded.delete(a.address) : this.folded.add(a.address);
            cx.notify();
          })).child(pic)))
        .child(div().min_w(0).flex_shrink_1().truncate().text_color(idle ? t.muted : t.foreground).child(name))
        .when(full && Boolean(more), el => el.child(div().min_w(0).flex_shrink_1().truncate().text_color(t.dim).child(`\u00b7 ${more}`)))
        .child(this.vegaIcons(a.address, t))
        .child(div().flex_1())
        .when(Boolean(machine), el => el.child(div().text_size(10).text_color(t.dim).child(machine)))
        .when(subs.length > 0, el => el.child(fold(a, subs)))
        .when(this.hovered === a.address, el => el.child(pin))
        .when(this.hovered !== a.address && Boolean(when), el => el.child(div().flex_none().text_size(11).text_color(t.dim).child(duration(now - when))));
    };

    // Groups in list order: spaces (grouped) or priority sections. Agents of a
    // space are next to each other in agents(), so neighbours make a group.
    const groups = pinned.length ? [{ id: "pinned", label: "Pinned", status: null, members: pinned }] : [];
    for (const a of rest) {
      const r = rankOf(a) || RANK.idle;
      const id = byPriority ? `sec-${r}` : `sp-${a.space}`;
      if (groups.at(-1)?.id === id) groups.at(-1).members.push(a);
      else groups.push({ id, label: byPriority ? SECTION[r] : a.space_name || "", status: byPriority ? SECTION_STATUS[r] : null, members: [a] });
    }

    // A group's header; it folds the group. Folded, it says how many and the most urgent state.
    const header = (g) => {
      const open = !this.folded.has(g.id);
      const top = g.members.reduce((m, a) => ((RANK[a.status] ?? 0) > (RANK[m] ?? 0) ? a.status : m), null);
      return h_flex()
        .id(g.id)
        .px(8)
        .pt(8)
        .h(30)
        .flex_none()
        .gap(6)
        .items_center()
        .text_size(11)
        .font_medium()
        .cursor_pointer()
        .hover((s) => s.text_color(t.foreground))
        .on_click((_e, cx) => {
          open ? this.folded.add(g.id) : this.folded.delete(g.id);
          cx.notify();
        })
        .child(div().truncate().text_color(t[g.status] ?? t.dim).child(g.label))
        .when(byPriority || !open, (el) => el.child(div().text_color(t.dim).child(String(g.members.length))))
        .child(div().flex_1())
        .when(!open, (el) => el
          .when(!byPriority && top && top !== "idle", (el) => el.child(this.status(t, { status: top }, true)))
          .child(svg("mods/workspace-chrome/chevron-right.svg").size(11).flex_none().text_color(t.dim)));
    };
    this.rowLabel = a => detail.get(a.address) ?? label(a);
    const rows = collapsed
      ? list.flatMap(withKids)
      : sort.id === "project"
        ? this.projectTree(t, all, list, shown, now)
        : groups.flatMap((g) => [header(g), ...(this.folded.has(g.id) ? [] : g.members.flatMap(withKids))]);

    return v_flex()
      .size_full()
      .min_h(0)
      .bg(t.chrome)
      .border_t(1)
      .border_color(t.border)
      .text_size(13)
      .when(!collapsed, (el) =>
        el.child(
          h_flex()
            .px(16)
            .h(41)
            .pt(14)
            .pb(5)
            .flex_none()
            .items_center()
            .justify_between()
            .child(div().text_size(11.5).font_medium().text_color(t.dim).child("VEGA"))
            .child(
              div()
                .id("ag-sort")
                .text_size(11)
                .text_color(t.dim)
                .hover((s) => s.text_color(t.foreground))
                .on_click(() => openOverlay({ mod: "vega-sidebar", kind: "popup", width: 200, height: 170, props: { sortMenu: true } }))
                .tooltip(`Sort agents: ${sort.label}`)
                .child(svg(`mods/workspace-chrome/${sort.icon}.svg`).size(14)),
            ),
        ),
      )
      .when(list.length === 0 && !collapsed, (el) =>
        el.child(div().px(12).text_size(12).text_color(t.dim).child("No agent status yet. Agents show up here when they report their status. Older versions and agents inside tmux may not.")),
      )
      .child(v_flex().id("ag-list").flex_1().min_h(0).px(collapsed ? 0 : 8).overflow_y_scrollbar().mode("always").children(rows))
      .when(collapsed, (el) =>
        el.child(
          h_flex()
            .h(28)
            .items_center()
            .justify_center()
            .child(
              div()
                .id("collapse")
                .text_size(13)
                .text_color(t.dim)
                .hover((s) => s.text_color(t.foreground))
                .on_click(toggleSidebar)
                .child("»"),
            ),
        ),
      );
  }
}
