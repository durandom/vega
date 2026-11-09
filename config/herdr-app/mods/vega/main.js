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
def failure(label,e):
 return label+(' slow ('+format(e.timeout,'g')+' s)' if isinstance(e,subprocess.TimeoutExpired) else ' unavailable')
def command(args):
 p=subprocess.run(args,capture_output=True,text=True,timeout=12)
 if p.returncode: raise ValueError(p.stderr or p.stdout or 'command failed')
 return json.loads(p.stdout)
local=command([sys.executable,'-c',reader])
for row in local['rows']: row['machine']=''
errors=[]
try: machines=command(['hc','machine','ls','--json'])['machines']
except (OSError,ValueError,KeyError,subprocess.TimeoutExpired) as e:
 machines=json.loads(sys.argv[2]); errors.append({'source':'machines','message':failure('hc machine ls',e)})
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
 except (OSError,ValueError,KeyError,TypeError,subprocess.TimeoutExpired) as e: return [],{'source':name,'message':failure('hc '+name+' badges',e)}
with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
 for rows,error in pool.map(remote,[m for m in machines if m.get('state')=='connected']):
  local['rows'].extend(rows)
  if error: errors.append(error)
local['machines']=machines
local['errors']=errors
print(json.dumps(local))`;

// Scratch apps must never create a detached daemon with a private state home, locally or on a
// machine. The app host owns vegad startup; no login job exists (vega-0h4c.42). The command
// (vega-post, or hc exec) runs only in the real default homes; one JSON line is printed.
const startRelay = `import os,pathlib,subprocess,sys,json
home=pathlib.Path.home()
def default(name,path):
 value=os.environ.get(name)
 return not value or pathlib.Path(value).expanduser().resolve()==path.resolve()
if default('HERDR_APP_HOME',home/'.config/herdr-app') and default('XDG_CONFIG_HOME',home/'.config') and default('XDG_STATE_HOME',home/'.local/state'):
 try:
  p=subprocess.run(sys.argv[1:],capture_output=True,text=True,timeout=30)
  print(json.dumps({'code':p.returncode,'stdout':p.stdout,'stderr':p.stderr}))
 except (OSError,subprocess.TimeoutExpired) as e: print(json.dumps({'code':-1,'stderr':str(e)}))
else: print(json.dumps({'skipped':True}))`;

// Idempotent starter on a machine. argv runs without a shell, so sh resolves the remote home;
// relay start reads that host's vegad.env, so a host pin (io-wsl registered as io) applies.
const remoteStart = 'exec "$HOME/.local/bin/vega-post" relay start';
const START_OK_MS = 300000, START_RETRY_MS = 60000;

export default class Vega extends View {
  init(_props, cx) {
    this.state = "checking";
    this.refreshing = false;
    this.started = new Map(); // machine -> {next: ms, fails}
    this.starting = false;
    this.seen = new Set(shared("vega.seen-decisions") ?? []);
    cx.spawn(async (cx) => {
      try { await exec("python3", ["-c", startRelay, `${home()}/.local/bin/vega-post`, "relay", "start"]); } catch (_) {}
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
    const result = await exec("python3", ["-c", collectBadges, readBadges, JSON.stringify(this.machines ?? [])]);
    if (result.code !== 0) throw new Error("badge reader unavailable");
    const data = JSON.parse(result.stdout);
    this.machines = data.machines ?? this.machines;
    const errors = data.errors ?? [];
    this.readErrors(errors);
    const failed = new Set(errors.map(e => e.source));
    // Keep only failed sources; successful empty reads and removed machines clear old rows.
    data.rows.push(...(shared("vega.badges") ?? []).filter(r => failed.has(r.machine || "local")));
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

  readErrors(errors) {
    const previous = this.failures ?? new Map();
    this.failures = new Map(errors.map(e => [e.source, (previous.get(e.source) ?? 0) + 1]));
    this.error = errors.filter(e => this.failures.get(e.source) >= 3).map(e => e.message).join("; ") || null;
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
    catch (_) { this.readErrors([{source: "reader", message: "badge reader unavailable"}]); }
    finally { this.refreshing = false; }
    this.state = state;
    cx.notify();
    this.startRemotes(cx);
  }

  // Starts vegad on every connected hc machine: when it turns connected, then every few
  // minutes (a failure retries after a minute). A failure shows after three in a row.
  async startRemotes(cx) {
    if (this.starting) return;
    this.starting = true;
    this.started ??= new Map();
    try {
      const live = (this.machines ?? []).filter(m => m.state === "connected").map(m => m.name);
      for (const name of [...this.started.keys()]) if (!live.includes(name)) this.started.delete(name);
      const due = live.filter(name => (this.started.get(name)?.next ?? 0) <= Date.now());
      await Promise.all(due.map(async name => {
        let ok = false;
        try {
          const r = await exec("python3", ["-c", startRelay, "hc", "exec", "--json", "--on", name, "--timeout", "20", "--", "sh", "-c", remoteStart]);
          const outer = r.code === 0 ? JSON.parse(r.stdout) : {code: -1};
          ok = Boolean(outer.skipped) || (outer.code === 0 && JSON.parse(outer.stdout).code === 0);
        } catch (_) {}
        const fails = ok ? 0 : (this.started.get(name)?.fails ?? 0) + 1;
        this.started.set(name, {next: Date.now() + (ok ? START_OK_MS : START_RETRY_MS), fails});
      }));
      const bad = [...this.started].filter(([, v]) => v.fails >= 3).map(([name]) => `hc ${name} vegad start failed`);
      this.startError = bad.join("; ") || null;
      cx.notify();
    } finally { this.starting = false; }
  }

  // The shipped status line (workspace-status) without the palette hint; vega's own
  // work (badges, vegad, inbox) runs in the background and shows nothing here.
  render(_cx) {
    const t = theme();
    const space = spaces().find(s => s.active);
    const list = agents();
    const shown = [this.error, this.startError].filter(Boolean).join("; ");
    const needs = list.filter(a => a.status === "blocked").length;
    const label = (icon, text) => h_flex().gap(6).items_center().child(svg(`mods/workspace-chrome/${icon}.svg`).size(13)).child(text);
    return h_flex().size_full().px(14).gap(16).items_center().bg(t.chrome).border_t(1).border_color(t.border).text_size(11.8).text_color(t.muted)
      .child(label(space?.machine ? "server" : "monitor", space?.machine ?? "This computer"))
      .when(Boolean(space?.branch), el => el.child(label("branch", space.branch)))
      .child(h_flex().gap(6).items_center().child(div().size(6).rounded(3).bg(t.dim)).child(`${list.length} agents`)
        .when(needs > 0, el => el.child(div().text_color(t.blocked).child(`· ${needs} needs you`))))
      // A failed badge read or remote vegad start (a machine unreachable, a reader error) shows at the right end.
      .when(Boolean(shown), el => el.child(div().flex_1()).child(div().id("vega-error").truncate().text_color(t.blocked).tooltip(shown).child(`vega: ${shown}`)));
  }
}
