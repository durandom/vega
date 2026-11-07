import { View, div, svg } from "gpui-kit";
import { h_flex } from "gpui-base";
import { exec, home, theme, watchFolder, shared, setShared, openOverlay, notify, nextCommand, run, spaces, agents } from "herdr";

// Assumes the same user and XDG_STATE_HOME as vegad; this reads the mod's environment,
// not the app config home. Both processes must share that state directory.
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

// Only saved, connected hc machines are read. No reverse link or registry ACL
// change is needed; hc's SSH connection already authorizes execution there.
const collectBadges = `import sys,json,subprocess,base64,concurrent.futures
reader=sys.argv[1]
def command(args):
 p=subprocess.run(args,capture_output=True,text=True,timeout=12)
 if p.returncode: raise ValueError(p.stderr or p.stdout or 'command failed')
 return json.loads(p.stdout)
local=command([sys.executable,'-c',reader])
for row in local['rows']: row['machine']=''
errors=[]
try: machines=command(['hc','machine','ls','--json'])['machines']
except (OSError,ValueError,KeyError,subprocess.TimeoutExpired) as e:
 machines=[]; errors.append('hc machines: '+str(e))
def remote(m):
 name=m['name']
 try:
  wrapped=command(['hc','exec','--json','--on',name,'--timeout','8','--','python3','-c',reader])
  if wrapped.get('code')!=0: raise ValueError(base64.b64decode(wrapped.get('stderr','')).decode('utf-8',errors='replace') or 'remote badge reader failed')
  data=json.loads(base64.b64decode(wrapped['stdout'],validate=True))
  rows=data['rows']
  for row in rows:
   row['machine']=name
   row['waiting_panes']=[dict(w,pane='hc:'+name+'/'+w['pane'].removeprefix('hc:')) for w in row.get('waiting_panes',[]) if w.get('pane','').removeprefix('hc:').startswith('t') and '/' not in w['pane']]
  return rows,None
 except (OSError,ValueError,KeyError,TypeError,subprocess.TimeoutExpired) as e: return [],name+': '+str(e)
with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
 for rows,error in pool.map(remote,[m for m in machines if m.get('state')=='connected']):
  local['rows'].extend(rows)
  if error: errors.append(error)
local['errors']=errors
print(json.dumps(local))`;

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
    const result = await exec("python3", ["-c", collectBadges, readBadges]);
    if (result.code !== 0) throw new Error(result.stderr || "Cannot read badges");
    const data = JSON.parse(result.stdout);
    this.error = data.errors?.length ? data.errors.join("; ") : null;
    const now = Date.now()/1000;
    const rows = data.rows.filter(r => now-r.at <= 60);
    // A changed timestamp renews freshness, but never reopens an old decision.
    const serialized = JSON.stringify(rows);
    if (serialized !== this.lastBadges) {
      this.lastBadges = serialized;
      setShared("vega.badges", rows);
      // The command exists only while the inbox window runs; a closed one would toast an error.
      if (shared("vega.inbox-open")) run("vega-inbox.refresh");
    }
    const fresh = rows.flatMap(r => (r.decision_texts ?? []).map(d => ({...d, key: `${r.machine || ""}/${r.project}/${d.id}`}))).filter(d => !this.seen.has(d.key));
    if (fresh.length) {
      for (const d of fresh) this.seen.add(d.key);
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
    try { await this.badges(cx); }
    catch (e) { this.error = String(e); }
    finally { this.refreshing = false; }
    this.state = state;
    cx.notify();
  }

  // The shipped status line (workspace-status) without the palette hint; vega's own
  // work (badges, vegad, inbox) runs in the background and shows nothing here.
  render(_cx) {
    const t = theme();
    const space = spaces().find(s => s.active);
    const list = agents();
    const needs = list.filter(a => a.status === "blocked").length;
    const label = (icon, text) => h_flex().gap(6).items_center().child(svg(`mods/workspace-chrome/${icon}.svg`).size(13)).child(text);
    return h_flex().size_full().px(14).gap(16).items_center().bg(t.chrome).border_t(1).border_color(t.border).text_size(11.8).text_color(t.muted)
      .child(label(space?.machine ? "server" : "monitor", space?.machine ?? "This computer"))
      .when(Boolean(space?.branch), el => el.child(label("branch", space.branch)))
      .child(h_flex().gap(6).items_center().child(div().size(6).rounded(3).bg(t.dim)).child(`${list.length} agents`)
        .when(needs > 0, el => el.child(div().text_color(t.blocked).child(`· ${needs} needs you`))));
  }
}
