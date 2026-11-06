import { View } from "gpui-kit";
import { h_flex } from "gpui-base";
import { exec, home, theme } from "herdr";

export default class Vega extends View {
  init(_props, cx) {
    this.state = "checking";
    this.refreshing = false;
    cx.spawn(async (cx) => {
      try {
        await exec(`${home()}/.local/bin/vega-post`, ["relay", "start"]);
      } catch (_) {
        // Startup failure must not break the status element.
      }
      await this.refresh(cx);
    });
    cx.timer.every(15000, (cx) => {
      cx.spawn(async (cx) => { await this.refresh(cx); });
    });
  }

  async refresh(cx) {
    if (this.refreshing) return;
    this.refreshing = true;
    let state = "unavailable";
    try {
      const result = await exec(`${home()}/.local/bin/vega-post`, ["relay", "status", "--json"]);
      if (result.code === 0) {
        const status = JSON.parse(result.stdout);
        if (typeof status.running === "boolean") {
          state = status.running ? "running" : "stopped";
        }
      }
    } catch (_) {
      // Missing binary, malformed status or an unreachable core stays visible.
    } finally {
      this.refreshing = false;
    }
    if (this.state !== state) {
      this.state = state;
      cx.notify();
    }
  }

  render(_cx) {
    const t = theme();
    return h_flex().size_full().items_center().px(12).text_size(12)
      .bg(t.chrome).text_color(this.state === "running" ? t.muted : t.working)
      .child(`vega · vegad ${this.state}`);
  }
}
