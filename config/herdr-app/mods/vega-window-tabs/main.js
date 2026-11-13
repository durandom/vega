import { View, div } from "gpui-kit";
import { h_flex } from "gpui-base";
import { layout, nextCommand, run, theme } from "herdr";
import { tabStack, targetTab } from "./navigation.js";

export default class WindowTabs extends View {
  init(_props, cx) {
    cx.spawn(async () => {
      for (;;) {
        const command = await nextCommand();
        const stack = tabStack(layout());
        const child = targetTab(stack, command);
        if (child) run("layout.select", { stack: stack.id, child });
      }
    });
  }

  render() {
    const stack = tabStack(layout());
    const t = theme();
    return h_flex().size_full().items_center().gap(4).px(8).bg(t.chrome)
      .children((stack?.children ?? []).map(tab =>
        div().id(`group-${tab.id}`).px(14).py(6).rounded(5).text_size(13)
          .bg(tab.id === stack.active ? t.selected : t.chrome)
          .text_color(tab.id === stack.active ? t.foreground : t.muted)
          .hover(style => style.bg(t.hover))
          .on_click(() => run("layout.select", { stack: stack.id, child: tab.id }))
          .child(tab.id)));
  }
}
