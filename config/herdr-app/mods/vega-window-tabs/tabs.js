// Fork of herdr-app 0.1.15 mods/tabs/main.js; see UPSTREAM.md.
import { div, svg, image } from "gpui-kit";
import { h_flex, Input } from "gpui-base";
import { terminals, run, theme, window, layout, settings, props } from "herdr";
import { tabStack, targetTab } from "./navigation.js";

function find(node, id) {
  if (node?.id === id) return node;
  for (const child of node?.children ?? []) {
    const found = find(child, id);
    if (found) return found;
  }
  return null;
}

function slotAddresses(node) {
  return [node?.content?.terminal, ...(node?.children ?? []).flatMap(slotAddresses)].filter(Boolean);
}

export function renderTabs() {
  const t = theme();
  const stack = tabStack(layout());
  const shown = (stack?.children ?? []).map(c => ({ ...c, active: c.id === stack.active }));
  const active = shown.findIndex(tab => tab.active);
  const selected = shown[active]?.id;
  if (selected !== this.selected || active !== this.selectedIndex) {
    this.selected = selected;
    this.selectedIndex = active;
    if (active >= 0) this.scroll.scroll_to_item(active);
  }
  const stop = (_e, cx) => cx.stop_propagation();
  const button = (id, label, action) => div().id(id).size(26).flex_none().flex().items_center().justify_center().rounded(6).text_color(t.dim).hover(s => s.bg(t.hover).text_color(t.foreground))
    .on_mouse_down("left", stop).on_click(action).child(label);
  const item = (c, i) => {
    const addresses = slotAddresses(c);
    const live = terminals().filter(p => addresses.includes(p.address));
    const program = live.find(p => p.program?.agent)?.program ?? live[0]?.program;
    const name = props()?.titles?.[c.id] ?? c.id;
    return h_flex().id(`tab-${c.id}`).h(29).px(10).gap(7).rounded(7).items_center().flex_none().text_size(12.8)
      .when(i <= active, el => el.on_size_change((_size, cx) => { this.scroll.scroll_to_item(active); cx.notify(); }))
      .border(1).border_color(c.active ? t.border : t.chrome).bg(c.active ? t.raised : t.chrome).text_color(c.active ? t.foreground : t.muted)
      .hover(s => s.bg(t.hover))
      .on_mouse_down("left", (e, cx) => { cx.stop_propagation(); this.pressed = c.id; if (e.click_count === 2) { this.pressed = null; this.rename(c.id, cx); } })
      .on_mouse_up("left", (_e, cx) => {
        const from = this.pressed; this.pressed = null;
        if (from !== null && from !== c.id && shown.some(tab => tab.id === from)) {
          // CLI positions include optional groups that are absent from the layout.
          const order = props()?.order ?? shown.map(tab => tab.id);
          this.change(["move", this.profile(), from, String(order.indexOf(c.id) + 1)], cx);
        } else run("layout.select", { stack: stack.id, child: c.id });
      })
      .when(Boolean(program?.icon), el => el.child(program.tint ? svg(program.icon).size(14).text_color(t.muted) : image(program.icon).size(14)))
      // Like stock, keep the label passive: no competing click target during drag.
      .child(this.editing === c.id ? div().id(`edit-${c.id}`).on_mouse_down("left", stop).child(Input.new(this.input).w(160)) : div().max_w(170).truncate().child(name))
      .when(c.active, el => el.child(div().id(`close-${c.id}`).text_color(t.dim).on_mouse_down("left", stop).on_click((_e, cx) => { cx.stop_propagation(); this.change(["close", this.profile(), c.id], cx); }).child(svg("mods/pane/close.svg").size(12))));
  };
  const root = layout(), sidebar = find(root, "sidebar"), w = window();
  const lightsPadding = w?.platform === "macos" && w.titlebar === "none" && !w.fullscreen && root?.traffic_lights
    ? Math.max(10, (root.traffic_lights[0] + 64) / settings().zoom - (sidebar?.hidden ? 0 : sidebar?.size?.px ?? 0))
    : 10;
  const configButton = h_flex().id("config").h(26).px(8).gap(5).flex_none().items_center().rounded(6)
    .text_size(12).bg(t.chrome).text_color(t.muted).hover(s => s.bg(t.hover)).on_mouse_down("left", stop)
    .on_click(() => this.open("config")).child(svg("mods/vega-window-tabs/changes.svg").size(14)).child("Config");
  return h_flex().size_full().min_w(0).px(10).pl(lightsPadding).gap(3).items_center().bg(t.chrome).border_b(1).border_color(t.border)
    .on_mouse_up("left", () => { this.pressed = null; })
    .on_scroll_wheel(e => { if (Math.abs(e.delta.y) >= 1) { const child = targetTab(stack, e.delta.y > 0 ? "prev" : "next"); if (child) run("layout.select", { stack: stack.id, child }); } })
    .child(h_flex().id("tab-scroll").flex_1().min_w(0).h_full().items_center().gap(3).overflow_x_scrollbar().track_scroll(this.scroll)
      .on_size_change((_size, cx) => { if (active >= 0) { this.scroll.scroll_to_item(active); cx.notify(); } })
      .children(shown.map(item)))
    .child(button("tab-new", "+", () => this.open("add")))
    .child(configButton)
    .when(w?.buttons, el => el.child(button("minimize", w.platform === "windows" ? svg("mods/vega-window-tabs/win-min.svg").size(16) : "−", () => run("window.minimize")))
      .child(button("maximize", w.platform === "windows" ? svg(`mods/vega-window-tabs/${w.maximized ? "win-restore" : "win-max"}.svg`).size(16) : "□", () => run("window.maximize")))
      .child(button("close-window", w.platform === "windows" ? svg("mods/vega-window-tabs/win-close.svg").size(16) : svg("mods/pane/close.svg").size(13), () => run("window.close"))))
    .when(Boolean(this.error), el => el.child(div().text_color(t.blocked).child(this.error)));
}
