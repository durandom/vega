// Manually ordered projects and spaces; project preferences belong to vega.

import { View, div, svg } from "gpui-kit";
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
// A sub-agent without a pane isn't in agents(): its raw word, error counting as
// blocked. It's never done: it reports to its parent, and the parent's done is the news.
const kidStatus = (k, listed) => {
  const s = listed.get(k.address) ?? k.status;
  return s === "error" ? "blocked" : s === "done" ? "idle" : s;
};
// Later stages add tab descriptors and renderers here without changing the shell.
const PROJECT_TABS = [{ id: "overview", label: "Overview", render: "overviewTab" }, { id: "uac", label: "UAC", render: "uacTab" },{ id: "config", label: "Config", render: "configTab" }];
const values = input => [...new Set(input.split(",").map(s => s.trim()).filter(Boolean))];
const machineOf = address => address?.includes("/") ? address.split("/")[0] : "local";

function duration(ms) {
  const m = Math.floor(ms / 60000);
  if (m < 1) return "<1m";
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  return h < 24 ? `${h}h` : `${Math.floor(h / 24)}d`;
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
    if (props()?.projectConfig || props()?.viewMenu) {
      this.preferences = { projects: {}, views: {} };
      this.loading = true;
      if (props()?.viewMenu) {
        this.viewName = InputState.new({ placeholder: "View name" });
        this.viewLabels = InputState.new({ placeholder: "Labels, comma separated" });
        this.viewMachines = InputState.new({ placeholder: "Machines, comma separated (local for this host)" });
      }
      cx.spawn(async cx => {
        await this.refreshPreferences(cx);
        const config = this.preferences.projects?.[props()?.projectConfig];
        if (props()?.projectConfig && !this.configError) {
          this.nameInput = InputState.new({ value: config?.name ?? "", placeholder: "Default project name" });
          this.labelsInput = InputState.new({ value: (config?.labels ?? []).join(", "), placeholder: "work, private" });
          this.originalLabels = config?.labels ?? [];
        }
        this.loading = false;
        cx.notify();
      });
      if (props()?.projectConfig) {
        // A window does not take the keyboard when it opens: own a focus handle so Esc works.
        this.focusHandle = cx.focus_handle?.();
        this.focusHandle?.focus();
        cx.spawn(cx => this.readOverview(cx));
        cx.timer.every(15000, cx => this.readOverview(cx));
      }
      return;
    }
    if (props()?.spaceMenu) return;
    this.preferences = { projects: {}, views: {} };
    this.activeView = localStorage.getItem("active-view") ?? "all";
    cx.spawn(cx => this.refreshPreferences(cx));
    cx.timer.every(5000, cx => this.refreshPreferences(cx));
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

  async refreshPreferences(cx) {
    if (this.readingPreferences) return;
    this.readingPreferences = true;
    try {
      const r = await exec("vega", ["project", "config", "--json"]);
      if (r.code !== 0) throw new Error(r.stderr || r.stdout || "Project config unavailable");
      const data = JSON.parse(r.stdout);
      if (!data.projects || !data.views) throw new Error("Invalid project config response");
      this.preferences = data;
      this.configError = null;
    } catch (e) { this.configError = String(e); }
    finally { this.readingPreferences = false; cx.notify(); }
  }

  // One draggable window per project (key: project ID) with its tabs. No terminal block:
  // a terminal rendered in this window and in its pane at once jitters (vega-0h4c.50).
  // A click on an agent row focuses that agent's terminal in the main view instead.
  openProject(project, cx, tab = null) {
    cx?.stop_propagation?.();
    if (!project) return;
    const uac = terminals().find(x => x.state === "live" && x.labels?.["vega.role"] === "uac"
      && x.labels?.["vega.project-id"] === project);
    openOverlay({ kind: "window", key: `vega-project-${project}`, width: 640, height: 0.7,
      mod: "vega-sidebar", props: { projectConfig: project, uac: uac?.address ?? null, tab } });
  }

  // Overview data the terminal labels do not carry. Beads counts come from `bd status --json`
  // in the UAC's checkout; remote projects wait for the corp-host check (vega-0h4c.50.2).
  async readOverview(cx) {
    const uac = terminals().find(x => x.address === props()?.uac);
    if (!uac || this.readingOverview) return;
    if (uac.address.includes("/")) { this.beads = "remote"; cx.notify(); return; }
    this.readingOverview = true;
    let beads;
    try {
      const r = await exec("bd", ["status", "--json"], { cwd: uac.cwd, timeout: 10000 });
      // Tolerate log lines before the JSON object.
      beads = r.code === 0 ? JSON.parse(r.stdout.slice(r.stdout.indexOf("{"))).summary : (r.stderr || "bd failed").trim();
    } catch (e) { beads = String(e); }
    // vega's --json views (vega-0h4c.50.4), run in the UAC's checkout so they pick its project.
    const vega = async args => {
      try {
        const r = await exec("vega", [...args, "--json"], { cwd: uac.cwd, timeout: 10000 });
        return r.code === 0 ? JSON.parse(r.stdout.slice(r.stdout.indexOf("{"))) : null;
      } catch { return null; }
    };
    const [asks, ci, capacity, facility] = await Promise.all([
      vega(["notify", "list", "--project", props().projectConfig]), vega(["ci"]), vega(["capacity"]), vega(["uac"])]);
    const ov = { asks: asks?.asks ?? null, watches: ci?.watches ?? null,
      capacity: capacity?.windows?.filter(w => w.binding) ?? null, agents: facility?.agents ?? [] };
    this.readingOverview = false;
    // Redraw only on a change: the window polls, most reads are equal.
    const sig = JSON.stringify([beads, ov]);
    if (sig !== this.beadsSig) { this.beadsSig = sig; this.beads = beads; this.ov = ov; cx.notify(); }
  }

  overviewTab(t) {
    const all = terminals();
    const uac = all.find(x => x.address === props().uac);
    if (!uac) return div().text_color(t.dim).child("No live UAC for this project.");
    const kids = all.filter(x => x.parent === uac.address && x.state === "live");
    const totals = this.vegaTotals(uac.address, props().projectConfig) ?? {};
    const since = uac.status_ms ? duration(Date.now() - uac.status_ms) : "";
    const tile = (id, icon, n, label, color, action) => v_flex().id(`ov-${id}`).flex_1().p(10).gap(2).rounded(8)
      .bg(t.surface).when(Boolean(action) && n > 0, el => el.cursor_pointer()
        .on_mouse_down("left", (_e, cx) => cx.stop_propagation()).on_click(() => action()))
      .child(h_flex().gap(6).items_center()
        .child(div().text_size(16).text_color(n > 0 ? color : t.dim).child(icon))
        .child(div().text_size(18).font_semibold().text_color(n > 0 ? t.foreground : t.dim).child(String(n ?? "–"))))
      .child(div().text_size(11).text_color(t.dim).child(label));
    const b = typeof this.beads === "object" && this.beads ? this.beads : null;
    // Status colours come from the theme only, so light and dark both work.
    const statusColor = s => ({ working: t.working, blocked: t.blocked, done: t.done, idle: t.idle })[s] ?? t.unknown ?? t.dim;
    const section = (label, n) => h_flex().gap(8).items_center().pt(2)
      .child(div().text_size(10.5).font_semibold().text_color(t.dim).child(`${label.toUpperCase()} ${n}`))
      .child(div().flex_1().h(1).bg(t.border));
    const row = x => h_flex().id(`ov-row-${x.address}`).gap(8).px(8).py(5).rounded(6).cursor_pointer()
      .when(x.address === focused()?.terminal, el => el.bg(t.raised))
      .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
      .on_click((_e, cx) => { cx.stop_propagation(); run("pane.focus", { terminal: x.address }); })
      .child(div().w(8).h(8).rounded(4).flex_none().bg(statusColor(x.status)))
      .child(div().flex_1().min_w(0).child(x.name || x.address))
      .child(div().w(110).flex_none().text_color(t.dim).whitespace_nowrap().child(this.ov?.agents?.find(a => a.pane === x.address)?.bead || ""))
      .child(div().w(70).flex_none().text_color(t.dim).child(x.agent ?? x.program?.key ?? ""))
      .child(div().w(64).flex_none().text_color(t.dim).child(x.status ?? ""));
    const ov = this.ov ?? {};
    const bar = w => h_flex().gap(8).items_center()
      .child(div().w(90).flex_none().text_color(t.dim).child(w.provider))
      .child(div().flex_1().h(4).rounded(2).bg(t.border)
        .child(div().h(4).rounded(2).w(`${Math.max(0, Math.min(100, w.left_percent))}%`)
          .bg(w.left_percent < 20 ? t.blocked : w.left_percent < 50 ? (t.ansi3 ?? t.working) : t.done)))
      .child(div().w(70).flex_none().text_right().text_color(w.stale ? t.dim : t.foreground)
        .child(`${Math.round(w.left_percent)} %${w.override ? " ⚑" : ""}`));
    const inbox = () => openOverlay({ mod: "vega-inbox", kind: "window", key: "vega-inbox", keep: true, width: 620, height: 520, props: { project: props().projectConfig } });
    const notes = () => openOverlay({ mod: "vega-sidebar", kind: "window", key: `vega-notes-${props().projectConfig}`, width: 700, height: 500, props: { mail: uac.address, notes: true } });
    const waiting = () => run("pane.focus", { terminal: totals.waiting_panes?.[0]?.pane?.replace(/^hc:/, "") || uac.address });
    const empty = (title, hint, action) => v_flex().items_center().gap(4).py(10)
      .child(div().text_color(t.foreground).child(title))
      .child(div().text_size(11).text_color(t.dim).child(hint))
      .when(Boolean(action), el => el.child(div().id(`ov-retry-${title}`).mt(4).px(10).py(4).rounded(6).cursor_pointer().bg(t.raised)
        .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
        .on_click((_e, cx) => { this.beadsSig = null; cx.spawn(cx => this.readOverview(cx)); }).child(action)));
    const beadsBody = this.beads === undefined
      ? div().text_size(11).text_color(t.dim).child("Reading beads…")
      : this.beads === "remote" ? empty("Remote project", "Beads on other machines are not read yet.")
      : typeof this.beads === "string" ? empty("Beads unavailable", this.beads.split("\n")[0].slice(0, 120), "Retry")
      : h_flex().gap(8)
        .child(tile("progress", "◐", b.in_progress_issues ?? 0, "in progress", t.working))
        .child(tile("ready", "○", b.ready_issues ?? 0, "ready", t.done))
        .child(tile("blocked", "✗", b.blocked_issues ?? 0, "blocked", t.blocked));
    return v_flex().gap(12)
      .child(h_flex().gap(8).items_center()
        .child(div().w(8).h(8).rounded(4).flex_none().bg(statusColor(uac.status)))
        .child(div().font_semibold().child("UAC"))
        .child(div().text_color(t.dim).child(`${uac.address} · ${uac.agent ?? ""} · ${uac.status ?? ""}${since ? ` ${since}` : ""}`)))
      .child(section("Needs you", (totals.decisions ?? 0) + (totals.waiting ?? 0) + (totals.notes ?? 0) + (ov.asks?.length ?? 0)))
      .child(h_flex().gap(8)
        .child(tile("decisions", "⚖", totals.decisions ?? 0, "decisions", t.blocked, inbox))
        .child(tile("asks", "⚠", ov.asks?.length ?? 0, "asks", t.blocked))
        .child(tile("waiting", "✋", totals.waiting ?? 0, "waiting on you", t.blocked, waiting))
        .child(tile("notes", "✉", totals.notes ?? 0, "notes", t.accent, notes)))
      .when(Boolean(ov.asks?.length), el => el.child(v_flex().gap(2).children(ov.asks.slice(0, 5).map((a, i) =>
        div().id(`ov-ask-${i}`).px(8).text_size(11.5).text_color(t.foreground).child(`⚠ ${a.text ?? a.what ?? a.message ?? JSON.stringify(a).slice(0, 100)}`)))))
      .child(section("Beads", b ? (b.in_progress_issues ?? 0) + (b.ready_issues ?? 0) + (b.blocked_issues ?? 0) : "–"))
      .child(beadsBody)
      .child(section("Sentinels", kids.length))
      .child(v_flex().gap(2)
        .children(kids.length ? kids.map(row) : [empty("No sentinels", "The UAC has none running.")]))
      .child(section("CI", ov.watches?.length ?? "–"))
      .child(ov.watches?.length
        ? v_flex().gap(2).children(ov.watches.map((w, i) => h_flex().id(`ov-ci-${i}`).gap(8).px(8)
            .child(div().w(8).h(8).rounded(4).flex_none().bg(/green|pass|success/.test(w.verdict ?? w.state ?? "") ? t.done : /red|fail/.test(w.verdict ?? w.state ?? "") ? t.blocked : t.working))
            .child(div().flex_1().min_w(0).child(String(w.target ?? w.ref ?? w.pr ?? "")))
            .child(div().text_color(t.dim).child(String(w.verdict ?? w.state ?? "")))))
        : div().px(8).text_size(11).text_color(t.dim).child(ov.watches ? "No CI watches." : "CI not read."))
      .child(section("Capacity", ""))
      .child(ov.capacity?.length
        ? v_flex().gap(4).px(8).children(ov.capacity.map(bar))
        : div().px(8).text_size(11).text_color(t.dim).child("Capacity not read."));
  }

  uacTab(t) {
    const all = terminals();
    const uac = all.find(x => x.address === props().uac);
    if (!uac) return div().text_color(t.dim).child("No live UAC for this project.");
    const kids = all.filter(x => x.parent === uac.address && x.state === "live");
    const row = (x, role) => h_flex().id(`uac-row-${x.address}`).gap(8).px(8).py(6).rounded(6).cursor_pointer()
      .when(x.address === focused()?.terminal, el => el.bg(t.raised))
      .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
      .on_click((_e, cx) => { cx.stop_propagation(); run("pane.focus", { terminal: x.address }); })
      .child(div().w(80).flex_none().text_color(t.dim).child(x.address))
      .child(div().flex_1().min_w(0).child(x.name || role))
      .child(div().w(90).flex_none().text_color(t.dim).child(x.agent ?? x.program?.key ?? x.app ?? ""))
      .child(div().w(70).flex_none().child(x.status ?? ""));
    return v_flex().gap(4)
      .child(row(uac, "UAC"))
      .child(div().pt(8).text_size(11).text_color(t.dim).child(`Sentinels (${kids.length}) · click to focus`))
      .children(kids.map(k => row(k, "sentinel")));
  }

  async saveProject(cx) {
    if (this.saving || !this.nameInput) return;
    this.saving = true; this.configError = null; cx.notify();
    const labels = values(this.labelsInput.value());
    const args = ["project", "config", props().projectConfig, "--name", this.nameInput.value().trim()];
    for (const label of labels) args.push("--label", label);
    for (const label of this.originalLabels) if (!labels.includes(label)) args.push("--unlabel", label);
    args.push("--json");
    try {
      const r = await exec("vega", args);
      if (r.code !== 0) throw new Error(r.stderr || r.stdout || "Save failed");
      const saved = JSON.parse(r.stdout).projects[props().projectConfig];
      this.originalLabels = saved.labels;
      this.saved = true;
    } catch (e) { this.configError = String(e); }
    finally { this.saving = false; cx.notify(); }
  }

  configTab(t) {
    const field = (label, input) => v_flex().gap(5)
      .child(div().text_size(11).text_color(t.dim).child(label))
      .child(Input.new(input).h(32).px(8).rounded(6).border_1().border_color(t.input).bg(t.background));
    return v_flex().gap(12)
      .child(field("Display name (empty uses project name)", this.nameInput))
      .child(field("Labels (comma separated)", this.labelsInput))
      .child(div().id("save-project").h(30).px(8).rounded(6).cursor_pointer().bg(t.raised)
        .on_click((_e, cx) => cx.spawn(cx => this.saveProject(cx)))
        .child(this.saving ? "Saving…" : "Save"))
      .when(Boolean(this.saved), el => el.child("Saved"));
  }

  projectOverlay(t) {
    const tab = PROJECT_TABS.find(tab => tab.id === (this.projectTab ?? props()?.tab)) ?? PROJECT_TABS[0];
    return v_flex().size_full().p(12).gap(12).bg(t.popover).text_color(t.foreground).text_size(12.8)
      .when(Boolean(this.focusHandle), el => el.track_focus(this.focusHandle)
        .on_mouse_down("left", () => this.focusHandle.focus()))
      .on_key_down((e, cx) => { if (e.keystroke === "escape") { cx.stop_propagation(); closeOverlay(props().overlay); } })
      .child(h_flex().gap(12).children(PROJECT_TABS.map(item => div().id(`project-tab-${item.id}`)
        .px(8).py(5).rounded(6).cursor_pointer().when(item.id === tab.id, el => el.bg(t.raised))
        .on_click((_e, cx) => { this.projectTab = item.id; cx.notify(); }).child(item.label)))
        .child(div().flex_1())
        .child(div().id("close-project").px(8).py(5).rounded(6).cursor_pointer()
          .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
          .on_click(() => closeOverlay(props().overlay)).child("Close")))
      .child(div().text_size(11).text_color(t.dim).child(props().projectConfig))
      .when(Boolean(this.configError), el => el.child(div().text_color(t.blocked).child(this.configError)))
      .child(tab.id !== "config" ? this[tab.render](t) : this.loading ? div().child("Loading…") : this.nameInput ? this[tab.render](t) : div().child("Config unavailable"));
  }

  async saveView(cx, remove = false, id = this.viewName.value().trim()) {
    if (this.saving) return;
    if (!id || id === "all") {
      this.configError = "Enter a view name other than all (reserved for All).";
      cx.notify(); return;
    }
    this.saving = true; this.configError = null; cx.notify();
    const args = ["project", "view", id];
    if (remove) args.push("--delete");
    else {
      const labels = values(this.viewLabels.value()), machines = values(this.viewMachines.value());
      if (!labels.length && !machines.length) {
        this.configError = "Choose at least one label or machine; All already shows everything.";
        this.saving = false; cx.notify(); return;
      }
      for (const label of labels) args.push("--label", label);
      for (const machine of machines) args.push("--machine", machine);
    }
    args.push("--json");
    try {
      const r = await exec("vega", args);
      if (r.code !== 0) throw new Error(r.stderr || r.stdout || "Save view failed");
      await this.refreshPreferences(cx);
    } catch (e) { this.configError = String(e); }
    finally { this.saving = false; cx.notify(); }
  }

  viewMenu(t) {
    const select = (id) => { localStorage.setItem("active-view", id); closeOverlay(props().overlay); };
    return v_flex().size_full().overflow_y_scrollbar().p(12).gap(9).bg(t.popover).text_color(t.foreground).text_size(12.8)
      .on_key_down((e, cx) => { if (e.keystroke === "escape") { cx.stop_propagation(); closeOverlay(props().overlay); } })
      .child(div().id("view-all").cursor_pointer().child("All").on_click(() => select("all")))
      .children(Object.entries(this.preferences.views ?? {}).map(([id, view]) => h_flex().gap(8)
        .child(div().id(`view-${id}`).flex_1().cursor_pointer().child(id).on_click(() => select(id)))
        .child(div().id(`edit-view-${id}`).cursor_pointer().child("Edit").on_click((_e, cx) => {
          this.viewName.set_value(id); this.viewLabels.set_value(view.labels.join(", "));
          this.viewMachines.set_value(view.machines.join(", ")); cx.notify();
        }))
        .child(div().id(`delete-view-${id}`).cursor_pointer().child("Delete").on_click((_e, cx) => cx.spawn(cx => this.saveView(cx, true, id))))))
      .child(div().text_size(11).text_color(t.dim).child("Save a view · any label AND any machine"))
      .child(Input.new(this.viewName).h(30).px(6).bg(t.background))
      .child(Input.new(this.viewLabels).h(30).px(6).bg(t.background))
      .child(Input.new(this.viewMachines).h(30).px(6).bg(t.background))
      .child(div().id("save-view").cursor_pointer().child(this.saving ? "Saving…" : "Save view")
        .on_click((_e, cx) => cx.spawn(cx => this.saveView(cx))))
      .when(Boolean(this.configError), el => el.child(div().text_color(t.blocked).child(this.configError)));
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

  // Full IDs keep same-named repositories separate. Legacy short labels may
  // join only a unique UAC; new terminals always carry vega.project-id.
  projectGroups(all) {
    const short = value => value?.replace(/\/$/, "").split("/").at(-1)?.replace(/\.git$/, "");
    const live = all.filter(x => x.state === "live");
    const byAddress = new Map(live.map(x => [x.address, x]));
    const labelsOf = address => byAddress.get(address)?.labels ?? {};
    const uacs = new Map();
    for (const x of live) {
      const l = x.labels ?? {}, id = l["vega.project-id"] || l["vega.project"];
      if (l["vega.role"] === "uac" && id) uacs.set(id, x);
    }
    const resolve = l => {
      if (l["vega.project-id"]) return uacs.has(l["vega.project-id"]) ? l["vega.project-id"] : null;
      const legacy = l["vega.project"];
      if (!legacy) return null;
      if (uacs.has(legacy)) return legacy;
      const matches = [...uacs.keys()].filter(id => short(id) === legacy);
      return matches.length === 1 ? matches[0] : null;
    };
    const projectOfSpace = new Map();
    // UAC space wins over a sentinel's stale label in the same space.
    for (const x of [...uacs.values(), ...live]) {
      const sp = this.spaceOfTerminal(x), p = resolve(x.labels ?? {});
      if (sp && p && !projectOfSpace.has(sp)) projectOfSpace.set(sp, p);
    }
    const projectOfAgent = a => {
      let terminal = byAddress.get(a.address), seen = new Set();
      while (terminal && !seen.has(terminal.address)) {
        seen.add(terminal.address);
        const p = resolve(terminal.labels ?? {}) || projectOfSpace.get(this.spaceOfTerminal(terminal));
        if (p) return p;
        terminal = byAddress.get(terminal.parent);
      }
      return projectOfSpace.get(a.space) ?? null;
    };
    for (const x of live) {
      const sp = this.spaceOfTerminal(x), p = projectOfAgent(x);
      if (sp && p && !projectOfSpace.has(sp)) projectOfSpace.set(sp, p);
    }
    return { live, labelsOf, uacs, projectOfSpace, projectOfAgent };
  }

  viewFilter() {
    return this.preferences?.views?.[localStorage.getItem("active-view")] ?? { labels: [], machines: [] };
  }

  matchesView(project, machine) {
    const view = this.viewFilter();
    const labels = this.preferences?.projects?.[project]?.labels ?? [];
    return (!view.labels.length || view.labels.some(label => labels.includes(label))) &&
      (!view.machines.length || view.machines.includes(machine));
  }

  orderedProjects(uacs) {
    const saved = this.order ?? [];
    const orderIndex = id => {
      const exact = saved.indexOf(id);
      if (exact >= 0) return exact;
      // Migrate the old short-name order only for an unambiguous project.
      const short = id.split("/").at(-1);
      return [...uacs.keys()].filter(p => p.split("/").at(-1) === short).length === 1 ? saved.indexOf(short) : -1;
    };
    return [...uacs.keys()].sort((a, b) => {
      const i = orderIndex(a), j = orderIndex(b);
      return (i < 0 ? Infinity : i) - (j < 0 ? Infinity : j) || a.localeCompare(b);
    });
  }

  // UAC projects in the Slayer's order: one group per project with a UAC on any machine, holding
  // that project's agents on every machine (UAC first, its sentinels as branches). Then the
  // spaces without a UAC. Symbols as in the herdr TUI sidebar (docs/design/theme-symbols.md).
  projectTree(t, all, list, shown, now) {
    const { live, labelsOf, uacs, projectOfSpace, projectOfAgent } = this.projectGroups(all);
    const projects = this.orderedProjects(uacs).filter(p => {
      const uac = uacs.get(p);
      return this.matchesView(uac.labels?.["vega.project-id"], machineOf(uac.address));
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
        .when(uac, el => el.on_mouse_down("right", (_e, cx) => this.openProject(l["vega.project-id"], cx)))
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
      const display = this.preferences?.projects?.[project]?.name || ul["vega.project"]?.split("/").at(-1) || p.split("/").at(-1);
      const totals = this.vegaTotals(uac.address, project) ?? {};
      const members = list.filter(a => projectOfAgent(a) === p)
        .sort((a, b) => (labelsOf(b.address)["vega.role"] === "uac") - (labelsOf(a.address)["vega.role"] === "uac"));
      const open = !this.folded.has(gid);
      const git = ul["vega.git"];
      out.push(h_flex().id(gid).on_mouse_down("right", (_e, cx) => this.openProject(project, cx)).pl(2).pr(6).pt(8).h(30).flex_none().gap(6).items_center().rounded(6)
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
        .child(div().min_w(0).flex_shrink_1().truncate().font_semibold().text_color(t.foreground).child(display))
        .when(Boolean(git), el => el.child(div().flex_none().text_size(11.5).text_color(git === "✓" ? C.git_clean : C.git_dirty).child(git)))
        .child(div().flex_1())
        .when(totals.notes >= 1, el => el.child(badge("notes", "✉", totals.notes, () => openOverlay({ mod: "vega-sidebar", kind: "window", key: `vega-notes-${project}`, width: 700, height: 500, props: { mail: uac.address, notes: true } }), gid)))
        .when(totals.decisions >= 1, el => el.child(badge("decisions", "⚖", totals.decisions, () => openOverlay({ mod: "vega-inbox", kind: "window", key: "vega-inbox", keep: true, width: 620, height: 520, props: { project } }), gid)))
        .when(totals.waiting >= 1, el => el.child(badge("waiting", "✋", totals.waiting, () => run("agent.focus", { terminal: totals.waiting_panes?.[0]?.pane?.replace(/^hc:/, "") || uac.address }), gid)))
        .when(!open, el => el.child(svg("mods/workspace-chrome/chevron-right.svg").size(11).flex_none().text_color(t.dim))));
      if (open) for (const a of members) out.push(...withTree(a));
    }
    const all_spaces = spaces();
    const rest = all_spaces.filter(s => !projectOfSpace.has(s.id) && this.matchesView(null, s.machine || "local"));
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
    for (const a of list.filter(a => !known.has(a.space) && !projectOfAgent(a) && this.matchesView(null, machineOf(a.address)))) out.push(...withTree(a));
    return out;
  }

  // The local machine's $host label: the one vegad wrote on a local terminal.
  localHost(live) {
    return live.find(x => !x.address.includes("/") && x.labels?.["vega.host"])?.labels["vega.host"] ?? "";
  }

  // Drag one project above another; the order stays across restarts.
  move(projects, from, to) {
    const all = this.orderedProjects(this.projectGroups(terminals()).uacs);
    const visible = projects.filter(p => p !== from);
    visible.splice(visible.indexOf(to), 0, from);
    // Reorder only the visible slots, so hidden projects keep their positions.
    const selected = new Set(projects);
    this.order = all.map(p => selected.has(p) ? visible.shift() : p);
    try { localStorage.setItem("project-order", JSON.stringify(this.order)); } catch (_) {}
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
    if (props()?.projectConfig) return this.projectOverlay(t);
    if (props()?.viewMenu) return this.viewMenu(t);
    if (props()?.spaceMenu) return this.spaceMenu(t);
    const collapsed = narrow();

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
    const membership = this.projectGroups(all);
    list = list.filter(a => {
      const project = membership.projectOfAgent(a), uac = membership.uacs.get(project);
      return this.matchesView(uac?.labels?.["vega.project-id"], uac ? machineOf(uac.address) : machineOf(a.address));
    });
    const projectOrder = this.orderedProjects(membership.uacs);
    list.sort((a, b) => {
      const pa = membership.projectOfAgent(a), pb = membership.projectOfAgent(b);
      const ia = projectOrder.indexOf(pa), ib = projectOrder.indexOf(pb);
      return (ia < 0 ? Infinity : ia) - (ib < 0 ? Infinity : ib) ||
        (pa && pa === pb ? (membership.labelsOf(b.address)["vega.role"] === "uac") - (membership.labelsOf(a.address)["vega.role"] === "uac") : 0);
    });
    setAgentOrder(list.flatMap(a => [a.address, ...this.tree(all, a.address).filter(s => listed.has(s.t.address)).map(s => s.t.address)]));

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

    const railRow = (a, i) => h_flex().id(`ag-${a.address}`).h(26).px(8).flex_none().items_center().gap(4)
      .hover(el => el.bg(t.hover)).on_click(() => run("pane.focus", { pane: a.pane }))
      .when(this.vegaTerminals.get(a.address)?.labels?.["vega.role"] === "uac", el =>
        el.on_mouse_down("right", (_e, cx) => this.openProject(this.vegaTerminals.get(a.address).labels["vega.project-id"], cx)))
      .child(div().flex_1().min_w(0).whitespace_nowrap().text_right().text_color(t.muted).child(String(i + 1)))
      .child(h_flex().w(14).flex_none().items_center().justify_center().child(this.status(t, a, true)));

    this.rowLabel = a => detail.get(a.address) ?? label(a);
    this.activeView = localStorage.getItem("active-view") ?? "all";
    const rows = collapsed ? list.map(railRow) : this.projectTree(t, all, list, shown, now);

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
                .id("ag-views")
                .text_size(11)
                .text_color(t.dim)
                .hover((s) => s.text_color(t.foreground))
                .on_click(() => openOverlay({ mod: "vega-sidebar", kind: "popup", width: 400, height: 400, props: { viewMenu: true } }))
                .tooltip("Filter by labels and machines")
                .child(this.preferences?.views?.[this.activeView] ? this.activeView : "All"),
            ),
        ),
      )
      .when(Boolean(this.configError) && !collapsed, el => el.child(div().px(12).text_size(11).text_color(t.blocked).child(this.configError)))
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
