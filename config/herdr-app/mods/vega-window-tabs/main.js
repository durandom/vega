import { View, div } from "gpui-kit";
import { h_flex, v_flex, Input, InputState } from "gpui-base";
import { layout, nextCommand, run, theme, props, exec, machines, openOverlay, closeOverlay, pickFolder } from "herdr";
import { tabStack, targetTab } from "./navigation.js";

export default class WindowTabs extends View {
  init(_props, cx) {
    this.error = "";
    this.busy = false;
    this.pressed = null;
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
        this.config = JSON.parse(r.stdout).config;
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
      const body = v_flex().size_full().p(14).gap(8).bg(t.surface).text_color(t.foreground)
        .child(h_flex().justify_between().child(this.mode === "add" ? "Add shell on machine" : "Effective window configuration")
          .child(button("dismiss", "×", () => closeOverlay(props().overlay))));
      if (this.mode === "add") {
        const names = ["local", ...machines().filter(m => !m.temporary && m.name).map(m => m.name)];
        body.child(v_flex().id("machines").flex_1().overflow_y_scrollbar().children([...new Set(names)].map(name =>
          button(`machine-${name}`, name, (_e,cx) => this.change(["add", this.profile(), "--machine", name], cx)))));
      } else {
        body.child(div().id("config-text").flex_1().overflow_y_scrollbar().text_size(12).child(this.config ?? "Loading…"))
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
    const stack = tabStack(layout());
    return h_flex().size_full().items_center().gap(4).px(8).bg(t.chrome).text_color(t.foreground)
      .on_mouse_up("left", () => { this.pressed = null; })
      .child(h_flex().id("tab-scroll").flex_1().min_w(0).h_full().items_center().gap(4).overflow_x_scrollbar()
        .children((stack?.children ?? []).map((tab,index) =>
          h_flex().id(`group-${tab.id}`).px(8).py(4).gap(6).rounded(5).text_size(13).flex_none()
            .bg(tab.id === stack.active ? t.selected : t.chrome)
            .hover(style => style.bg(t.hover))
            .on_mouse_down("left", (_e,cx) => { cx.stop_propagation(); this.pressed = tab.id; })
            .on_mouse_up("left", (_e,cx) => {
              cx.stop_propagation();
              const from = this.pressed; this.pressed = null;
              if (from && from !== tab.id) this.change(["move", this.profile(), from, String(index + 1)], cx);
              else run("layout.select", { stack: stack.id, child: tab.id });
            })
            .child(this.editing === tab.id ? Input.new(this.input).w(160) :
              div().id(`name-${tab.id}`).child(props()?.titles?.[tab.id] ?? tab.id)
                .on_click((_e,cx) => { cx.stop_propagation(); this.rename(tab.id,cx); }))
            .child(button(`close-${tab.id}`, "×", (_e,cx) => this.change(["close", this.profile(), tab.id], cx))))))
      .child(button("add", "+", () => this.open("add")))
      .child(button("config", "Config", () => this.open("config")))
      .when(Boolean(this.error), el => el.child(div().text_color(t.blocked).child(this.error)));
  }
}
