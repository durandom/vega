import { View, div } from "gpui-kit";
import { h_flex, v_flex, InputState, ScrollHandle } from "gpui-base";
import { layout, nextCommand, run, theme, props, exec, machines, openOverlay, closeOverlay, pickFolder } from "herdr";
import { renderTabs } from "./tabs.js";
import { diffRows, renderDiff } from "./diff.js";
import { tabStack, targetTab } from "./navigation.js";

export default class WindowTabs extends View {
  init(_props, cx) {
    this.error = "";
    this.busy = false;
    this.pressed = null;
    this.scroll = ScrollHandle.new();
    this.rows = [];
    this.editing = null;
    this.mode = props()?.mode;
    if (this.mode === "config") this.refresh(cx);
    if (this.mode) return;
    cx.spawn(async cx => {
      for (;;) {
        const command = await nextCommand();
        const stack = tabStack(layout());
        const child = targetTab(stack, command);
        if (child) run("layout.select", { stack: stack.id, child });
        else if (command === "new") this.open("add");
        else if (command === "close" && stack?.active) this.change(["close", this.profile(), stack.active], cx);
        else if (command === "rename" && stack?.active) this.rename(stack.active, cx);
      }
    });
  }

  profile() { return props()?.profile; }
  open(mode) {
    openOverlay({ mod: "vega-window-tabs", kind: "dialog", width: mode === "config" ? 700 : 300,
      height: mode === "config" ? 540 : 360, props: { profile: this.profile(), mode } });
  }
  refresh(cx) {
    cx.spawn(async cx => {
      try {
        const r = await exec("vega", ["window", "show", this.profile()]);
        if (r.code !== 0) throw new Error(r.stdout || r.stderr);
        const diff = JSON.parse(r.stdout).diff;
        if (typeof diff !== "string") throw new Error("Update vega: window show did not return a diff");
        const lines = diff.trimEnd().split("\n");
        const first = lines.findIndex(line => line.startsWith("@@"));
        this.rows = diffRows(first < 0 ? [] : lines.slice(first));
        this.loaded = true; this.error = "";
      } catch (e) { this.error = String(e); }
      cx.notify();
    });
  }
  change(args, cx) {
    if (this.busy) return;
    this.busy = true; this.error = ""; cx.notify();
    cx.spawn(async cx => {
      try {
        const r = await exec("vega", ["window", ...args]);
        if (r.code !== 0) throw new Error(r.stdout || r.stderr);
        if (this.mode === "add") closeOverlay(props().overlay);
        if (this.mode === "config") {
          this.message = args[0] === "promote" ? "Written to checkout. Review/commit, rebuild/install, then apply and reset." : "Local changes reset.";
          this.refresh(cx);
        }
      } catch (e) { this.error = String(e); }
      this.busy = false; cx.notify();
    });
  }
  rename(id, cx) {
    if (this.busy) return;
    this.editing = id;
    this.input = InputState.new({ value: props()?.titles?.[id] ?? id });
    const finish = (_e, cx) => {
      if (this.editing !== id) return;
      this.editing = null;
      this.change(["rename", this.profile(), id, this.input.value()], cx);
    };
    this.input.on("submit", finish);
    this.input.on("blur", finish);
    this.input.focus(); cx.notify();
  }
  render() {
    const t = theme();
    const button = (id, label, action) => div().id(id).px(10).py(6).rounded(5)
      .hover(s => s.bg(t.hover)).on_mouse_down("left", (_e,cx) => cx.stop_propagation())
      .on_click((e,cx) => { cx.stop_propagation(); if (!this.busy) action(e,cx); }).child(label);
    if (this.mode) {
      const body = v_flex().size_full().p(14).gap(8).bg(t.background).text_color(t.foreground)
        .child(h_flex().justify_between().child(this.mode === "add" ? "Add shell on machine" : "Applied repo → effective profile")
          .child(button("dismiss", "×", () => closeOverlay(props().overlay))));
      if (this.mode === "add") {
        const names = ["local", ...machines().filter(m => !m.temporary && m.name).map(m => m.name)];
        body.child(v_flex().id("machines").flex_1().overflow_y_scrollbar().children([...new Set(names)].map(name =>
          button(`machine-${name}`, name, (_e,cx) => this.change(["add", this.profile(), "--machine", name], cx)))));
      } else {
        body.child(this.rows.length ? renderDiff(this.rows, t) : div().flex_1().child(this.loaded ? "No local changes" : this.error || "Loading…"))
          .child(h_flex().gap(8)
            .child(button("promote", "Promote to checkout…", (_e,cx) => cx.spawn(async cx => {
              try {
                const repo = await pickFolder({ prompt: "Choose the vega checkout" });
                if (repo) this.change(["promote", this.profile(), "--repo", repo], cx);
              } catch(e) { this.error = String(e); cx.notify(); }
            })))
            .child(button("reset", "Reset local changes", (_e,cx) => this.change(["reset", this.profile()], cx))))
          .child(this.message ?? "Promote writes an uncommitted profile. Reset discards local tab edits.");
      }
      return body.child(this.busy ? "Saving…" : this.error);
    }
    return renderTabs.call(this);
  }
}
