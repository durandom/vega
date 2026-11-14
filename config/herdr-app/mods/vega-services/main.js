import { View, div } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { exec, hc, readTextFile, appHome, spaces, panes, terminals, theme, window } from "herdr";
import { attribute, privateTarget } from "./model.js";

export default class Services extends View {
  init(_props, cx) {
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
  toggle(row, cx) {
    if (this.pending.has(row.key) || this.closed) return;
    const existing = this.forwards.get(row.key), target = this.targets.get(row.machine);
    if (!existing && (!row.machine || !privateTarget(target) || !privateTarget(row.machine))) return;
    this.pending.add(row.key);
    cx.spawn(async cx => {
      try {
        if (existing) { await this.call("stop", [existing.id]); this.forwards.delete(row.key); }
        else {
          const id = String(++this.next);
          const f = await this.call("start", [id, target, row.host, String(row.port)]);
          if (this.closed) await this.call("stop", [id]);
          else this.forwards.set(row.key, {...f, id, row});
        }
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
  open(url, cx) {
    cx.spawn(async cx => {
      try {
        const platform = window()?.platform;
        const r = await exec(platform === "macos" ? "open" : "xdg-open", [url]);
        if (r.code) throw new Error(r.stderr || r.stdout);
      } catch (e) { this.errors = [String(e.message ?? e)]; cx.notify(); }
    });
  }
  render() {
    const t = theme();
    const button = (id, text, action) => div().id(id).px(7).py(3).rounded(4).text_color(t.accent).hover(s => s.bg(t.hover))
      .on_mouse_down("left", (_e,cx) => cx.stop_propagation()).on_click((_e,cx) => action(cx)).child(text);
    let group = "";
    return v_flex().size_full().min_h(0).bg(t.background).rounded(10).border(1).border_color(t.border).text_size(12).text_color(t.foreground)
      .child(h_flex().px(12).h(40).items_center().child(div().flex_1().child("Services"))
        .child(button("refresh", this.reading ? "Reading…" : "Refresh", cx => this.refresh(cx))))
      .children(this.errors.map(e => div().px(12).py(4).text_color(t.blocked).child(e)))
      .child(v_flex().id("services-list").flex_1().min_h(0).overflow_y_scroll().p(10).gap(6)
        .children(this.rows.map(row => {
          const header = `${row.project} · ${row.spaceName} · ${row.machine ?? "local"}`;
          const changed = group !== header; group = header;
          const f = this.forwards.get(row.key), busy = this.pending.has(row.key);
          const allowed = row.machine && privateTarget(row.machine) && privateTarget(this.targets.get(row.machine));
          const port = f?.local ?? row.port;
          const host = !f && row.host === "::1" ? "[::1]" : "127.0.0.1";
          const url = `http://${host}:${port}`;
          return v_flex().gap(4).flex_none()
            .when(changed, el => el.child(div().pt(8).font_semibold().text_color(t.muted).child(header)))
            .child(h_flex().gap(6).items_center()
              .child(div().flex_1().child(`${row.name} :${row.port}${row.gone ? " · stopped" : ""}`))
              .when(Boolean(f || allowed), el => el.child(button(`forward-${row.key}`, busy ? "Working…" : f ? "Stop" : "Forward", cx => this.toggle(row,cx)))))
            .child(div().text_color(t.dim).child(url))
            .when(Boolean(f || !row.machine), el => el.child(button(`open-${row.key}`, "Open", cx => this.open(url,cx))))
            .when(Boolean(row.machine && !allowed), el => el.child(div().text_color(t.dim).child("Read only · private SSH hosts only")));
        }))
        .when(!this.rows.length, el => el.child(div().text_color(t.dim).child("No attributed TCP listeners. Refresh after starting a service."))))
      .child(div().p(10).text_color(t.dim).child("Manual refresh · HTTP links · forwards end when this panel closes"));
  }
}
