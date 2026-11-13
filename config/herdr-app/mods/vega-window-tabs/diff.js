// Diff renderer extracted from herdr-app 0.1.15 mods/changes/main.js; see UPSTREAM.md.
import { div } from "gpui-kit";
import { h_flex, Scrollbar, v_virtual_list } from "gpui-base";
import { settings } from "herdr";

// Text draws a tab as nothing: pad each to the next 4-column stop.
const untab = (text) => text.replace(/[^\t]*\t/g, (s) => s.slice(0, -1).padEnd((s.length - 1) - (s.length - 1) % 4 + 4));

// Numbers each diff line once, so a frame only draws the rows on screen.
export function diffRows(lines) {
  let old = 0, next = 0;
  return lines.map((line) => {
    const match = /^@@ -(\d+)(?:,\d+)? \+(\d+)/.exec(line);
    if (match) { old = +match[1]; next = +match[2]; return { kind: "@", text: line }; }
    const kind = line[0];
    if (kind === "-") return { kind, number: old++, text: untab(line.slice(1)) || " " };
    if (kind === "+") return { kind, number: next++, text: untab(line.slice(1)) || " " };
    if (kind === " ") { old++; return { kind, number: next++, text: untab(line.slice(1)) || " " }; }
    return { kind: "", text: line };
  });
}

function diffRow({ kind, number, text }, t) {
  if (kind === "@") return div().w_full().px(12).h(20).flex_none().truncate().bg(t.raised).text_color(t.dim).child(text);
  return h_flex().w_full().h(20).flex_none().gap(10).when(kind === "+", (e) => e.bg(t.ansi2 + "18")).when(kind === "-", (e) => e.bg(t.ansi1 + "18"))
    .child(div().w(30).flex_none().text_right().text_color(t.dim).child(number === undefined ? "" : String(number)))
    .child(div().flex_1().min_w(0).truncate().child(text));
}

export function renderDiff(rows, t) {
  const width = Math.max(300, ...rows.map(row => (row.text.length + 1) * 7 + 50));
  return div().flex_1().min_h(0).relative().font_family(settings().font_family ?? "JetBrains Mono").text_size(11)
    .child(div().id("changes-diff-x").size_full().overflow_x_scrollbar()
      .child(div().w_full().min_w(width).h_full().child(v_virtual_list("changes-diff", rows.length, 20, (i) => String(i),
        (range) => rows.slice(range.start, range.end).map((row) => diffRow(row, t))).size_full())))
    .child(Scrollbar.vertical("changes-diff").viewport_from_layout().absolute().inset_0());
}
