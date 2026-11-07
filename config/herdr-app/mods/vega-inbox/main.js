import { View, div } from "gpui-kit";
import { v_flex, h_flex, InputState, Input } from "gpui-base";
import { shared, setShared, nextCommand, props, theme, exec, run, closeOverlay, terminals } from "herdr";

// hc --json wraps base64 stdout/stderr and the program's exit code. Decode in
// Python (already required for badges), retaining argv boundaries and Unicode.
const remoteAction = `import sys,json,base64,subprocess
machine,cwd,program,*args=sys.argv[1:]
p=subprocess.run(['hc','exec','--json','--on',machine,'--cwd',cwd,'--timeout','30','--',program,*args],capture_output=True,text=True,timeout=35)
if p.returncode: print(json.dumps({'code':p.returncode,'stdout':p.stdout,'stderr':p.stderr}))
else:
 v=json.loads(p.stdout)
 print(json.dumps({'code':v['code'],'stdout':base64.b64decode(v['stdout'],validate=True).decode('utf-8',errors='replace'),'stderr':base64.b64decode(v['stderr'],validate=True).decode('utf-8',errors='replace')}))`;

function decisionKey(row, id) { return `${row.machine ? row.machine + "/" : ""}${row.project}/${id}`; }

export default class VegaInbox extends View {
  init(_props, cx) {
    this.inputs = new Map();
    this.busy = new Set();
    this.results = new Map();
    setShared("vega.inbox-project", props()?.project ?? null);
    this.prepare(cx);
    setShared("vega.inbox-open", true);
    cx.timer.every(15000, cx => { this.prepare(cx); cx.notify(); });
    cx.spawn(async cx => {
      for (;;) { await nextCommand(); this.prepare(cx); cx.notify(); }
    });
  }

  close() { setShared("vega.inbox-open", false); }

  prepare(cx) {
    for (const row of shared("vega.badges") ?? []) {
      for (const d of row.decision_texts ?? []) this.input(decisionKey(row, d.id), cx);
    }
  }

  input(key, cx) {
    if (!this.inputs.has(key)) {
      const input = InputState.new({placeholder: "Your answer (or dismissal reason)…"});
      input.on("change", (_e,cx) => cx.notify());
      this.inputs.set(key, input);
    }
    // Never focus automatically: new decisions must not steal the keyboard.
    return this.inputs.get(key);
  }

  act(row, decision, dismiss, cx) {
    const key = decisionKey(row, decision.id);
    const text = this.inputs.get(key)?.value().trim() ?? "";
    if (this.busy.has(key)) return;
    if (!row.checkout || !text) {
      this.results.set(key, !row.checkout ? "Project checkout unavailable; refresh the UAC claim." : "Enter an answer or dismissal reason.");
      cx.notify(); return;
    }
    this.busy.add(key); this.results.delete(key); cx.notify();
    cx.spawn(async (cx) => {
      try {
        const args = dismiss ? ["close", decision.id, "--reason", text] : ["uac", "answer", decision.id, text];
        let r;
        if (row.machine) {
          const wrapped = await exec("python3", ["-c", remoteAction, row.machine, row.checkout, dismiss ? "bd" : "vega", ...args]);
          r = wrapped.code === 0 ? JSON.parse(wrapped.stdout) : wrapped;
        } else {
          r = await exec(dismiss ? "bd" : "vega", args, {cwd: row.checkout});
        }
        this.results.set(key, r.code === 0 ? (dismiss ? "Dismissed. Waiting for refresh." : "Answer sent to the UAC.") : r.stdout || r.stderr || `Command failed (${r.code})`);
      } catch (e) { this.results.set(key, String(e)); }
      finally { this.busy.delete(key); cx.notify(); }
    });
  }

  button(id, label, action, t) {
    return div().id(id).px(12).py(6).rounded(6).bg(t.raised).cursor_pointer().child(label)
      .on_mouse_down("left", (_e,cx) => cx.stop_propagation())
      .on_click((_e,cx) => {cx.stop_propagation(); action(cx);});
  }

  render(cx) {
    const t = theme();
    const project = shared("vega.inbox-project");
    const rows = (shared("vega.badges") ?? []).filter(r => Date.now()/1000-r.at <= 60 && (!project || r.project === project));
    const live = new Set(terminals().filter(t => t.state === "live").map(t => t.address));
    const cards = rows.flatMap(row => (row.decision_texts ?? []).map(d => {
      const key = decisionKey(row, d.id);
      const input = this.inputs.get(key);
      if (!input) return div().child("Loading decision…");
      return v_flex().id(key).flex_none().gap(8).p(12).rounded(8).bg(t.surface)
        .child(div().text_size(11).text_color(t.muted).child(`${row.machine ? row.machine + " · " : ""}${row.project} · ${d.id}`))
        .child(div().font_semibold().child(d.title))
        .child(div().child(d.question || "No question text provided."))
        .child(Input.new(input).h(36).px(10).rounded(6).border_1().border_color(t.input).bg(t.background).text_color(t.foreground))
        .child(h_flex().gap(8)
          .child(this.button(`answer-${key}`, this.busy.has(key) ? "Sending…" : "Answer", cx => this.act(row,d,false,cx),t))
          .child(this.button(`dismiss-${key}`, "Dismiss", cx => this.act(row,d,true,cx),t)))
        .when(this.results.has(key), el => el.child(div().text_size(12).text_color(t.muted).child(this.results.get(key))));
    }));
    const waiting = rows.flatMap(row => (row.waiting_panes ?? []).map(w => ({...w, project: row.project})))
      .filter(w => live.has(w.pane.replace(/^hc:/,"")));
    return v_flex().size_full().gap(12).p(16).bg(t.chrome).text_size(13).text_color(t.foreground)
      .child(h_flex().flex_none().justify_between().items_center().child(div().font_semibold().child("Vega inbox"))
        .child(this.button("close-inbox","Close",() => closeOverlay(props().overlay),t)))
      .child(v_flex().id("inbox-cards").flex_1().min_h(0).overflow_y_scrollbar().gap(12)
        .when(!cards.length, el => el.child("No open decisions."))
        .children(cards)
        .child(div().font_semibold().child(`Waiting panes · ${waiting.length}`))
        .children(waiting.map(w => this.button(`waiting-${w.pane}`,`${w.name} · ${w.title || w.attention}`,
          () => run("agent.focus",{terminal:w.pane.replace(/^hc:/,"")}),t))));
  }
}
