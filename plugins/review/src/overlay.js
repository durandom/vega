// vega review overlay (docs/design/live-review.md). agent-browser injects it
// with --init-script before any page script; `vega review start` replaces the
// config placeholder. The secret lives only in this closure, never on window.
(function (CFG) {
  "use strict";
  if (window.top !== window || location.origin !== CFG.origin) return;
  // Taken before page scripts run, so the app cannot patch them to read the secret.
  const F = window.fetch.bind(window);
  const setIntervalF = window.setInterval.bind(window);
  const setTimeoutF = window.setTimeout.bind(window);
  const clearTimeoutF = window.clearTimeout.bind(window);
  const MO = window.MutationObserver;
  const FunctionF = Function;

  const COLORS = {
    draft: "#7c3aed", queued: "#64748b", sent: "#2563eb", working: "#d97706", done: "#16a34a",
    skipped: "#94a3b8",
  };
  const PANEL_KEY = "vega-review-panel";
  const HELP = "Annotate (Alt+A), then click any element or select text and write a note. " +
    "Or write a note on the whole page below. Notes wait here as drafts: edit or remove them, then Send delivers all at once. " +
    "Links and buttons do not fire while annotating; Esc stops. Alt+click works anytime.";
  const CSS_TEXT = `
    :host { all: initial; }
    * { box-sizing: border-box; font: 13px/1.4 system-ui, -apple-system, sans-serif; }
    .layer { position: fixed; inset: 0; pointer-events: none; }
    .hover { position: fixed; outline: 2px dashed #7c3aed; background: rgba(124,58,237,.08); display: none; }
    .hl { position: fixed; outline: 3px solid #f59e0b; background: rgba(245,158,11,.10); border-radius: 3px; }
    .hl-label { position: absolute; left: -3px; bottom: 100%; margin-bottom: 4px; width: max-content; max-width: 320px;
      background: #f59e0b; color: #111; padding: 3px 7px; border-radius: 4px; font-weight: 600; white-space: pre-wrap; }
    .hl.off { display: none; }
    .hl-x { background: none; color: #111; padding: 0 0 0 6px; font-weight: 700; }
    .pin { position: fixed; width: 22px; height: 22px; margin: -11px 0 0 -11px; border-radius: 50%;
      color: #fff; font-weight: 700; display: flex; align-items: center; justify-content: center;
      pointer-events: auto; cursor: pointer; border: 2px solid #fff; box-shadow: 0 1px 4px rgba(0,0,0,.4); }
    .pin.working { animation: pulse 1s ease-in-out infinite; }
    @keyframes pulse { 50% { transform: scale(1.25); } }
    .pop, .panel { position: fixed; pointer-events: auto; background: #111827; color: #f9fafb; border-radius: 8px;
      box-shadow: 0 6px 24px rgba(0,0,0,.35); }
    .pop { padding: 10px; width: 300px; max-width: calc(100vw - 16px); }
    p { margin: 0; white-space: pre-wrap; overflow-wrap: anywhere; }
    .sel { color: #9ca3af; font-size: 11px; overflow-wrap: anywhere; }
    .quote { border-left: 3px solid #7c3aed; padding-left: 6px; color: #d1d5db; font-style: italic;
      white-space: pre-wrap; overflow-wrap: anywhere; max-height: 6em; overflow: auto; margin: 4px 0; }
    textarea { width: 100%; min-height: 70px; margin: 6px 0; border-radius: 6px; border: 1px solid #374151;
      background: #1f2937; color: #f9fafb; padding: 6px; resize: vertical; }
    .row { display: flex; gap: 6px; justify-content: flex-end; align-items: center; }
    button { pointer-events: auto; cursor: pointer; border: 0; border-radius: 6px; padding: 4px 9px;
      background: #374151; color: #f9fafb; }
    button.primary, button.on { background: #7c3aed; }
    button:disabled { opacity: .45; cursor: default; }
    button.link { background: none; padding: 0 4px; color: #93c5fd; }
    .panel { width: 340px; max-width: calc(100vw - 16px); display: flex; flex-direction: column;
      max-height: calc(100vh - 16px); }
    .head { display: flex; gap: 6px; align-items: center; padding: 6px 8px; cursor: move; user-select: none;
      touch-action: none; }
    .head .name { font-weight: 700; flex: 1; }
    .head .count { color: #9ca3af; font-size: 11px; }
    .page-note { min-height: 44px; margin: 0; }
    .body { padding: 0 8px 8px; display: flex; flex-direction: column; gap: 6px; min-height: 0; }
    .panel.collapsed .body { display: none; }
    .help { color: #9ca3af; font-size: 12px; }
    .msg { color: #d1d5db; font-size: 12px; }
    .list { overflow: auto; min-height: 0; max-height: 50vh; display: flex; flex-direction: column; gap: 6px; }
    .item { background: #1f2937; border-radius: 6px; padding: 6px 8px; border-left: 4px solid #64748b; }
    .item.flash { outline: 2px solid #f59e0b; }
    .item .top { display: flex; gap: 6px; align-items: baseline; }
    .item .top .sel { flex: 1; }
    .reply { margin-top: 6px; padding-top: 6px; border-top: 1px solid #374151; }
    .who { color: #9ca3af; font-size: 11px; }
    .hls { display: flex; flex-direction: column; gap: 4px; }
    .hls .row { justify-content: space-between; }
    .hlitem { display: flex; gap: 6px; align-items: baseline; background: #1f2937; border-radius: 6px;
      padding: 4px 8px; border-left: 4px solid #f59e0b; }
    .hlitem.off { border-left-color: #4b5563; color: #9ca3af; }
    .hlitem .what { flex: 1; overflow-wrap: anywhere; cursor: pointer; }
  `;

  let host = null, root = null, layer = null, hoverBox = null;
  let panel = null, body = null, list = null, msg = null, countEl = null, annotateBtn = null;
  let collapseBtn = null, sendBtn = null, hlSection = null;
  let annotate = false, pop = null, editing = null, quoteAt = 0, lastState = "";
  let notes = [];          // all notes of the session, from the inbox
  const highlights = [];   // {el, sel, label, box, off}; off: the owner hid it, only in this page view
  const patches = new Map(); // name -> {code, fn, runs, error}: the agent's design patches
  let patchTimer = null, patchObserver = null;

  const el = (tag, cls, text) => {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  };
  const button = (cls, text, onClick) => {
    const b = el("button", cls, text);
    b.addEventListener("click", (ev) => { ev.stopPropagation(); if (ev.isTrusted) onClick(ev); });
    return b;
  };
  const ours = (e) => host && (e === host || host.contains(e));
  const store = {
    get() { try { return JSON.parse(localStorage.getItem(PANEL_KEY)) || {}; } catch (_) { return {}; } },
    set(v) { try { localStorage.setItem(PANEL_KEY, JSON.stringify(v)); } catch (_) { /* private mode */ } },
  };

  function mount() {
    if (host && host.isConnected) return;
    if (!host) {
      host = document.createElement("vega-review");
      root = host.attachShadow({ mode: "open" });
      const style = el("style"); style.textContent = CSS_TEXT;
      layer = el("div", "layer");
      hoverBox = el("div", "hover");
      layer.append(hoverBox);
      buildPanel();
      root.append(style, layer, panel);
    }
    document.documentElement.appendChild(host);
  }

  // The floating panel: drag by its head, collapse, position kept per origin.
  function buildPanel() {
    panel = el("div", "panel");
    const head = el("div", "head");
    countEl = el("span", "count");
    annotateBtn = button("", "Annotate", () => setAnnotate(!annotate));
    annotateBtn.title = "Alt+A: click any element or select text to write a note";
    collapseBtn = button("", "–", () => setCollapsed(!panel.classList.contains("collapsed")));
    head.append(el("span", "name", "vega review"), countEl, annotateBtn, collapseBtn);
    body = el("div", "body");
    msg = el("div", "msg");
    list = el("div", "list");
    hlSection = el("div", "hls");
    // A note on the page itself, no element picked first.
    const pageTa = el("textarea", "page-note");
    pageTa.placeholder = "Note on this page (Enter adds it, Shift+Enter new line)";
    const addPage = async (ev) => {
      if (!ev.isTrusted) return;
      const text = pageTa.value.trim();
      if (!text) return;
      if (await addNote(null, "", "", text)) pageTa.value = "";
    };
    pageTa.addEventListener("keydown", (ev) => {
      ev.stopPropagation();
      if (ev.key === "Enter" && !ev.shiftKey) { ev.preventDefault(); addPage(ev); }
    });
    sendBtn = button("primary", "Send", sendDrafts);
    const row = el("div", "row"); row.append(button("", "Add page note", addPage), sendBtn);
    body.append(el("div", "help", HELP), msg, hlSection, list, pageTa, row);
    renderHighlights();
    panel.append(head, body);
    const saved = store.get();
    if (saved.collapsed) panel.classList.add("collapsed");
    collapseBtn.textContent = saved.collapsed ? "+" : "–";
    if (Number.isFinite(saved.x) && Number.isFinite(saved.y)) moveTo(saved.x, saved.y);
    else Object.assign(panel.style, { right: "12px", bottom: "12px" });

    head.addEventListener("pointerdown", (ev) => {
      if (!ev.isTrusted || ev.button !== 0 || ev.target.closest("button")) return;
      const r = panel.getBoundingClientRect();
      const dx = ev.clientX - r.left, dy = ev.clientY - r.top;
      head.setPointerCapture(ev.pointerId);
      const move = (e) => moveTo(e.clientX - dx, e.clientY - dy);
      const up = () => {
        head.removeEventListener("pointermove", move);
        head.removeEventListener("pointerup", up);
        save();
      };
      head.addEventListener("pointermove", move);
      head.addEventListener("pointerup", up);
    });
  }

  function moveTo(x, y) {
    const r = panel.getBoundingClientRect();
    const w = r.width || 340, h = r.height || 40;
    x = Math.max(0, Math.min(x, innerWidth - w));
    y = Math.max(0, Math.min(y, innerHeight - h));
    Object.assign(panel.style, { left: x + "px", top: y + "px", right: "auto", bottom: "auto" });
  }
  function save() {
    const r = panel.getBoundingClientRect();
    const v = { collapsed: panel.classList.contains("collapsed") };
    if (panel.style.left !== "") Object.assign(v, { x: Math.round(r.left), y: Math.round(r.top) });
    store.set(v);
  }
  function setCollapsed(on) {
    panel.classList.toggle("collapsed", on);
    collapseBtn.textContent = on ? "+" : "–";
    save();
    keepInView();
  }
  // A grown or expanded panel, or a smaller window, must not push it out of view.
  function keepInView() {
    if (panel.style.left !== "") { const r = panel.getBoundingClientRect(); moveTo(r.left, r.top); }
  }

  function setAnnotate(on) {
    annotate = on;
    annotateBtn.className = on ? "on" : "";
    hoverBox.style.display = "none";
    say(on ? "Annotating: click an element or select text (Esc stops)" : "");
  }

  function say(text) { if (msg) msg.textContent = text; }

  function unique(sel) {
    try { return document.querySelectorAll(sel).length === 1; } catch (_) { return false; }
  }
  const attr = (v) => '"' + v.replace(/["\\]/g, "\\$&") + '"';

  // A short selector that matches only this element.
  function selectorOf(target) {
    if (target.id && unique("#" + CSS.escape(target.id))) return "#" + CSS.escape(target.id);
    const tag = target.tagName.toLowerCase();
    for (const a of ["data-testid", "data-test", "data-cy", "name", "aria-label"]) {
      const v = target.getAttribute(a);
      if (v && unique(`${tag}[${a}=${attr(v)}]`)) return `${tag}[${a}=${attr(v)}]`;
    }
    const parts = [];
    for (let e = target; e && e.nodeType === 1 && e !== document.documentElement; e = e.parentElement) {
      if (e.id && unique("#" + CSS.escape(e.id))) { parts.unshift("#" + CSS.escape(e.id)); break; }
      let part = e.tagName.toLowerCase();
      const parent = e.parentElement;
      if (parent) {
        const same = [...parent.children].filter((s) => s.tagName === e.tagName);
        if (same.length > 1) part += `:nth-of-type(${same.indexOf(e) + 1})`;
      }
      parts.unshift(part);
      if (unique(parts.join(" > "))) break;
    }
    return parts.join(" > ");
  }

  function find(sel) { try { return document.querySelector(sel); } catch (_) { return null; } }

  function place(box, target) {
    const r = target.getBoundingClientRect();
    Object.assign(box.style, { left: r.left + "px", top: r.top + "px", width: r.width + "px", height: r.height + "px" });
    return r;
  }

  function closePop() { if (pop) { pop.remove(); pop = null; } }

  // The note editor for an element, or a text passage in it (`quote`).
  function openPop(target, quote) {
    if (pop && pop.querySelector("textarea").value.trim()) {
      pop.querySelector("textarea").focus();
      say("Finish or cancel the open note first");
      return;
    }
    closePop();
    const sel = selectorOf(target);
    const r = target.getBoundingClientRect();
    pop = el("div", "pop");
    const ta = el("textarea");
    ta.placeholder = "Your note for the agent (Enter adds it, Shift+Enter new line)";
    const add = (ev) => {
      if (!ev.isTrusted) return;   // only the owner's own input makes notes
      const text = ta.value.trim();
      if (!text) return;
      addNote(target, sel, quote, text);
      closePop();
    };
    const row = el("div", "row");
    row.append(button("", "Cancel", closePop), button("primary", "Add note", add));
    pop.append(el("div", "sel", sel));
    if (quote) pop.append(el("div", "quote", quote));
    pop.append(ta, row);
    pop.style.left = Math.max(8, Math.min(r.left, innerWidth - 316)) + "px";
    pop.style.top = Math.max(8, Math.min(r.bottom + 6, innerHeight - 200)) + "px";
    root.append(pop);
    ta.focus();
    ta.addEventListener("keydown", (ev) => {
      ev.stopPropagation();
      if (ev.key === "Enter" && !ev.shiftKey) { ev.preventDefault(); add(ev); }
      if (ev.key === "Escape") closePop();
    });
  }

  async function post(path, fields) {
    const r = await F(CFG.endpoint + path, {
      method: "POST", headers: { "content-type": "text/plain" },
      body: JSON.stringify(Object.assign({ secret: CFG.secret }, fields)),
    });
    const j = await r.json().catch(() => ({}));
    if (!r.ok) throw new Error(j.error || r.status);
    return j;
  }

  // A note on `target`, or on the page itself when target is null.
  async function addNote(target, selector, quote, text) {
    try {
      const j = await post("/note", {
        url: location.href, selector, text, quote: quote || "",
        tag: target ? target.tagName.toLowerCase() : "",
        snippet: target ? (target.innerText || target.value || "").trim().slice(0, 120) : "",
      });
      say(`Note ${j.id} added as a draft; Send delivers all drafts`);
      await refresh();
      return true;
    } catch (e) {
      say("Could not reach the review inbox: " + e.message);
      return false;
    }
  }

  async function sendDrafts() {
    try {
      const j = await post("/send", {});
      say(j.queued ? `Sent ${j.queued} note(s) to the agent` : "No drafts to send");
      await refresh();
    } catch (e) {
      say("Could not send: " + e.message);
    }
  }

  async function editNote(id, fields) {
    try {
      await post("/edit", Object.assign({ id }, fields));
    } catch (e) {
      say(`Could not change note ${id}: ` + e.message);
    }
    editing = null;
    await refresh(true);
  }

  async function refresh(force) {
    mount();
    try {
      const r = await F(CFG.endpoint + "/state", {
        method: "POST", headers: { "content-type": "text/plain" },
        body: JSON.stringify({ secret: CFG.secret }),
      });
      if (!r.ok) return;
      const text = await r.text();
      if (text === lastState && !force) return;
      lastState = text;
      const state = JSON.parse(text);
      notes = state.notes || [];
      syncPatches(state.patches || []);
      renderPins();
      if (editing == null) renderList();
    } catch (_) { /* inbox gone: keep the last state */ }
  }

  function renderPins() {
    for (const p of layer.querySelectorAll(".pin")) p.remove();
    for (const n of notes.filter((n) => n.url === location.href && n.selector)) {
      const pin = el("div", "pin " + n.status, String(n.id));
      pin.style.background = COLORS[n.status] || COLORS.queued;
      pin.title = `${n.status}: ${n.text}`;
      pin.dataset.selector = n.selector;
      pin.addEventListener("click", () => showItem(n.id));
      layer.append(pin);
    }
    track();
  }

  function showItem(id) {
    setCollapsed(false);
    const item = list.querySelector(`[data-id="${id}"]`);
    if (!item) return;
    item.scrollIntoView({ block: "nearest" });
    item.classList.add("flash");
    setTimeout(() => item.classList.remove("flash"), 1200);
  }

  // Every note with its full thread; drafts can be edited or removed.
  function renderList() {
    const scrolled = list.scrollTop;
    list.replaceChildren();
    const drafts = notes.filter((n) => n.status === "draft").length;
    countEl.textContent = notes.length ? `${drafts} draft(s) · ${notes.length - drafts} sent` : "";
    sendBtn.disabled = drafts === 0;
    sendBtn.textContent = drafts ? `Send ${drafts} note(s)` : "Send";
    for (const n of notes) {
      const item = el("div", "item");
      item.dataset.id = n.id;
      item.style.borderLeftColor = COLORS[n.status] || COLORS.queued;
      const top = el("div", "top");
      const on = n.selector || "this page";
      const where = n.url === location.href ? on : `${new URL(n.url).pathname} · ${n.selector || "page"}`;
      top.append(el("strong", "", `#${n.id}`), el("span", "sel", where), el("span", "who", n.status));
      item.append(top);
      if (n.quote) item.append(el("div", "quote", n.quote));
      if (editing === n.id) {
        const ta = el("textarea"); ta.value = n.text;
        const row = el("div", "row");
        row.append(
          button("", "Cancel", () => { editing = null; renderList(); }),
          button("primary", "Save", () => editNote(n.id, { text: ta.value })),
        );
        ta.addEventListener("keydown", (ev) => ev.stopPropagation());
        item.append(ta, row);
        setTimeout(() => ta.focus());
      } else {
        item.append(el("p", "", n.text));
        if (n.status === "draft") {
          const row = el("div", "row");
          row.append(
            button("link", "Edit", () => { editing = n.id; renderList(); }),
            button("link", "Remove", () => editNote(n.id, { delete: true })),
          );
          item.append(row);
        }
      }
      for (const reply of n.replies || []) {
        const r = el("div", "reply");
        r.append(el("div", "who", "agent"), el("p", "", reply));
        item.append(r);
      }
      list.append(item);
    }
    if (!notes.length) list.append(el("div", "help", "No notes yet."));
    list.scrollTop = scrolled;
    keepInView();
  }

  // The agent's highlights: each can be hidden and shown again, or all at once.
  // Hiding is local to this page view; the agent is not told.
  function setOff(h, off) {
    h.off = off;
    h.box.classList.toggle("off", off);
  }
  function renderHighlights() {
    if (!hlSection) return;
    hlSection.replaceChildren();
    hlSection.style.display = highlights.length ? "" : "none";
    if (!highlights.length) return;
    const shown = highlights.filter((h) => !h.off).length;
    const head = el("div", "row");
    head.append(
      el("span", "who", `Agent highlights: ${shown} of ${highlights.length} shown`),
      button("", shown ? "Hide all" : "Show all", () => {
        for (const h of highlights) setOff(h, shown > 0);
        renderHighlights();
      }),
    );
    hlSection.append(head);
    for (const h of highlights) {
      const item = el("div", "hlitem" + (h.off ? " off" : ""));
      const what = el("span", "what", h.label || h.sel);
      what.title = h.sel;
      what.addEventListener("click", (ev) => {
        if (!ev.isTrusted || !h.el.isConnected) return;
        h.el.scrollIntoView({ block: "center", behavior: "smooth" });
      });
      item.append(what, button("link", h.off ? "Show" : "Hide", () => {
        setOff(h, !h.off);
        renderHighlights();
      }));
      hlSection.append(item);
    }
    keepInView();
  }

  // Keeps pins and highlights on their elements while the page scrolls or changes.
  function track() {
    if (!layer) return;
    for (const pin of layer.querySelectorAll(".pin")) {
      const t = find(pin.dataset.selector);
      if (!t) { pin.style.display = "none"; continue; }
      const r = t.getBoundingClientRect();
      pin.style.display = "";
      pin.style.left = r.left + "px";
      pin.style.top = r.top + "px";
    }
    for (const h of highlights) {
      if (!h.el.isConnected) { const again = find(h.sel); if (again) h.el = again; }
      place(h.box, h.el);
    }
  }

  function onMove(ev) {
    if (!annotate || ours(ev.target)) { if (hoverBox) hoverBox.style.display = "none"; return; }
    hoverBox.style.display = "block";
    place(hoverBox, ev.target);
  }

  // The text the owner selected on the page, with the element that holds it.
  function selectedText() {
    const s = getSelection();
    if (!s || s.isCollapsed || !s.rangeCount) return null;
    const quote = s.toString().trim();
    let node = s.getRangeAt(0).commonAncestorContainer;
    if (node.nodeType !== 1) node = node.parentElement;
    if (!quote || !node || ours(node)) return null;
    return { quote: quote.slice(0, 1000), target: node };
  }

  // While annotating, the page sees no pointer input: links, buttons and the
  // app's own handlers stay still. Press and release keep their default, so
  // text can still be selected; true marks the events whose default goes too.
  const SWALLOW = {
    pointerdown: false, mousedown: false, pointerup: false, mouseup: false, touchstart: false,
    touchend: true, dblclick: true, auxclick: true, contextmenu: true,
  };
  function swallow(ev) {
    if (!annotate || ours(ev.target)) return;
    ev.stopImmediatePropagation();
    if (SWALLOW[ev.type]) ev.preventDefault();
    if (ev.type === "mouseup" && ev.isTrusted) {
      const t = selectedText();
      if (t) { quoteAt = Date.now(); openPop(t.target, t.quote); }
    }
  }

  function onClick(ev) {
    if (ours(ev.target) || !(annotate || ev.altKey)) return;
    ev.preventDefault();
    ev.stopImmediatePropagation();
    if (!ev.isTrusted || Date.now() - quoteAt < 500) return;   // the click that ended a selection
    openPop(ev.target);
  }

  function onKey(ev) {
    if (!ev.isTrusted || ours(ev.target)) return;
    if (ev.altKey && !ev.ctrlKey && !ev.metaKey && ev.code === "KeyA") {
      ev.preventDefault();
      setAnnotate(!annotate);
    } else if (ev.key === "Escape") {
      if (pop) closePop(); else setAnnotate(false);
    }
  }

  // The agent's side, called through `agent-browser eval` by `vega review`.
  function highlight(sel, label) {
    mount();
    const t = find(sel);
    if (!t) return { found: false };
    // The same selector again replaces its highlight (new label, shown again).
    const old = highlights.findIndex((h) => h.sel === sel);
    if (old >= 0) highlights.splice(old, 1)[0].box.remove();
    const box = el("div", "hl");
    const h = { el: t, sel, label: label || "", box, off: false };
    const tag = el("div", "hl-label", label || null);
    const x = button("hl-x", "\u00d7", () => { setOff(h, true); renderHighlights(); });
    x.title = "Hide this highlight (the panel shows it again)";
    tag.append(x);
    box.append(tag);
    layer.append(box);
    highlights.push(h);
    place(box, t);
    renderHighlights();
    t.scrollIntoView({ block: "center", behavior: "smooth" });
    return { found: true, highlights: highlights.length };
  }
  function clear() {
    for (const h of highlights.splice(0)) h.box.remove();
    renderHighlights();
    return { cleared: true };
  }

  // Design patches (`vega review patch`): idempotent JS, run on every page of the
  // origin and again after DOM changes (SPA navigation, re-renders). A removed
  // or changed patch leaves its nodes; the CLI reloads the page for that.
  function syncPatches(list) {
    const names = new Set(list.map((p) => p.name));
    for (const name of [...patches.keys()]) if (!names.has(name)) patches.delete(name);
    let changed = false;
    for (const p of list) {
      const cur = patches.get(p.name);
      if (cur && cur.code === p.code) continue;
      patches.set(p.name, { code: p.code, fn: null, runs: 0, error: null });
      changed = true;
    }
    if (changed) applyPatches();
  }
  function applyPatches() {
    patchTimer = null;
    if (!document.body || !patches.size) return;
    for (const p of patches.values()) {
      try {
        if (p.fn === null) p.fn = new FunctionF(p.code);
        if (!p.fn) continue;
        p.fn.call(window);
        p.runs++;
        p.error = null;
      } catch (e) {
        if (p.fn === null) p.fn = false; // does not compile: no retry
        p.error = String((e && e.message) || e);
      }
    }
    // The patches' own changes must not trigger another run.
    if (patchObserver) patchObserver.takeRecords();
  }
  function watchPatches() {
    if (patchObserver || !MO) return;
    patchObserver = new MO(() => {
      if (!patches.size) return;
      if (patchTimer) clearTimeoutF(patchTimer);
      patchTimer = setTimeoutF(applyPatches, 150);
    });
    patchObserver.observe(document.documentElement, { childList: true, subtree: true });
  }
  function patchStatus() {
    const out = {};
    for (const [name, p] of patches) out[name] = { runs: p.runs, error: p.error };
    return out;
  }
  // Hides the overlay for an agent screenshot of the page alone.
  function hide(on) {
    mount();
    host.style.display = on ? "none" : "";
    return { hidden: !!on };
  }

  Object.defineProperty(window, "__vegaReview", {
    value: Object.freeze({
      highlight, clear, refresh: () => refresh(true), patches: patchStatus, hide,
    }),
    configurable: false, writable: false,
  });

  addEventListener("mousemove", onMove, true);
  for (const t of Object.keys(SWALLOW)) addEventListener(t, swallow, { capture: true, passive: false });
  addEventListener("click", onClick, true);
  addEventListener("keydown", onKey, true);
  addEventListener("scroll", track, true);
  addEventListener("resize", () => {
    track();
    if (panel) keepInView();
  });
  const start = () => { mount(); watchPatches(); refresh(); };
  if (document.readyState === "loading") addEventListener("DOMContentLoaded", start, { once: true });
  else start();
  setIntervalF(track, 500);
  setIntervalF(refresh, 3000);
})(__VEGA_REVIEW_CFG__);
