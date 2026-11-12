// Manually ordered projects and spaces; project preferences belong to vega.

import { View, div, svg } from "gpui-kit";
import { v_flex, h_flex, InputState, Input } from "gpui-base";
import { agents, terminals, panes, focused, spaces, run, theme, shared, setShared, setAgentOrder, openOverlay, closeOverlay, props, window, layout, exec, home, nextCommand } from "herdr";

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
// A sub-agent without a pane isn't in agents(): its raw word, error counting as blocked.
// Done is unread news (blue) only while the terminal's turn is past what the Slayer saw (label
// herdr.seen, kept by herdr-app); the tab bar shows it the same way. Seen, it reads as idle.
const seenDone = (term, s) => s === "done" && term?.turn != null && Number(term.labels?.["herdr.seen"]) >= term.turn ? "idle" : s;
const kidStatus = (k, listed) => {
  const s = seenDone(k, listed.get(k.address) ?? k.status);
  return s === "error" ? "blocked" : s;
};
// Later stages add tab descriptors and renderers here without changing the shell.
const PROJECT_TABS = [{ id: "overview", label: "Overview", render: "overviewTab" }, { id: "board", label: "Board", render: "boardTab" },
  { id: "decisions", label: "Decisions", render: "decisionsTab" },
  { id: "asks", label: "Asks", render: "asksTab" }, { id: "notes", label: "Notes", render: "notesTab" },
  { id: "uac", label: "UAC", render: "uacTab" },
  { id: "models", label: "Models", render: "modelsTab" },
  { id: "config", label: "Config", render: "configTab" }];

// A command in a project's checkout: locally via exec, on an hc machine through `hc exec --json`
// (base64 stdout/stderr, decoded in Python as vega-inbox does).
const remoteAction = `import sys,json,base64,subprocess
machine,cwd,program,*args=sys.argv[1:]
p=subprocess.run(['hc','exec','--json','--on',machine,'--cwd',cwd,'--timeout','30','--',program,*args],capture_output=True,text=True,timeout=35)
if p.returncode: print(json.dumps({'code':p.returncode,'stdout':p.stdout,'stderr':p.stderr}))
else:
 v=json.loads(p.stdout)
 print(json.dumps({'code':v['code'],'stdout':base64.b64decode(v['stdout'],validate=True).decode('utf-8',errors='replace'),'stderr':base64.b64decode(v['stderr'],validate=True).decode('utf-8',errors='replace')}))`;
async function inCheckout(machine, cwd, program, args) {
  if (!machine) return exec(program, args, { cwd, timeout: 15000 });
  const r = await exec("python3", ["-c", remoteAction, machine, cwd, program, ...args]);
  return r.code === 0 ? JSON.parse(r.stdout) : r;
}
const jsonOf = out => JSON.parse(out.slice(Math.min(...["[", "{"].map(c => out.indexOf(c)).filter(i => i >= 0))));
const values = input => [...new Set(input.split(",").map(s => s.trim()).filter(Boolean))];
// An input inside the project window: the window refocuses its own handle on every left press
// (for Esc), so the press must stop here or the input loses the focus it just took.
const inputBox = (t, state, h = 32) => div().on_mouse_down("left", (_e, cx) => cx.stop_propagation())
  .child(Input.new(state).h(h).px(8).rounded(6).border_1().border_color(t.input).bg(t.background));
// A small pill: a label on a project, a view's filter, a toggle in an editor.
const pill = (t, id, text, { on = false, action = null, tooltip = null } = {}) => div().id(id).flex_none().px(6).rounded(8)
  .text_size(10.5).bg(on ? t.accent : t.raised).text_color(on ? t.background : t.dim)
  .when(Boolean(tooltip), el => el.tooltip(tooltip))
  .when(Boolean(action), el => el.cursor_pointer()
    .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
    .on_click((_e, cx) => { cx.stop_propagation(); action(cx); cx.notify(); }))
  .child(text);
// Toggle one entry in a comma-separated input (the editors keep their state in inputs).
const toggleValue = (state, value) => {
  const list = values(state.value());
  state.set_value((list.includes(value) ? list.filter(v => v !== value) : [...list, value]).join(", "));
};
// Ad-hoc filter by one label, set by clicking a label pill in the sidebar (no saved view).
const LABEL_VIEW = "label:";
// A waiting pane from a badge row as `vega mux dismiss` on that row's machine knows it: the
// vega mod qualifies remote panes as hc:<machine>/<pane>, the machine's records say hc:<pane>.
const dismissArgs = (row, pane) => ["mux", "dismiss", row?.machine ? pane.replace(`hc:${row.machine}/`, "hc:") : pane];
// The active view lives in shared state: localStorage is per mod copy, so the filter popup
// could not reach the sidebar through it. The sidebar copies it to its own localStorage to
// keep it across restarts; shared state is empty after a restart, so that copy is the fallback.
const VIEW_KEY = "vega.view";
const activeView = () => shared(VIEW_KEY) ?? localStorage.getItem("active-view") ?? "all";
const setActiveView = id => { setShared(VIEW_KEY, id); localStorage.setItem("active-view", id); };
const INACTIVE_KEY = "vega.hide-inactive";
const hideInactive = () => shared(INACTIVE_KEY) ?? localStorage.getItem("hide-inactive") === "true";
const setHideInactive = on => { setShared(INACTIVE_KEY, on); localStorage.setItem("hide-inactive", String(on)); };
const machineOf = address => address?.includes("/") ? address.split("/")[0] : "local";

function duration(ms) {
  const m = Math.floor(ms / 60000);
  if (m < 1) return "<1m";
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  return h < 24 ? `${h}h` : `${Math.floor(h / 24)}d`;
}

// Shared by all copies on herdr-app 0.1.14+. Overlays do not participate.
const navigation = { agents: [], spaces: [], panes: new Map(), commands: new Map(), sampled: null, polled: 0 };
function visit(history, id) {
  if (!id || history[0] === id) return;
  const index = history.indexOf(id);
  if (index >= 0) history.splice(index, 1);
  history.unshift(id);
}
function sampleFocus() {
  const f = focused();
  if (!f) return;
  visit(navigation.spaces, f.space);
  if (f.tab && f.pane) {
    if (!navigation.panes.has(f.tab)) navigation.panes.set(f.tab, []);
    visit(navigation.panes.get(f.tab), f.pane);
  }
  // The frequent path only reads focused(); copy agent inventory on transitions.
  if (f.terminal !== navigation.sampled) {
    navigation.sampled = f.terminal;
    if (agents().some(a => a.address === f.terminal)) visit(navigation.agents, f.terminal);
  }
}
function lastFocus(command) {
  sampleFocus();
  const f = focused();
  if (!f) return;
  let history, live, current, action, argument;
  // keys.json binds "vega-sidebar.<command>"; accept a delivered prefix too.
  command = String(command).replace(/^vega-sidebar[.:]/, "");
  if (command === "last-agent") {
    history = navigation.agents; live = new Set(agents().map(a => a.address));
    current = f.terminal; action = "agent.focus"; argument = "terminal";
  } else if (command === "last-space") {
    history = navigation.spaces; live = new Set(spaces().map(s => s.id));
    current = f.space; action = "space.select"; argument = "id";
  } else if (command === "last-pane") {
    history = navigation.panes.get(f.tab) ?? [];
    live = new Set(panes().filter(p => p.tab === f.tab).map(p => p.pane));
    current = f.pane; action = "pane.focus"; argument = "pane";
  } else return;
  history.splice(0, history.length, ...history.filter(id => live.has(id)));
  const target = history.find(id => id !== current);
  if (!target) return;
  run(action, { [argument]: target });
  // Capture actual focus, not an optimistic change if the command failed.
  sampleFocus();
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
        this.viewMatch = "all";
      }
      cx.spawn(async cx => {
        await this.refreshPreferences(cx);
        const config = this.preferences.projects?.[props()?.projectConfig];
        if (props()?.projectConfig && !this.configError) {
          this.nameInput = InputState.new({ value: config?.name ?? "", placeholder: "Default project name" });
          this.nameInput.on("submit", (_e, cx) => cx.spawn(cx => this.saveProject(cx)));
          this.labelsInput = InputState.new({ placeholder: "Add a label, Enter" });
          this.labelsInput.on("submit", (_e, cx) => cx.spawn(cx => this.addLabels(cx)));
          this.originalLabels = config?.labels ?? [];
          this.labels = [...this.originalLabels];
        }
        this.loading = false;
        cx.notify();
      });
      if (props()?.projectConfig) {
        // A window does not take the keyboard when it opens: own a focus handle so Esc works.
        this.focusHandle = cx.focus_handle?.();
        this.focusHandle?.focus();
        cx.spawn(cx => this.readOverview(cx));
        cx.spawn(cx => this.readAsks(cx));
        if (this.currentTab() === "models") cx.spawn(cx => this.readModels(cx));
        cx.timer.every(15000, cx => { this.readOverview(cx); this.readAsks(cx); if (this.currentTab() === "board") this.readBoard(cx); this.readNotes(cx); if (this.currentTab() === "models") this.readModels(cx); });
        cx.spawn(cx => this.readNotes(cx));
        if (props()?.follow) cx.timer.every(1000, () => this.followFocus());
        this.decisionInputs = new Map(); this.decisionBusy = new Set(); this.decisionResults = new Map(); this.decisionDone = new Set();
        this.askBusy = new Set(); this.askResults = new Map();
        this.dismissBusy = new Set(); this.dismissResults = new Map();
        this.expanded = new Set(); this.readyLimit = 12;
      }
      return;
    }
    if (props()?.spaceMenu) return;
    this.startNavigation(cx);
    this.preferences = { projects: {}, views: {} };
    this.activeView = activeView();
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

  startNavigation(cx) {
    // A key is broadcast to shown copies. Collect its promise continuations in
    // one microtask batch so exactly one acts, even when other copies are hidden
    // or removed. No retained owner/context can strand command handling.
    cx.spawn(async cx => {
      for (;;) {
        const command = await nextCommand();
        let delivery = navigation.commands.get(command);
        if (!delivery) navigation.commands.set(command, delivery = { handled: false });
        await Promise.resolve();
        if (!delivery.handled) {
          delivery.handled = true;
          navigation.commands.delete(command);
          lastFocus(command);
        }
      }
    });
    cx.spawn(async cx => {
      for (;;) {
        const now = Date.now();
        // Share the polling budget across hidden and visible copies. No redraw.
        if (now - navigation.polled >= 250) {
          navigation.polled = now;
          sampleFocus();
        }
        await cx.sleep(300);
      }
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

  async toggleProjectPin(project, cx) {
    if (this.saving) return;
    this.saving = true; this.configError = null; cx.notify();
    try {
      const pinned = this.preferences?.projects?.[project]?.pinned;
      const r = await exec("vega", ["project", "config", project, pinned ? "--unpin" : "--pin", "--json"]);
      if (r.code !== 0) throw new Error(r.stderr || r.stdout || "Project pin failed");
      await this.refreshPreferences(cx);
    } catch (e) { this.configError = String(e); }
    finally { this.saving = false; cx.notify(); }
  }

  // One draggable window per project (key: project ID) with its tabs. No terminal block:
  // a terminal rendered in this window and in its pane at once jitters (vega-0h4c.50).
  // A click on an agent row focuses that agent's terminal in the main view instead.
  openProject(project, cx, tab = null, follow = false) {
    cx?.stop_propagation?.();
    if (!project) return;
    const uac = terminals().find(x => x.state === "live" && x.labels?.["vega.role"] === "uac"
      && x.labels?.["vega.project-id"] === project);
    openOverlay({ kind: "window", key: follow ? "vega-project-follow" : `vega-project-${project}`, width: 640, height: 0.7,
      mod: "vega-sidebar", props: { projectConfig: project, uac: uac?.address ?? null, tab, follow } });
  }

  // Notes the human was shown (vega post notes): this project's, plus project-less ones
  // (quarantine, drafts) on the UAC pane they were raised for.
  async readNotes(cx) {
    if (this.readingNotes) return;
    this.readingNotes = true;
    let notes;
    try {
      const r = await exec("vega", ["post", "notes", "--json"], { timeout: 10000 });
      if (r.code !== 0) throw new Error((r.stderr || r.stdout || "notes failed").trim());
      notes = r.stdout.split("\n").filter(l => l.trim().startsWith("{")).map(l => JSON.parse(l))
        .filter(n => n.project === props().projectConfig || (!n.project && n.pane === props().uac));
    } catch (e) { notes = String(e.message ?? e); }
    this.readingNotes = false;
    const sig = JSON.stringify(notes);
    if (sig !== this.notesSig) { this.notesSig = sig; this.notes = notes; cx.notify(); }
  }

  async markNotes(action, cx) {
    const list = Array.isArray(this.notes) ? this.notes : [];
    const upto = {};
    for (const n of list) if (!upto[n.host] || n.id > upto[n.host]) upto[n.host] = n.id;
    const args = ["post", "notes", action];
    for (const [host, id] of Object.entries(upto)) args.push("--upto", `${host}=${id}`);
    if (!Object.keys(upto).length) return;
    const r = await exec("vega", args, { timeout: 10000 });
    this.notesResult = r.code === 0 ? (action === "read" ? "Marked read." : "Cleared.") : (r.stderr || r.stdout || "failed").trim();
    this.notesSig = null;
    await this.readNotes(cx);
    cx.notify();
  }

  notesTab(t) {
    if (this.notes === undefined) return div().text_color(t.dim).child("Reading notes…");
    if (typeof this.notes === "string") return div().text_color(t.blocked).child(this.notes);
    const button = (id, label, action) => div().id(id).px(10).py(4).rounded(6).bg(t.raised).cursor_pointer().child(label)
      .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
      .on_click((_e, cx) => { cx.stop_propagation(); cx.spawn(cx => action(cx)); });
    if (!this.notes.length) return v_flex().items_center().gap(4).py(12).child(div().child("No notes"))
      .child(div().text_size(11).text_color(t.dim).child(this.notesResult ?? "Nothing was shown to you for this project."));
    return v_flex().gap(8)
      .child(h_flex().gap(8).items_center()
        .child(button("notes-read", "Mark all read", cx => this.markNotes("read", cx)))
        .child(button("notes-clear", "Clear", cx => this.markNotes("clear", cx)))
        .when(Boolean(this.notesResult), el => el.child(div().text_size(11).text_color(t.dim).child(this.notesResult))))
      .children(this.notes.map(n => v_flex().id(`note-${n.host}-${n.id}`).gap(4).p(10).rounded(8).bg(t.surface)
        .child(h_flex().gap(6).items_center()
          .child(div().w(8).h(8).rounded(4).flex_none().bg(n.unread ? t.blocked : t.dim))
          .child(div().text_size(11).text_color(t.dim).child(`${n.kind ?? "note"} · ${n.host} · ${new Date(n.id).toLocaleString()}`)))
        .child(div().font_semibold().child(n.title ?? ""))
        .child(div().text_size(11.5).text_color(t.foreground).child((n.body ?? "").slice(0, 800)))));
  }

  // Shared across project/config windows; never infer a default from a host's effective
  // row: host and project pins can mask it. The default reader is the CLI's pin view.
  async readModels(cx) {
    if (this.modelsRead) return this.modelsRead;
    this.modelsRead = this.loadModels(cx);
    try { await this.modelsRead; } finally { this.modelsRead = null; }
  }

  async loadModels(cx) {
    try {
      const r = await exec("vega", ["lanista", "hosts", "--json"], { timeout: 10000 });
      if (r.code !== 0) throw new Error((r.stdout || r.stderr || `hosts failed (${r.code})`).trim());
      const data = JSON.parse(r.stdout);
      if (!Array.isArray(data.rows)) throw new Error("Invalid lanista hosts response: rows missing");
      const cells = [...new Map(data.rows.filter(r => r.lane !== "pi").map(r => [`${r.seat}/${r.lane}/${r.tier}`, r])).values()];
      const defaults = await Promise.all(cells.map(async row => {
        try {
          const r = await exec("vega", this.modelArgs(row, true), { timeout: 10000 });
          if (r.code !== 0) throw new Error((r.stdout || r.stderr || `default read failed (${r.code})`).trim());
          const field = name => r.stdout.match(new RegExp(`^${name}: (.+)$`, "m"))?.[1];
          const model = field("model"), source = field("source"), expires = field("expires");
          if (!model || !source || !expires) throw new Error("Invalid lanista default response");
          return {...row, model, source: source === "global seats" ? "global" : source, until: expires === "never" ? null : Number(expires)};
        } catch (e) { return {...row, model: "Unknown", source: "unknown", error: String(e.message ?? e)}; }
      }));
      setShared("vega.models", { rows: data.rows, defaults });
      this.modelsError = null;
    } catch (e) { this.modelsError = String(e.message ?? e); }
    finally { cx.notify(); }
  }

  modelArgs(row, defaults) {
    const args = ["lanista", "pin", row.lane, "--tier", row.tier];
    if (row.seat !== "herdr-agents") args.push("--seat", row.seat);
    return args.concat(defaults ? ["--default"] : ["--host", row.host]);
  }

  async changeModel(row, defaults, model, cx) {
    if (shared("vega.models-busy")) return;
    if (defaults && row.lane === "pi") return;
    if (model !== null && !row.allowed.includes(model)) return;
    setShared("vega.models-busy", true);
    this.modelResult = null; this.modelActionError = false; this.modelMenu = null; cx.notify();
    try {
      const r = await exec("vega", [...this.modelArgs(row, defaults), ...(model === null ? ["--clear"] : ["--set", model])], { timeout: 10000 });
      if (r.code !== 0) throw new Error((r.stdout || r.stderr || `pin failed (${r.code})`).trim());
      this.modelResult = model === null ? "Pin cleared." : "Model saved.";
    } catch (e) { this.modelResult = String(e.message ?? e); this.modelActionError = true; }
    finally {
      // Finish a poll that started before the mutation, then request a fresh snapshot.
      if (this.modelsRead) await this.modelsRead;
      await this.readModels(cx);
      setShared("vega.models-busy", false);
      cx.notify();
    }
  }

  modelsTab(t) {
    const data = shared("vega.models");
    const button = (id, label, action) => div().id(id).px(6).py(3).rounded(4).bg(t.raised).cursor_pointer()
      .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
      .on_click((_e, cx) => { cx.stop_propagation(); action(cx); cx.notify(); }).child(label);
    const body = v_flex().gap(8)
      .child(button("models-refresh", "Refresh", cx => cx.spawn(cx => this.readModels(cx))))
      .when(Boolean(this.modelsError), el => el.child(div().text_color(t.blocked).child(this.modelsError)))
      .when(Boolean(this.modelResult), el => el.child(div().text_color(this.modelActionError ? t.blocked : t.done).child(this.modelResult)));
    if (!data) return body.child(div().text_color(t.dim).child(this.modelsError ? "Models unavailable" : "Reading models…"));
    if (!data.rows.length) return body.child(div().text_color(t.dim).child("No known hosts"));
    const groups = [{ name: "Default", rows: data.defaults, defaults: true },
      ...[...new Set(data.rows.map(r => r.host))].sort().map(host => ({ name: host, rows: data.rows.filter(r => r.host === host), defaults: false }))];
    return body.children(groups.map(group => v_flex().id(`models-group-${group.name}`).gap(6).p(8).rounded(6).bg(t.surface)
      .child(div().font_semibold().child(group.name))
      .when(group.defaults, el => el.child(div().text_size(11).text_color(t.dim).child("Pi inherits zai defaults.")))
      .children(group.rows.map(row => {
        const key = `${group.defaults ? "default" : row.host}/${row.seat}/${row.lane}/${row.tier}`;
        return v_flex().id(`model-${key}`).gap(3)
          .child(h_flex().gap(6).items_center()
            .child(div().w(130).text_color(t.dim).child(`${row.lane} · ${row.tier}`))
            .child(div().flex_1().child(`${row.model} · ${row.source}`)))
          .child(div().text_size(10).text_color(t.dim).child(`${row.seat} · expires ${row.until == null ? "never" : new Date(row.until * 1000).toLocaleString()}`))
          .when(Boolean(row.error), el => el.child(div().text_color(t.blocked).child(row.error)))
          .child(h_flex().gap(6)
            .child(button(`model-select-${key}`, "Choose model ▾", () => { this.modelMenu = this.modelMenu === key ? null : key; }))
            .child(button(`model-clear-${key}`, "Reset", cx => cx.spawn(cx => this.changeModel(row, group.defaults, null, cx)))))
          .when(this.modelMenu === key, el => el.child(v_flex().gap(2).p(4).bg(t.raised)
            .children(row.allowed.map(model => button(`model-option-${key}/${model}`, model,
              cx => cx.spawn(cx => this.changeModel(row, group.defaults, model, cx)))))));
      }))));
  }

  // Open asks (vega notify) of this project from every host. Read locally for every project:
  // `notify list` queries the registry, and ack/resolve reach the origin host over NATS.
  async readAsks(cx) {
    if (this.readingAsks || !props()?.projectConfig) return;
    this.readingAsks = true;
    let asks;
    try {
      const r = await exec("vega", ["notify", "list", "--json", "--project", props().projectConfig], { timeout: 10000 });
      const v = r.code === 0 ? jsonOf(r.stdout) : null;
      asks = v ? { list: v.asks ?? [], error: v.registry_error ?? null } : (r.stderr || r.stdout || `vega notify failed (${r.code})`).trim();
    } catch (e) { asks = String(e); }
    this.readingAsks = false;
    const sig = JSON.stringify(asks);
    if (sig !== this.asksSig) { this.asksSig = sig; this.asks = asks; cx.notify(); }
  }

  // The user handled a waiting child in its pane: `vega mux dismiss` where its parent record is
  // (the badge row's machine), so its own turns stop raising the hand (vega-0h4c.56).
  async dismissWaiting(row, w, cx) {
    if (this.dismissBusy.has(w.pane)) return;
    this.dismissBusy.add(w.pane); this.dismissResults.delete(w.pane); cx.notify();
    try {
      const r = await inCheckout(row?.machine || null, row?.checkout || "/", "vega", dismissArgs(row, w.pane));
      this.dismissResults.set(w.pane, r.code === 0 ? "Dismissed; the hand goes with the next refresh."
        : (r.stderr || r.stdout || `Failed (${r.code})`).trim().split("\n")[0]);
    } catch (e) { this.dismissResults.set(w.pane, String(e)); }
    finally { this.dismissBusy.delete(w.pane); cx.notify(); }
  }

  // Ack keeps the ask open and stops escalation; resolve closes it. argv only, no shell.
  async askAct(a, action, cx) {
    if (this.askBusy.has(a.id)) return;
    this.askBusy.add(a.id); this.askResults.delete(a.id); cx.notify();
    try {
      const r = await exec("vega", ["notify", action, a.id], { timeout: 15000 });
      this.askResults.set(a.id, r.code === 0 ? (action === "ack" ? "Acknowledged." : "Resolved.") : (r.stderr || r.stdout || `Failed (${r.code})`).trim().split("\n")[0]);
    } catch (e) { this.askResults.set(a.id, String(e)); }
    finally { this.askBusy.delete(a.id); }
    this.asksSig = null;
    await this.readAsks(cx);
    cx.notify();
  }

  asksTab(t) {
    if (this.asks === undefined) return div().text_color(t.dim).child("Reading asks…");
    if (typeof this.asks === "string") return div().text_color(t.blocked).child(this.asks.split("\n")[0].slice(0, 200));
    const button = (id, label, action) => div().id(id).px(10).py(4).rounded(6).bg(t.raised).cursor_pointer().child(label)
      .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
      .on_click((_e, cx) => { cx.stop_propagation(); cx.spawn(cx => action(cx)); });
    const levelColor = l => ({ urgent: t.blocked, action: t.ansi3 ?? t.working, info: t.accent })[l] ?? t.dim;
    const warn = this.asks.error ? div().text_size(11).text_color(t.blocked).child(`Registry: ${this.asks.error}`.slice(0, 200)) : null;
    if (!this.asks.list.length) return v_flex().items_center().gap(4).py(12).child(div().child("No open asks"))
      .child(div().text_size(11).text_color(t.dim).child("Nothing in this project waits on you through vega notify."))
      .when(Boolean(warn), el => el.child(warn));
    return v_flex().gap(8).when(Boolean(warn), el => el.child(warn))
      .children(this.asks.list.map(a => {
        const busy = this.askBusy.has(a.id), result = this.askResults.get(a.id);
        const pane = a.pane?.replace(/^hc:/, "");
        return v_flex().id(`ask-${a.id}`).gap(6).p(10).rounded(8).bg(t.surface)
          .child(h_flex().gap(6).items_center()
            .child(div().w(8).h(8).rounded(4).flex_none().bg(a.acknowledged ? t.dim : levelColor(a.level)))
            .child(div().text_size(11).text_color(t.dim).child([a.level, a.host, a.pane, a.bead, a.acknowledged ? "acknowledged" : null].filter(Boolean).join(" · "))))
          .child(div().font_semibold().child(a.line ?? ""))
          .when(Boolean(a.cmd), el => el.child(div().text_size(11).text_color(t.dim).child(`$ ${a.cmd}`)))
          .child(h_flex().gap(8).items_center()
            .when(Boolean(pane), el => el.child(button(`ask-focus-${a.id}`, "Go to pane", () => run("pane.focus", { terminal: pane }))))
            .when(!a.acknowledged, el => el.child(button(`ask-ack-${a.id}`, busy ? "…" : "Ack", cx => this.askAct(a, "ack", cx))))
            .child(button(`ask-resolve-${a.id}`, busy ? "…" : "Resolve", cx => this.askAct(a, "resolve", cx)))
            .when(Boolean(result), el => el.child(div().text_size(11).text_color(t.dim).child(result))));
      }));
  }

  // Overview data the terminal labels do not carry. Beads counts come from `bd status --json`
  // in the UAC's checkout; remote projects wait for the corp-host check (vega-0h4c.50.2).
  async readOverview(cx) {
    const uac = terminals().find(x => x.address === props()?.uac);
    if (!uac || this.readingOverview) return;
    if (uac.address.includes("/")) { this.beads = "remote"; cx.notify(); return; }
    this.readingOverview = true;
    // The registry checkout first: the terminal's cwd is its login shell's folder and drifts.
    const cwd = this.badgeRow().checkout;
    let beads;
    try {
      const r = await exec("bd", ["status", "--json"], { cwd, timeout: 10000 });
      // Tolerate log lines before the JSON object.
      beads = r.code === 0 ? JSON.parse(r.stdout.slice(r.stdout.indexOf("{"))).summary : (r.stderr || "bd failed").trim();
    } catch (e) { beads = String(e); }
    // vega's --json views (vega-0h4c.50.4), run in the UAC's checkout so they pick its project.
    const vega = async args => {
      try {
        const r = await exec("vega", [...args, "--json"], { cwd, timeout: 10000 });
        return r.code === 0 ? JSON.parse(r.stdout.slice(r.stdout.indexOf("{"))) : null;
      } catch { return null; }
    };
    const [ci, capacity, facility] = await Promise.all([vega(["ci"]), vega(["capacity"]), vega(["uac"])]);
    const ov = { watches: ci?.watches ?? null,
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
        .on_mouse_down("left", (_e, cx) => cx.stop_propagation()).on_click((_e, cx) => { action(); cx.notify(); }))
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
    const asks = typeof this.asks === "object" && this.asks ? this.asks.list : null;
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
      .child(section("Needs you", (totals.decisions ?? 0) + (totals.waiting ?? 0) + (totals.notes ?? 0) + (asks?.length ?? 0)))
      .child(h_flex().gap(8)
        .child(tile("decisions", "⚖", totals.decisions ?? 0, "decisions", t.blocked, inbox))
        .child(tile("asks", "⚠", asks?.length ?? 0, "asks", t.blocked, () => { this.projectTab = "asks"; }))
        .child(tile("waiting", "✋", totals.waiting ?? 0, "waiting on you", t.blocked, waiting))
        .child(tile("notes", "✉", totals.notes ?? 0, "notes", t.accent, notes)))
      .when(Boolean(asks?.length), el => el.child(v_flex().gap(2).children(asks.slice(0, 5).map((a, i) =>
        div().id(`ov-ask-${i}`).px(8).text_size(11.5).text_color(t.foreground).child(`⚠ ${a.line ?? ""}`)))))
      .when(Boolean(totals.waiting_panes?.length), el => el.child(v_flex().gap(2).children(totals.waiting_panes.map(w => {
        const result = this.dismissResults?.get(w.pane);
        const small = (id, label, tip, action) => div().id(id).flex_none().px(6).py(1).rounded(4).cursor_pointer().bg(t.raised)
          .text_size(11).tooltip(tip).on_mouse_down("left", (_e, cx) => cx.stop_propagation())
          .on_click((_e, cx) => { cx.stop_propagation(); action(cx); }).child(label);
        return h_flex().id(`ov-wait-${w.pane}`).gap(6).px(8).items_center().text_size(11.5)
          .child(div().flex_none().child("✋"))
          .child(div().flex_1().min_w(0).truncate().child(`${w.name || w.pane} · ${w.title || w.attention}`))
          .when(Boolean(result), el => el.child(div().flex_none().text_size(11).text_color(t.dim).child(result)))
          .child(small(`ov-wait-go-${w.pane}`, "Go", "Focus the pane", () => run("pane.focus", { terminal: w.pane.replace(/^hc:/, "") })))
          // A dialog (input) is live and an open ask (unanswered) closes elsewhere: not dismissable.
          .when(w.attention === "silent" || w.attention === "asking", el => el.child(small(`ov-wait-dismiss-${w.pane}`,
            this.dismissBusy?.has(w.pane) ? "…" : "Dismiss", "Handled: drop the hand until the parent briefs it again",
            cx => cx.spawn(cx => this.dismissWaiting(totals, w, cx)))));
      }))))
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

  currentTab() { return this.projectTab ?? props()?.tab ?? PROJECT_TABS[0].id; }

  // The badge row of this project (machine, checkout, decision texts), written by the vega mod.
  badgeRow() {
    const uac = terminals().find(x => x.address === props()?.uac);
    const machine = uac?.address?.includes("/") ? uac.address.split("/")[0] : "";
    const row = (shared("vega.badges") ?? []).find(r => r.project === props()?.projectConfig && (r.machine || "") === machine);
    return { machine: machine || null, checkout: row?.checkout ?? uac?.cwd ?? null, row };
  }

  async readBoard(cx) {
    if (this.readingBoard) return;
    const { machine, checkout } = this.badgeRow();
    if (!checkout) { this.board = "No checkout known for this project."; cx.notify(); return; }
    this.readingBoard = true;
    const bd = async args => {
      const r = await inCheckout(machine, checkout, "bd", [...args, "--json"]);
      if (r.code !== 0) throw new Error((r.stderr || r.stdout || `bd ${args[0]} failed`).trim().split("\n")[0]);
      return jsonOf(r.stdout);
    };
    let board;
    try {
      const [progress, ready, blocked] = await Promise.all([bd(["list", "--status", "in_progress"]), bd(["ready", "--limit", "50"]), bd(["blocked"])]);
      board = { progress, ready, blocked };
    } catch (e) { board = String(e.message ?? e); }
    this.readingBoard = false;
    const sig = JSON.stringify(board);
    if (sig !== this.boardSig) { this.boardSig = sig; this.board = board; cx.notify(); }
  }

  boardTab(t) {
    const section = (label, n) => h_flex().gap(8).items_center().pt(4)
      .child(div().text_size(10.5).font_semibold().text_color(t.dim).child(`${label.toUpperCase()} ${n}`))
      .child(div().flex_1().h(1).bg(t.border));
    if (this.board === undefined) return div().text_color(t.dim).child("Reading beads…");
    if (typeof this.board === "string") return v_flex().items_center().gap(6).py(12)
      .child(div().child("Beads unavailable")).child(div().text_size(11).text_color(t.dim).child(this.board.slice(0, 160)))
      .child(div().id("board-retry").px(10).py(4).rounded(6).cursor_pointer().bg(t.raised)
        .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
        .on_click((_e, cx) => { this.boardSig = null; cx.spawn(cx => this.readBoard(cx)); }).child("Retry"));
    const bead = (b, extra, color) => {
      const open = this.expanded.has(b.id);
      return v_flex().id(`bead-${b.id}`).px(8).py(4).rounded(6).cursor_pointer().when(open, el => el.bg(t.surface))
        .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
        .on_click((_e, cx) => { open ? this.expanded.delete(b.id) : this.expanded.add(b.id); cx.notify(); })
        .child(h_flex().gap(8).items_center()
          .child(div().w(8).h(8).rounded(4).flex_none().bg(color))
          .child(div().w(22).flex_none().text_color(t.dim).child(`P${b.priority ?? "?"}`))
          .child(div().w(110).flex_none().text_color(t.dim).whitespace_nowrap().child(b.id))
          .child(div().flex_1().min_w(0).whitespace_nowrap().child(b.title ?? ""))
          .when(Boolean(extra), el => el.child(div().flex_none().text_size(11).text_color(t.dim).child(extra))))
        .when(open, el => el.child(v_flex().gap(4).pt(6).pl(16).text_size(11.5)
          .child(div().text_color(t.foreground).child((b.description ?? "").slice(0, 600)))
          .when(Boolean(b.acceptance_criteria), el => el.child(div().text_color(t.dim).child(`Done when: ${b.acceptance_criteria.slice(0, 300)}`)))));
    };
    const { progress = [], ready = [], blocked = [] } = this.board;
    const shown = ready.slice(0, this.readyLimit);
    return v_flex().id("board-body").gap(4)
      .child(section("In progress", progress.length))
      .children(progress.length ? progress.map(b => bead(b, (b.assignee ?? "").split(".").pop(), t.working)) : [div().px(8).text_size(11).text_color(t.dim).child("Nothing in progress.")])
      .child(section("Ready", ready.length))
      .children(shown.map(b => bead(b, b.issue_type, t.done)))
      .when(ready.length > shown.length, el => el.child(div().id("board-more").px(8).py(4).cursor_pointer().text_color(t.accent)
        .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
        .on_click((_e, cx) => { this.readyLimit += 20; cx.notify(); }).child(`Show more (${ready.length - shown.length})`)))
      .child(section("Blocked", blocked.length))
      .children(blocked.slice(0, 20).map(b => bead(b, `← ${(b.blocked_by ?? []).slice(0, 2).join(", ")}`, t.blocked)));
  }

  // Answer or dismiss a decision, as vega-inbox does: `vega uac answer` or `bd close --reason`.
  decide(d, dismiss, cx) {
    const { machine, checkout } = this.badgeRow();
    const text = this.decisionInputs.get(d.id)?.value().trim() ?? "";
    if (this.decisionBusy.has(d.id)) return;
    if (!checkout || !text) { this.decisionResults.set(d.id, !checkout ? "Project checkout unavailable." : "Enter an answer or dismissal reason."); cx.notify(); return; }
    this.decisionBusy.add(d.id); this.decisionResults.delete(d.id); cx.notify();
    cx.spawn(async cx => {
      try {
        const r = dismiss ? await inCheckout(machine, checkout, "bd", ["close", d.id, "--reason", text])
          : await inCheckout(machine, checkout, "vega", ["uac", "answer", d.id, text]);
        if (r.code === 0) this.decisionDone.add(d.id);
        this.decisionResults.set(d.id, r.code === 0 ? (dismiss ? "Dismissed." : "Answer sent to the UAC.") : (r.stderr || r.stdout || `Failed (${r.code})`).trim());
      } catch (e) { this.decisionResults.set(d.id, String(e)); }
      finally { this.decisionBusy.delete(d.id); cx.notify(); }
    });
  }

  decisionsTab(t) {
    const { row } = this.badgeRow();
    const list = (row?.decision_texts ?? []).filter(d => !this.decisionDone.has(d.id));
    const done = [...this.decisionResults.entries()].filter(([id]) => this.decisionDone.has(id));
    if (!list.length) return v_flex().items_center().gap(4).py(12)
      .child(div().child("No open decisions"))
      .child(div().text_size(11).text_color(t.dim).child(row ? "The UAC has nothing waiting for you." : "No badge data for this project yet."))
      .children(done.map(([id, msg]) => div().text_size(11).text_color(t.dim).child(`${id}: ${msg}`)));
    const button = (id, label, action) => div().id(id).px(12).py(5).rounded(6).bg(t.raised).cursor_pointer().child(label)
      .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
      .on_click((_e, cx) => { cx.stop_propagation(); action(cx); });
    return v_flex().id("decisions-body").gap(10).children(list.map(d => {
      if (!this.decisionInputs.has(d.id)) {
        const input = InputState.new({ placeholder: "Your answer (or dismissal reason)…" });
        this.decisionInputs.set(d.id, input);
      }
      return v_flex().id(`decision-${d.id}`).gap(6).p(10).rounded(8).bg(t.surface)
        .child(div().text_size(11).text_color(t.dim).child(d.id))
        .child(div().font_semibold().child(d.title ?? ""))
        .child(div().child(d.question || "No question text provided."))
        .child(inputBox(t, this.decisionInputs.get(d.id)))
        .child(h_flex().gap(8)
          .child(button(`answer-${d.id}`, this.decisionBusy.has(d.id) ? "Sending…" : "Answer", cx => this.decide(d, false, cx)))
          .child(button(`dismiss-${d.id}`, "Dismiss", cx => this.decide(d, true, cx))))
        .when(this.decisionResults.has(d.id), el => el.child(div().text_size(11).text_color(t.dim).child(this.decisionResults.get(d.id))));
    }));
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
    this.saved = false;
    const labels = [...this.labels];
    const args = ["project", "config", props().projectConfig, "--name", this.nameInput.value().trim()];
    for (const label of labels) args.push("--label", label);
    for (const label of this.originalLabels) if (!labels.includes(label)) args.push("--unlabel", label);
    args.push("--json");
    try {
      const r = await exec("vega", args);
      if (r.code !== 0) throw new Error(r.stderr || r.stdout || "Save failed");
      const saved = JSON.parse(r.stdout).projects[props().projectConfig];
      this.originalLabels = saved.labels;
      this.labels = [...saved.labels];
      this.saved = true;
      await this.refreshPreferences(cx);
    } catch (e) { this.configError = String(e); }
    finally { this.saving = false; cx.notify(); }
  }

  // Labels save at once: add from the input (commas split), remove by a pill's ✕.
  async addLabels(cx) {
    const add = values(this.labelsInput.value()).filter(l => !this.labels.includes(l));
    this.labelsInput.set_value("");
    if (!add.length) return;
    this.labels.push(...add);
    await this.saveProject(cx);
  }

  async setLabel(label, on, cx) {
    if (on === this.labels.includes(label)) return;
    this.labels = on ? [...this.labels, label] : this.labels.filter(l => l !== label);
    await this.saveProject(cx);
  }

  configTab(t) {
    const heading = label => div().text_size(11).text_color(t.dim).child(label);
    const button = (id, label, action) => div().id(id).flex_none().px(10).py(5).rounded(6).cursor_pointer().bg(t.raised)
      .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
      .on_click((_e, cx) => { cx.stop_propagation(); cx.spawn(cx => action(cx)); }).child(label);
    // Labels other projects use, offered as one-click additions.
    const known = [...new Set(Object.values(this.preferences?.projects ?? {}).flatMap(p => p.labels ?? []))]
      .filter(l => !this.labels.includes(l)).sort();
    return v_flex().gap(14)
      .child(v_flex().gap(5).child(heading("Display name (empty uses the project name)"))
        .child(h_flex().gap(8).items_center()
          .child(div().flex_1().child(inputBox(t, this.nameInput)))
          .child(button("save-project", this.saving ? "Saving…" : "Save", cx => this.saveProject(cx)))))
      .child(v_flex().gap(6).child(heading("Labels · group and filter projects in the sidebar"))
        .child(h_flex().gap(6).flex_wrap().items_center()
          .children(this.labels.length ? this.labels.map(l => pill(t, `label-${l}`, `${l}  ✕`, { on: true,
            tooltip: "Remove", action: cx => cx.spawn(cx => this.setLabel(l, false, cx)) }))
            : [div().text_size(11).text_color(t.dim).child("No labels yet.")]))
        .child(h_flex().gap(8).items_center()
          .child(div().flex_1().child(inputBox(t, this.labelsInput)))
          .child(button("add-label", "Add", cx => this.addLabels(cx))))
        .when(known.length > 0, el => el.child(h_flex().gap(6).flex_wrap().items_center()
          .child(div().text_size(11).text_color(t.dim).child("Used elsewhere:"))
          .children(known.map(l => pill(t, `known-label-${l}`, `+ ${l}`, { action: cx => cx.spawn(cx => this.setLabel(l, true, cx)) }))))))
      .when(Boolean(this.saved), el => el.child(div().text_size(11).text_color(t.dim).child("Saved")));
  }

  // Follow mode: one shared window (key vega-project-follow) that reopens in place on the
  // focused terminal's project. Pinned: one window per project (key vega-project-<id>).
  followFocus() {
    if (!props()?.follow) return;
    const all = terminals().filter(x => x.state === "live");
    const term = all.find(x => x.address === focused()?.terminal);
    const project = term?.labels?.["vega.project-id"];
    if (project && project !== props().projectConfig) this.openProject(project, null, this.currentTab(), true);
  }

  togglePin() {
    const project = props().projectConfig, tab = this.currentTab();
    closeOverlay(props().overlay);
    this.openProject(project, null, tab, !props().follow);
  }

  projectOverlay(t) {
    const tab = PROJECT_TABS.find(tab => tab.id === (this.projectTab ?? props()?.tab)) ?? PROJECT_TABS[0];
    const uac = terminals().find(x => x.address === props().uac);
    const name = this.preferences?.projects?.[props().projectConfig]?.name || uac?.labels?.["vega.project"] || props().projectConfig.split("/").pop();
    const stop = el => el.on_mouse_down("left", (_e, cx) => cx.stop_propagation());
    return v_flex().size_full().p(12).gap(10).bg(t.popover).text_color(t.foreground).text_size(12.8)
      .when(Boolean(this.focusHandle), el => el.track_focus(this.focusHandle)
        .on_mouse_down("left", () => this.focusHandle.focus()))
      .on_key_down((e, cx) => { if (e.keystroke === "escape") { cx.stop_propagation(); closeOverlay(props().overlay); } })
      // Title bar: presses here are not stopped, so they drag the window (kind "window").
      .child(h_flex().id("project-titlebar").gap(8).items_center().h(26).px(6).rounded(6).bg(t.surface)
        .child(div().text_color(t.dim).child("⠿"))
        .child(div().font_semibold().child(name))
        .child(div().text_size(11).text_color(t.dim).truncate().child(props().projectConfig))
        .child(div().flex_1())
        .child(stop(div().id("project-pin").px(6).py(2).rounded(4).cursor_pointer().bg(t.raised)
          .tooltip(props().follow ? "Following the focused project: click to pin this one" : "Pinned to this project: click to follow the focus")
          .on_click(() => this.togglePin()).child(props().follow ? "↺ follow" : "📌 pinned")))
        .child(stop(div().id("close-project").px(6).py(2).rounded(4).cursor_pointer()
          .tooltip("Close (Esc)").on_click(() => closeOverlay(props().overlay)).child("✕"))))
      .child(h_flex().gap(8).children(PROJECT_TABS.map(item => div().id(`project-tab-${item.id}`)
        .px(8).py(5).rounded(6).cursor_pointer().when(item.id === tab.id, el => el.bg(t.raised))
        .on_click((_e, cx) => { this.projectTab = item.id; if (item.id === "models") cx.spawn(cx => this.readModels(cx)); if (item.id === "board") cx.spawn(cx => this.readBoard(cx)); if (item.id === "notes") cx.spawn(cx => this.readNotes(cx)); cx.notify(); })
        .child(item.id === "notes" && this.notes?.length ? `${item.label} ${this.notes.filter(n => n.unread).length || ""}`.trim() : item.label))))
      .when(Boolean(this.configError), el => el.child(div().text_color(t.blocked).child(this.configError)))
      .child(v_flex().id(`project-body-${tab.id}`).flex_1().min_h(0).overflow_y_scrollbar()
        .child(tab.id !== "config" ? this[tab.render](t) : this.loading ? div().child("Loading…") : this.nameInput ? this[tab.render](t) : div().child("Config unavailable")));
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
      args.push("--match", this.viewMatch === "any" ? "any" : "all");
    }
    args.push("--json");
    try {
      const r = await exec("vega", args);
      if (r.code !== 0) throw new Error(r.stderr || r.stdout || "Save view failed");
      // A saved view is shown at once; deleting the shown view goes back to All.
      if (!remove) setActiveView(id);
      else if (activeView() === id) setActiveView("all");
      await this.refreshPreferences(cx);
    } catch (e) { this.configError = String(e); }
    finally { this.saving = false; cx.notify(); }
  }

  viewMenu(t) {
    const select = (id) => { setActiveView(id); closeOverlay(props().overlay); };
    const active = activeView();
    const heading = label => div().pt(4).text_size(10.5).font_semibold().text_color(t.dim).child(label.toUpperCase());
    const link = (id, label, action) => div().id(id).flex_none().px(6).py(2).rounded(4).cursor_pointer().text_size(11)
      .text_color(t.dim).hover(s => s.bg(t.raised).text_color(t.foreground)).on_click(action).child(label);
    const choice = (id, label, on, detail, action) => h_flex().id(id).gap(8).items_center().px(8).py(5).rounded(6)
      .cursor_pointer().when(on, el => el.bg(t.raised)).hover(s => s.bg(t.raised)).on_click(action)
      .child(div().w(12).flex_none().text_color(t.accent).child(on ? "✓" : ""))
      .child(div().flex_none().child(label))
      .child(h_flex().flex_1().min_w(0).gap(4).flex_wrap().children(detail));
    // Every label on any project and every machine with a terminal, offered as toggles.
    const labels = [...new Set(Object.values(this.preferences.projects ?? {}).flatMap(p => p.labels ?? []))].sort();
    const machines = [...new Set(["local", ...terminals().map(x => machineOf(x.address))])].sort();
    const chosenLabels = values(this.viewLabels.value()), chosenMachines = values(this.viewMachines.value());
    const toggles = (kind, all, chosen, state) => h_flex().gap(6).flex_wrap().items_center()
      .children(all.length ? all.map(v => pill(t, `pick-${kind}-${v}`, v, { on: chosen.includes(v), action: () => toggleValue(state, v) }))
        : [div().text_size(11).text_color(t.dim).child(kind === "label" ? "No labels yet: add them in a project's Config tab." : "")]);
    return v_flex().size_full().overflow_y_scrollbar().p(12).gap(6).bg(t.popover).text_color(t.foreground).text_size(12.8)
      .on_key_down((e, cx) => { if (e.keystroke === "escape") { cx.stop_propagation(); closeOverlay(props().overlay); } })
      .child(heading("Show"))
      .child(choice("view-all", "All", active === "all", [], () => select("all")))
      .children(labels.map(l => choice(`view-label-${l}`, `#${l}`, active === LABEL_VIEW + l, [], () => select(LABEL_VIEW + l))))
      .children(Object.entries(this.preferences.views ?? {}).map(([id, view]) => h_flex().gap(4).items_center()
        .child(div().flex_1().min_w(0).child(choice(`view-${id}`, id, active === id,
          [...view.labels.map(l => pill(t, `view-${id}-l-${l}`, `#${l}`)),
            ...(view.match === "any" && view.labels.length && view.machines.length ? [div().text_size(10.5).text_color(t.dim).child("or")] : []),
            ...view.machines.map(m => pill(t, `view-${id}-m-${m}`, `@${m}`))],
          () => select(id))))
        .child(link(`edit-view-${id}`, "Edit", (_e, cx) => {
          this.viewName.set_value(id); this.viewLabels.set_value(view.labels.join(", "));
          this.viewMachines.set_value(view.machines.join(", ")); this.viewMatch = view.match === "any" ? "any" : "all"; cx.notify();
        }))
        .child(link(`delete-view-${id}`, "Delete", (_e, cx) => cx.spawn(cx => this.saveView(cx, true, id))))))
      .child(heading("Save a view"))
      .child(Input.new(this.viewName).h(30).px(8).rounded(6).border_1().border_color(t.input).bg(t.background))
      .child(div().text_size(11).text_color(t.dim).child("Labels"))
      .child(toggles("label", labels, chosenLabels, this.viewLabels))
      .child(div().text_size(11).text_color(t.dim).child("Machines"))
      .child(toggles("machine", machines, chosenMachines, this.viewMachines))
      .child(h_flex().gap(6).items_center().pt(2)
        .child(div().text_size(11).text_color(t.dim).child("Show projects matching"))
        .child(pill(t, "match-all", "a label and a machine", { on: this.viewMatch !== "any", action: () => { this.viewMatch = "all"; } }))
        .child(pill(t, "match-any", "a label or a machine", { on: this.viewMatch === "any", action: () => { this.viewMatch = "any"; } })))
      .child(h_flex().pt(4).child(div().id("save-view").px(12).py(5).rounded(6).bg(t.raised).cursor_pointer()
        .child(this.saving ? "Saving…" : "Save view").on_click((_e, cx) => cx.spawn(cx => this.saveView(cx)))))
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

  // The vega.project-id of `addr` or its nearest labelled live ancestor.
  projectLabelOf(addr, seen = new Set()) {
    const x = this.vegaTerminals?.get(addr);
    if (!x || seen.has(addr)) return null;
    return x.labels?.["vega.project-id"] || this.projectLabelOf(x.parent, new Set([...seen, addr]));
  }

  // A child labelled with another project than its parent leaves the parent's tree (vega-0h4c.75),
  // but only into a project with a UAC (the signal behind the "no UAC" marker, vega-0h4c.79).
  foreignChild(k) {
    const own = k.labels?.["vega.project-id"], parent = this.projectLabelOf(k.parent);
    return Boolean(own && parent && own !== parent && this.projectUacs?.has(own));
  }

  // Live sub-agents of `addr`, depth first, each with the tree lines in front of it.
  tree(all, addr, lead = [], visible = null, seen = new Set()) {
    if (seen.has(addr)) return [];
    seen = new Set([...seen, addr]);
    const children = parent => all.filter(k => k.parent === parent && k.state === "live"
      && !seen.has(k.address) && !k.labels?.["vega.worktree-owner"] && !this.foreignChild(k));
    // Promote active grandchildren past hidden idle parents, preserving tree connectors.
    const frontier = (parent, path) => children(parent).flatMap(k => {
      if (path.has(k.address)) return [];
      return !visible || visible.has(k.address) ? [k] : frontier(k.address, new Set([...path, k.address]));
    });
    const kids = frontier(addr, seen);
    return kids.flatMap((k, i) => {
      const last = i === kids.length - 1;
      return [
        { t: k, lines: [...lead, last ? "└" : "├"] },
        ...this.tree(all, k.address, [...lead, last ? "" : "│"], visible, seen),
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
  // join only a unique project; new terminals always carry vega.project-id.
  projectGroups(all) {
    const short = value => value?.replace(/\/$/, "").split("/").at(-1)?.replace(/\.git$/, "");
    // The Uber-UAC is no project's member: its project label opens no group and no "no UAC" marker.
    const live = all.filter(x => x.state === "live" && x.labels?.["vega.role"] !== "uber");
    const byAddress = new Map(live.map(x => [x.address, x]));
    const labelsOf = address => byAddress.get(address)?.labels ?? {};
    const projects = new Map();
    for (const x of live) {
      const id = x.labels?.["vega.project-id"];
      if (id && !projects.has(id)) projects.set(id, x);
    }
    // Keep legacy-only projects too, without inventing a third group for an
    // ambiguous short name shared by multiple full project IDs.
    for (const x of live) {
      const legacy = x.labels?.["vega.project"];
      if (!x.labels?.["vega.project-id"] && legacy && !projects.has(legacy)
        && ![...projects.keys()].some(id => short(id) === legacy)) projects.set(legacy, x);
    }
    const resolve = l => {
      if (l["vega.project-id"]) return l["vega.project-id"];
      const legacy = l["vega.project"];
      if (!legacy) return null;
      if (projects.has(legacy)) return legacy;
      const matches = [...projects.keys()].filter(id => short(id) === legacy);
      return matches.length === 1 ? matches[0] : null;
    };
    const uacs = new Map();
    for (const x of live) {
      const id = resolve(x.labels ?? {});
      if (x.labels?.["vega.role"] === "uac" && id) {
        uacs.set(id, x);
        projects.set(id, x);
      }
    }
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
    return { live, labelsOf, projects, uacs, projectOfSpace, projectOfAgent };
  }

  viewFilter() {
    const active = activeView();
    if (active.startsWith(LABEL_VIEW)) return { labels: [active.slice(LABEL_VIEW.length)], machines: [] };
    return this.preferences?.views?.[active] ?? { labels: [], machines: [] };
  }

  matchesView(project, machine) {
    const view = this.viewFilter();
    const labels = this.preferences?.projects?.[project]?.labels ?? [];
    const byLabel = view.labels.some(label => labels.includes(label)), byMachine = view.machines.includes(machine);
    // any: a chosen label OR a chosen machine; an empty dimension then adds nothing.
    if (view.match === "any") return byLabel || byMachine;
    return (!view.labels.length || byLabel) && (!view.machines.length || byMachine);
  }

  orderedProjects(projects) {
    const saved = this.order ?? [];
    const orderIndex = id => {
      const exact = saved.indexOf(id);
      if (exact >= 0) return exact;
      // Migrate the old short-name order only for an unambiguous project.
      const short = id.split("/").at(-1);
      return [...projects.keys()].filter(p => p.split("/").at(-1) === short).length === 1 ? saved.indexOf(short) : -1;
    };
    return [...projects.keys()].sort((a, b) => {
      const i = orderIndex(a), j = orderIndex(b);
      return (i < 0 ? Infinity : i) - (j < 0 ? Infinity : j) || a.localeCompare(b);
    });
  }

  // Projects in the Slayer's order, holding agents on every machine (UAC first,
  // when present, its sentinels as branches). Then the unlabelled spaces.
  // Symbols as in the herdr TUI sidebar (docs/design/theme-symbols.md).
  projectTree(t, all, list, shown, now) {
    const { live, labelsOf, projects: groups, uacs, projectOfSpace, projectOfAgent } = this.projectGroups(all);
    const projects = this.orderedProjects(groups).filter(p => {
      const representative = groups.get(p);
      // A project without a UAC whose agents all render under a parent elsewhere has no rows (vega-0h4c.80).
      const empty = !uacs.has(p) && !this.preferences?.projects?.[p]?.pinned
        && !list.some(a => projectOfAgent(a) === p)
        && !(this.plainTerminals ?? []).some(x => projectOfAgent(x) === p);
      return !empty && this.matchesView(p, machineOf(representative.address))
        && (!this.visibleSessions || this.preferences?.projects?.[p]?.pinned
          || list.some(a => projectOfAgent(a) === p && this.hasVisibleTree(all, a)));
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
      const st = depth ? { status: kidStatus(a, this.listed) } : { ...a, status: this.topStatus(a) };
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
        .on_click(() => depth || !a.pane ? this.peek(a.address) : run("pane.focus", { pane: a.pane }))
        .when(uac, el => el.on_mouse_down("right", (_e, cx) => this.openProject(l["vega.project-id"], cx)))
        .children(lines.map(x => div().self_start().mt(-4).w(9).h(16).mr(2).flex_none()
          .when(Boolean(x), el => el.border_l(1.5).border_color(t.input))
          .when(x === "└" || x === "├", el => el.border_b(1.5).rounded_bl(6))))
        .child(this.status(t, st, true))
        .when(uac, el => el.child(div().flex_none().font_semibold().text_color(C.uac).child(l["vega.uac"] || "u▲c")))
        .child(div().flex_none().text_color(color).child(glyph))
        .when(!depth && this.foreignChild(this.vegaTerminals.get(a.address) ?? {}), el => el.child(div().flex_none().text_size(11).text_color(t.dim)
          .tooltip(`Spawned by ${this.vegaTerminals.get(a.address).parent}`).child(`↑ ${this.vegaTerminals.get(a.address).parent.split("/").at(-1)}`)))
        .when(Boolean(name), el => el.child(div().min_w(0).flex_shrink_1().truncate().text_color(a.status === "idle" ? t.muted : t.foreground).child(name)))
        .child(tokens(a.address, uac))
        .child(div().flex_1())
        .when(!depth && subs.length > 0 && this.folded.has(a.address), el => el.child(div().text_size(11).text_color(t.dim).child(`+${subs.length}`)))
        .when(Boolean(when), el => el.child(div().flex_none().text_size(11).text_color(t.dim).child(duration(now - when))))
        .child(host(a.address));
    };
    const withTree = (a) => {
      const subs = this.tree(all, a.address, [], this.visibleSessions);
      const visible = !this.visibleSessions || this.visibleSessions.has(a.address);
      return [...(visible ? [agentRow(a, subs, 0)] : []),
        ...(visible && this.folded.has(a.address) ? [] : subs.map(s => agentRow(s.t, [], 1, s.lines)))];
    };
    const badge = (key, glyph, n, action, gid) => div().id(`total-${gid}-${key}`).flex_none().text_size(11).font_semibold().text_color(C[key])
      .tooltip(`Project ${key}`).child(`${glyph} ${n}`)
      .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
      .on_click((_e, cx) => { cx.stop_propagation(); action(); });
    const SHELLS = new Set(["sh", "bash", "zsh", "fish", "dash", "nu", "ksh", "tcsh", "login"]);
    const programOf = x => {
      const base = v => v?.split("/").at(-1)?.replace(/^-/, "");
      return base(x.foreground?.name) || base(x.foreground?.argv?.[0]) || x.program?.key || "terminal";
    };
    // Plain shells are noise: under Active only a focused one or one in a pinned project stays.
    const plainRows = (members, pinned) => members.filter(x => !this.visibleSessions || !SHELLS.has(programOf(x))
      || x.address === shown || pinned).map(x => {
      const cwd = x.foreground?.cwd?.split("/").at(-1);
      const argv = x.foreground?.argv?.join(" ");
      return h_flex().id(`term-${x.address}`).pl(18).pr(6).h(24).flex_none().gap(6).items_center().rounded(6)
        .when(x.address === shown, el => el.bg(t.selected))
        .cursor_pointer().hover(s => s.bg(t.hover))
        .tooltip([argv, x.foreground?.cwd].filter(Boolean).join(" · ") || x.address)
        .on_click(() => x.pane ? run("pane.focus", { pane: x.pane }) : this.peek(x.address))
        .child(div().flex_none().text_color(C.other_agent).child("◇"))
        .child(div().min_w(0).flex_shrink_1().truncate().text_color(t.dim).child(programOf(x)))
        .when(Boolean(cwd) && !SHELLS.has(programOf(x)), el => el.child(div().min_w(0).flex_shrink_1().truncate().text_size(11).text_color(t.dim).child(cwd)))
        .child(div().flex_1())
        .child(host(x.address));
    });
    const plainOf = (p) => (this.plainTerminals ?? []).filter(x => projectOfAgent(x) === p);
    const out = [];
    if (this.uberRow) out.push(this.uberRowView(t, C, host, shown, now));
    for (const p of projects) {
      const gid = `uac-${p}`;
      const uac = uacs.get(p);
      const ul = uac?.labels ?? {};
      const project = p;
      const address = groups.get(p).address;
      const display = this.preferences?.projects?.[project]?.name || ul["vega.project"]?.split("/").at(-1) || p.split("/").at(-1);
      const totals = this.vegaTotals(address, project) ?? {};
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
        .when(!uac, el => el.child(div().flex_none().text_size(11).text_color(t.blocked).child("no UAC")))
        .when(Boolean(git), el => el.child(div().flex_none().text_size(11.5).text_color(git === "✓" ? C.git_clean : C.git_dirty).child(git)))
        // Label pills: a click filters the sidebar to that label, a second click shows all again.
        .children((this.preferences?.projects?.[project]?.labels ?? []).slice(0, 2).map(l => {
          const on = activeView() === LABEL_VIEW + l;
          return pill(t, `plabel-${gid}-${l}`, l, { on, tooltip: on ? "Show all projects" : `Show only ${l} projects`,
            action: () => setActiveView(on ? "all" : LABEL_VIEW + l) });
        }))
        .when((this.preferences?.projects?.[project]?.labels ?? []).length > 2, el => el.child(pill(t, `plabel-${gid}-more`,
          `+${this.preferences.projects[project].labels.length - 2}`, { tooltip: this.preferences.projects[project].labels.join(", ") })))
        .child(div().flex_1())
        // One summary badge per project (decisions, waiting, notes), coloured by the most urgent;
        // details live in the project window and the status line (vega-0h4c.50).
        .when((totals.decisions ?? 0) + (totals.waiting ?? 0) + (totals.notes ?? 0) >= 1, el => {
          const urgent = (totals.decisions ?? 0) + (totals.waiting ?? 0);
          return el.child(div().id(`total-${gid}`).flex_none().px(5).rounded(8).text_size(11).font_semibold()
            .bg(urgent ? t.blocked : t.raised).text_color(urgent ? t.background : t.foreground)
            .tooltip(`⚖ ${totals.decisions ?? 0} decisions · ✋ ${totals.waiting ?? 0} waiting · ✉ ${totals.notes ?? 0} notes`)
            .child(String((totals.decisions ?? 0) + (totals.waiting ?? 0) + (totals.notes ?? 0)))
            .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
            .on_click((_e, cx) => totals.decisions ? this.openProject(project, cx, "decisions")
              : totals.waiting ? (cx.stop_propagation(), run("pane.focus", { terminal: totals.waiting_panes?.[0]?.pane?.replace(/^hc:/, "") || address }))
              : this.openProject(project, cx, "notes")));
        })
        .child(div().id(`pin-${gid}`).flex_none().text_color(this.preferences?.projects?.[project]?.pinned ? t.accent : t.dim)
          .cursor_pointer().tooltip(this.preferences?.projects?.[project]?.pinned ? "Unpin project" : "Pin project in Active view")
          .child(svg("mods/vega-sidebar/pin.svg").size(12).text_color(this.preferences?.projects?.[project]?.pinned ? t.accent : t.dim))
          .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
          // The click's cx dies after the handler: the awaiting toggle runs in cx.spawn.
          .on_click((_e, cx) => { cx.stop_propagation(); cx.spawn(cx => this.toggleProjectPin(project, cx)); }))
        .child(div().id(`gear-${gid}`).flex_none().text_color(t.dim).cursor_pointer().tooltip("Project window")
          .child(svg("mods/workspace-chrome/settings.svg").size(12).text_color(t.dim))
          .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
          .on_click((_e, cx) => this.openProject(project, cx)))
        .when(!open, el => el.child(svg("mods/workspace-chrome/chevron-right.svg").size(11).flex_none().text_color(t.dim))));
      if (open) {
        for (const a of members) out.push(...withTree(a));
        out.push(...plainRows(plainOf(p), Boolean(this.preferences?.projects?.[project]?.pinned)));
      }
    }
    const all_spaces = spaces();
    const rest = all_spaces.filter(s => !projectOfSpace.has(s.id) && this.matchesView(null, s.machine || "local")
      && (!this.visibleSessions || list.some(a => a.space === s.id && !projectOfAgent(a) && this.hasVisibleTree(all, a))));
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
        out.push(...plainRows((this.plainTerminals ?? []).filter(x => x.space === sp.id && !projectOfAgent(x)), false));
      }
    }
    // Agents whose space this window doesn't know yet still show.
    const known = new Set(all_spaces.map(s => s.id));
    for (const a of list.filter(a => !known.has(a.space) && !projectOfAgent(a) && this.matchesView(null, machineOf(a.address)))) out.push(...withTree(a));
    return out;
  }

  // The Uber-UAC's row: marker, status, name, "engaged" while its window is open, host. A click
  // focuses its pane like an agent row (peek without a pane).
  uberRowView(t, C, host, shown, now) {
    const { term, agent } = this.uberRow;
    const l = term.labels ?? {};
    const a = { ...(agent ?? {}), address: term.address, status: agent ? this.topStatus(agent) : "idle" };
    const when = agent ? this.since(agent) : 0;
    const pane = agent?.pane ?? term.pane;
    return h_flex().id(`uber-${term.address}`).pl(18).pr(6).h(24).flex_none().gap(6).items_center().rounded(6)
      .when(term.address === shown, el => el.bg(t.selected))
      .cursor_pointer().hover(s => s.bg(t.hover))
      .tooltip(["Uber-UAC", agent?.title].filter(Boolean).join(" · "))
      .on_click(() => pane ? run("pane.focus", { pane }) : this.peek(term.address))
      .child(this.status(t, a, true))
      .child(div().flex_none().font_semibold().text_color(C.uac).child("Über"))
      .when(l["vega.engaged"] === "yes", el => el.child(div().flex_none().text_size(11).text_color(t.accent).child("engaged")))
      .child(div().flex_1())
      .when(Boolean(when), el => el.child(div().flex_none().text_size(11).text_color(t.dim).child(duration(now - when))))
      .child(host(term.address));
  }

  // A top-level row's status: its agent report, done only while unread (seenDone).
  topStatus(a) {
    return seenDone(this.vegaTerminals?.get(a.address), this.listed.get(a.address) ?? a.status);
  }

  hasVisibleTree(all, a) {
    return !this.visibleSessions || this.visibleSessions.has(a.address)
      || this.tree(all, a.address).some(s => this.visibleSessions.has(s.t.address));
  }

  // The local machine's $host label: the one vegad wrote on a local terminal.
  localHost(live) {
    return live.find(x => !x.address.includes("/") && x.labels?.["vega.host"])?.labels["vega.host"] ?? "";
  }

  // Drag one project above another; the order stays across restarts.
  move(projects, from, to) {
    const all = this.orderedProjects(this.projectGroups(terminals()).projects);
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
  // Per-agent signals only; project-wide notes and decisions sit in the project badge.
  vegaIcons(address, t, skip = ["uac", "kind"]) {
    skip = [...skip, "notes", "decisions"];
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
    sampleFocus();
    const collapsed = narrow();

    let list = agents();
    this.listed = new Map(list.map((a) => [a.address, a.status]));
    const all = terminals();
    this.vegaTerminals = new Map(all.map(t => [t.address, t]));
    this.projectUacs = this.projectGroups(all).uacs;
    // The Uber-UAC lists as one row at the top, outside every project (docs/design/uber-uac.md).
    const uberTerm = all.find(x => x.state === "live" && x.labels?.["vega.role"] === "uber");
    this.uberRow = uberTerm ? { term: uberTerm, agent: list.find(a => a.address === uberTerm.address) } : null;
    list = list.filter(a => a.address !== uberTerm?.address);
    // A worktree's placeholder shell holds the checkout for its sentinel; the TUI never listed it.
    const placeholder = (address) => Boolean(this.vegaTerminals.get(address)?.labels?.["vega.worktree-owner"]);
    list = list.filter(a => !placeholder(a.address));
    const filtering = hideInactive();
    // A UAC without an agent-status entry still provides context for its live children.
    if (filtering) {
      const known = new Set(list.map(a => a.address));
      list = [...list, ...all.filter(a => a.state === "live" && a.labels?.["vega.role"] === "uac"
        && !known.has(a.address) && !placeholder(a.address)).map(a => ({ ...a, space: this.spaceOfTerminal(a) }))];
    }
    this.migratePins(all);
    const focus = focused();
    const shown = focus?.terminal ?? focus?.agent;
    const now = Date.now();
    this.track(list, now);
    // A listed child belongs to its visible ancestor's tree, even in another space.
    const listed = new Set(list.map(a => a.address));
    list = list.filter(a => {
      const seen = new Set([a.address]);
      let parent = this.vegaTerminals.get(a.address)?.parent;
      while (parent && !seen.has(parent) && this.vegaTerminals.get(parent)?.state === "live") {
        if (listed.has(parent) && !this.foreignChild(this.vegaTerminals.get(a.address))) return false;
        seen.add(parent); parent = this.vegaTerminals.get(parent)?.parent;
      }
      return true;
    });
    const membership = this.projectGroups(all);
    list = list.filter(a => {
      const project = membership.projectOfAgent(a);
      return this.matchesView(project, machineOf(membership.projects.get(project)?.address ?? a.address));
    });
    if (shared(INACTIVE_KEY) != null) localStorage.setItem("hide-inactive", String(filtering));
    // Count only this view's sessions, before folds; UACs may remain as context.
    const kids = new Set(list.flatMap(a => this.tree(all, a.address).map(s => s.t.address)));
    const candidates = new Map(list.flatMap(a => [a, ...this.tree(all, a.address).map(s => s.t)]).map(a => [a.address, a]));
    // Judge the status the row shows, including unread done sub-agents.
    const rowStatus = a => kids.has(a.address) ? kidStatus(a, this.listed) : this.topStatus(a);
    this.visibleSessions = filtering ? new Set([...candidates.values()].filter(a =>
      a.address === shown || ["working", "blocked", "done", "error"].includes(rowStatus(a))
    ).map(a => a.address)) : null;
    if (filtering) for (const uac of membership.uacs.values()) {
      if (this.preferences?.projects?.[uac.labels?.["vega.project-id"]]?.pinned
        || this.tree(all, uac.address).some(s => this.visibleSessions.has(s.t.address))) this.visibleSessions.add(uac.address);
    }
    // Live terminals without an agent (token-burn tui, pdc, a dev server, a shell) list under their
    // project or space as dim rows. Sub-agent rows, UACs and worktree placeholders are not among them.
    const agentAddresses = new Set(agents().map(x => x.address));
    this.plainTerminals = all.filter(x => x.state === "live" && x.foreground && !agentAddresses.has(x.address)
      && !kids.has(x.address) && !placeholder(x.address) && !["uac", "uber"].includes(x.labels?.["vega.role"])
      && !(list.some(a => a.address === x.address)))
      .map(x => ({ ...x, space: this.spaceOfTerminal(x) }))
      .filter(x => this.matchesView(membership.projectOfAgent(x), machineOf(x.address)));
    const hiddenCount = filtering ? [...candidates.keys()].filter(a => !this.visibleSessions.has(a)).length : 0;
    list = list.filter(a => this.hasVisibleTree(all, a));
    const projectOrder = this.orderedProjects(membership.projects);
    list.sort((a, b) => {
      const pa = membership.projectOfAgent(a), pb = membership.projectOfAgent(b);
      const ia = projectOrder.indexOf(pa), ib = projectOrder.indexOf(pb);
      return (ia < 0 ? Infinity : ia) - (ib < 0 ? Infinity : ib) ||
        (pa && pa === pb ? (membership.labelsOf(b.address)["vega.role"] === "uac") - (membership.labelsOf(a.address)["vega.role"] === "uac") : 0);
    });
    setAgentOrder(list.flatMap(a => [a.address, ...this.tree(all, a.address).map(s => s.t.address)])
      .filter(a => listed.has(a) && (!this.visibleSessions || this.visibleSessions.has(a))));

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
      .hover(el => el.bg(t.hover)).on_click(() => a.pane ? run("pane.focus", { pane: a.pane }) : this.peek(a.address))
      .when(this.vegaTerminals.get(a.address)?.labels?.["vega.role"] === "uac", el =>
        el.on_mouse_down("right", (_e, cx) => this.openProject(this.vegaTerminals.get(a.address).labels["vega.project-id"], cx)))
      .child(div().flex_1().min_w(0).whitespace_nowrap().text_right().text_color(t.muted).child(String(i + 1)))
      .child(h_flex().w(14).flex_none().items_center().justify_center()
        .child(this.status(t, { status: this.topStatus(a) }, true)));

    this.rowLabel = a => detail.get(a.address) ?? label(a);
    this.activeView = activeView();
    if (shared(VIEW_KEY) && localStorage.getItem("active-view") !== this.activeView) localStorage.setItem("active-view", this.activeView);
    const rail = filtering ? list.flatMap(a => [a, ...this.tree(all, a.address).map(s => s.t)])
      .filter(a => this.visibleSessions.has(a.address)) : list;
    const rows = collapsed ? rail.map(railRow) : this.projectTree(t, all, list, shown, now);

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
            .child(h_flex().gap(8).items_center()
              .child(pill(t, "ag-hide-inactive", "Active", { on: filtering,
                tooltip: filtering ? "Show inactive sessions" : "Hide inactive sessions",
                action: () => setHideInactive(!filtering) }))
              .child(
              div()
                .id("ag-views")
                .text_size(11)
                .text_color(t.dim)
                .hover((s) => s.text_color(t.foreground))
                .on_click(() => openOverlay({ mod: "vega-sidebar", kind: "popup", width: 420, height: 480, props: { viewMenu: true } }))
                .tooltip("Filter by labels and machines")
                .child(`${this.activeView.startsWith(LABEL_VIEW) ? `#${this.activeView.slice(LABEL_VIEW.length)}`
                  : this.preferences?.views?.[this.activeView] ? this.activeView : "All"} ▾`),
            )),
        ),
      )
      .when(filtering, el => el.child(div().id("ag-idle-hidden").px(collapsed ? 4 : 16).text_size(11).text_color(t.dim)
        .cursor_pointer().tooltip("Show inactive sessions")
        .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
        .on_click((_e, cx) => { cx.stop_propagation(); setHideInactive(false); cx.notify(); })
        .child(`${hiddenCount} idle hidden`)))
      .when(Boolean(this.configError) && !collapsed, el => el.child(div().px(12).text_size(11).text_color(t.blocked).child(this.configError)))
      .when(list.length === 0 && !collapsed && !filtering, (el) =>
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
