import { View } from "gpui-kit";
import { h_flex } from "gpui-base";
import { exec, home, theme, watchFolder, shared, setShared, openOverlay, notify, nextCommand, run } from "herdr";

// The badge directory follows vegad's XDG environment, not the app config home.
// Read in one subprocess: no shell interpolation, no terminal/session identity.
const readBadges = `import os,json,pathlib
root=pathlib.Path(os.environ.get('XDG_STATE_HOME',str(pathlib.Path.home()/'.local/state')))/'vega/badges/v2'
rows=[]
for p in root.glob('*.json'):
 try:
  row=json.loads(p.read_text())
  if row.get('schema')==2: rows.append(row)
 except (OSError,ValueError): pass
print(json.dumps({'folder':str(root),'rows':rows}))`;

export default class Vega extends View {
  init(_props, cx) {
    this.state = "checking";
    this.refreshing = false;
    this.seen = new Set(shared("vega.seen-decisions") ?? []);
    cx.spawn(async (cx) => {
      try { await exec(`${home()}/.local/bin/vega-post`, ["relay", "start"]); } catch (_) {}
      await this.refresh(cx);
    });
    cx.timer.every(15000, (cx) => {
      cx.spawn(async (cx) => { await this.refresh(cx); });
    });
    cx.spawn(async () => {
      for (;;) { if (await nextCommand() === "inbox") this.open(); }
    });
  }

  open() {
    openOverlay({mod: "vega-inbox", kind: "window", key: "vega-inbox", keep: true, width: 620, height: 520});
  }

  async badges(cx) {
    const result = await exec("python3", ["-c", readBadges]);
    if (result.code !== 0) throw new Error(result.stderr || "Cannot read badges");
    const data = JSON.parse(result.stdout);
    const now = Date.now()/1000;
    const rows = data.rows.filter(r => now-r.at <= 60);
    // A changed timestamp renews freshness, but never reopens an old decision.
    const serialized = JSON.stringify(rows);
    if (serialized !== this.lastBadges) {
      this.lastBadges = serialized;
      setShared("vega.badges", rows);
      run("vega-inbox.refresh");
    }
    const fresh = rows.flatMap(r => r.decision_texts ?? []).filter(d => !this.seen.has(`${d.project}/${d.id}`));
    if (fresh.length) {
      for (const d of fresh) this.seen.add(`${d.project}/${d.id}`);
      setShared("vega.seen-decisions", [...this.seen]);
      setShared("vega.inbox-project", null);
      if (!shared("vega.inbox-open")) this.open(); // Scratch-app probe: kind window preserves keyboard focus.
      notify({title: `${fresh.length} new decision${fresh.length === 1 ? "" : "s"}`,
        body: fresh[0].title, action: {name: "vega.inbox"}});
    }
    if (!this.watching) {
      this.watching = true;
      cx.spawn(async (cx) => {
        try {
          for (;;) { await watchFolder(data.folder); await this.badges(cx); }
        } catch (_) { /* Missing folder/watcher failure: retry on the status timer. */ }
        finally { this.watching = false; }
      });
    }
  }

  async refresh(cx) {
    if (this.refreshing) return;
    this.refreshing = true;
    let state = "unavailable";
    try {
      const result = await exec(`${home()}/.local/bin/vega-post`, ["relay", "status", "--json"]);
      if (result.code === 0) {
        const status = JSON.parse(result.stdout);
        if (typeof status.running === "boolean") state = status.running ? "running" : "stopped";
      }
    } catch (_) {}
    try { await this.badges(cx); this.error = null; }
    catch (e) { this.error = String(e); }
    finally { this.refreshing = false; }
    this.state = state;
    cx.notify();
  }

  render(_cx) {
    const t = theme();
    const rows = (shared("vega.badges") ?? []).filter(r => Date.now()/1000-r.at <= 60);
    const decisions = rows.reduce((n,r) => n+(r.decisions || 0),0);
    return h_flex().id("vega-inbox-open").size_full().items_center().px(12).text_size(12)
      .bg(t.chrome).text_color(this.error ? t.blocked : this.state === "running" ? t.muted : t.working)
      .cursor_pointer().tooltip(this.error || "Open Vega inbox")
      .on_click(() => this.open()).child(`vega · vegad ${this.state} · ⚖ ${decisions}`);
  }
}
