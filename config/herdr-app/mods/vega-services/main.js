import { View, div } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { exec, hc, readTextFile, appHome, spaces, panes, terminals, theme, focused, notify } from "herdr";
import { attribute, privateTarget, selectedProject, serviceAddress } from "./model.js";

export default class Services extends View {
  init(_props, cx) {
    this.showAgentPorts = false;
    this.rows = []; this.errors = []; this.forwards = new Map(); this.pending = new Set();
    this.owner = `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
    this.next = 0; this.targets = new Map();
    this.helper = `${appHome()}/mods/vega-services/helper.py`;
    this.refresh(cx);
  }
  async call(action, args = []) {
    const r = await exec("python3", [this.helper, action, this.owner, ...args], {timeout: 60000});
    const data = JSON.parse(r.stdout);
    if (r.code || data.error) throw new Error(data.error || r.stderr || "Services helper failed");
    return data;
  }
  refresh(cx) {
    if (this.reading || this.closed) return;
    this.reading = true;
    cx.spawn(async cx => {
      const errors = [], rows = [];
      try {
        const script = await readTextFile(this.helper);
        const inventory = await hc(["machine", "ls", "--json"]);
        if (inventory.code) throw new Error(inventory.stderr || inventory.stdout);
        this.targets = new Map((JSON.parse(inventory.stdout).machines ?? []).filter(m => !m.temporary && !m.via).map(m => [m.name, m.target]));
        const allSpaces = spaces(), allTerms = terminals(), allPanes = panes();
        const machines = [...new Set(allSpaces.map(s => s.machine ?? null))];
        await Promise.all(machines.map(async machine => {
          try {
            const r = await exec("python3", ["-c", script, "scan"], {on: machine, timeout: 30000});
            const data = JSON.parse(r.stdout);
            if (r.code || data.error) throw new Error(data.error || r.stderr);
            rows.push(...attribute(data, machine, allTerms, allSpaces, allPanes));
          } catch (e) { errors.push(`${machine ?? "local"}: ${e.message ?? e}`); }
        }));
        for (const [key, f] of this.forwards) {
          try {
            if (!(await this.call("status", [f.id])).alive) this.forwards.delete(key);
          } catch (e) { errors.push(String(e.message ?? e)); }
        }
        this.rows = rows.sort((a,b) => a.project.localeCompare(b.project) || a.space.localeCompare(b.space) || a.port - b.port);
        // Keep forwards visible after a listener disappears, so Stop remains available.
        for (const [key, f] of this.forwards) if (!rows.some(r => r.key === key)) this.rows.push({...f.row, gone: true});
      } catch (e) { errors.push(String(e.message ?? e)); }
      this.errors = errors; this.reading = false;
      if (!this.closed) cx.notify();
    });
  }
  forwardKey(row) {
    if (this.forwards.has(row.key)) return row.key;
    for (const [key, f] of this.forwards) {
      if (f.row?.machine === row.machine && f.row?.host === row.host && f.row?.also_forward?.includes(row.port)) return key;
    }
    return row.key;
  }
  forwardFor(row) {
    const key = this.forwardKey(row), f = this.forwards.get(key);
    return f && key !== row.key ? {...f, local: row.port} : f;
  }
  toggle(row, cx, openAfter = false) {
    if (this.pending.has(row.key) || this.closed) return;
    const existingKey = this.forwardKey(row), existing = this.forwards.get(existingKey), target = this.targets.get(row.machine);
    if (!existing && (row.gone || !row.machine || !privateTarget(target) || !privateTarget(row.machine))) return;
    this.pending.add(row.key);
    cx.spawn(async cx => {
      try {
        if (existing && !openAfter) { await this.call("stop", [existing.id]); this.forwards.delete(existingKey); }
        else if (!existing) {
          const id = String(++this.next);
          const args = [id, target, row.host, String(row.port)];
          if (row.registered) args.push("same", ...(row.also_forward ?? []).map(String));
          const f = await this.call("start", args);
          if (this.closed) await this.call("stop", [id]);
          else this.forwards.set(row.key, {...f, id, row});
        }
        if (openAfter && !this.closed) cx.open_url(serviceAddress(row, this.forwardFor(row)));
        this.errors = [];
      } catch (e) { this.errors = [String(e.message ?? e)]; }
      this.pending.delete(row.key);
      if (!this.closed) cx.notify();
    });
    cx.notify();
  }
  async close() {
    this.closed = true;
    await this.call("close");
  }
  activate(row, cx) {
    if (this.closed || this.pending.has(row.key) || (row.gone && !this.forwardFor(row))) return;
    const url = serviceAddress(row, this.forwardFor(row));
    const web = /^https?:\/\//i.test(url);
    const allowed = row.machine && privateTarget(row.machine) && privateTarget(this.targets.get(row.machine));
    if (!web || (row.machine && !allowed)) {
      cx.write_to_clipboard(serviceAddress(row));
      notify({title: "Copied address", body: serviceAddress(row), timeout: 1800});
    } else if (row.machine) this.toggle(row, cx, true);
    else cx.open_url(url);
  }
  render() {
    const t = theme();
    const button = (id, text, action) => div().id(id).px(7).py(3).rounded(4).text_color(t.accent).hover(s => s.bg(t.hover))
      .on_mouse_down("left", (_e,cx) => cx.stop_propagation()).on_click((_e,cx) => action(cx)).child(text);
    const scope = selectedProject(focused(), terminals());
    const rows = this.rows.filter(row => (row.unattributed || scope.all || (scope.project && row.project === scope.project)) && (this.showAgentPorts || !row.agent || row.title || this.forwards.has(row.key)));
    let group = "";
    return v_flex().size_full().min_h(0).bg(t.background).rounded(10).border(1).border_color(t.border).text_size(12).text_color(t.foreground)
      .child(h_flex().px(12).h(40).items_center().child(div().flex_1().child("Services"))
        .child(button("agent-ports", this.showAgentPorts ? "Hide agent ports" : "Show agent ports", cx => { this.showAgentPorts = !this.showAgentPorts; cx.notify(); }))
        .child(button("refresh", this.reading ? "Reading…" : "Refresh", cx => this.refresh(cx))))
      .children(this.errors.map(e => div().px(12).py(4).text_color(t.blocked).child(e)))
      .child(v_flex().id("services-list").flex_1().min_h(0).overflow_y_scroll().p(10).gap(6)
        .children(rows.map(row => {
          const header = `${row.project} · ${row.spaceName} · ${row.machine ?? "local"}`;
          const changed = group !== header; group = header;
          const f = this.forwardFor(row), busy = this.pending.has(row.key) || this.pending.has(this.forwardKey(row));
          const allowed = row.machine && privateTarget(row.machine) && privateTarget(this.targets.get(row.machine));
          const url = serviceAddress(row, f);
          return v_flex().gap(4).flex_none()
            .when(changed, el => el.child(div().pt(8).font_semibold().text_color(t.muted).child(header)))
            .child(v_flex().p(10).gap(5).rounded(6).border(1).border_color(t.border)
            .child(h_flex().gap(6).items_center()
              .child(div().flex_1().font_semibold().child(`${row.title || row.name} :${row.port}`))
              .when(Boolean(f || (allowed && !row.gone)), el => el.child(button(`forward-${row.key}`, busy ? "Working…" : f ? f.row?.also_forward?.length ? "Stop bundle" : "Stop" : "Forward", cx => this.toggle(row,cx)))))
            .child(div().text_color(f ? t.accent : row.gone ? t.dim : t.foreground)
              .child(`${f ? "Forwarded" : row.gone ? "Stopped" : "Running"} · ${row.machine ?? "local"}${f && row.gone ? " · remote listener stopped" : ""}`))
            .when(Boolean(row.description), el => el.child(div().text_color(t.muted).child(row.description)))
            .when(Boolean(row.also_forward?.length), el => el.child(div().text_color(t.muted)
              .child(`Bundle · ${[row.port, ...row.also_forward].join(" + ")} · same local ports`)))
            .child(div().text_color(t.muted).child(f ? `Local forward → ${row.machine}:${row.port}` : row.machine ? `Remote port · ${row.machine}:${row.port}` : `Local port · ${row.port}`))
            .child(button(`address-${row.key}`, url, cx => this.activate(row,cx)))
            .when(Boolean(row.machine && !allowed), el => el.child(div().text_color(t.dim).child("Read only · private SSH hosts only"))));
        }))
        .when(!rows.length, el => el.child(div().text_color(t.dim).child("No services for the selected project. Refresh after starting a service."))))
      .child(div().p(10).text_color(t.dim).child("Manual refresh · click to open or copy · forwards end on close"));
  }
}
