// Pure attribution: machine boundaries first, ancestry before directory fallback.
export const PRIVATE_HOSTS = new Set(["proteus", "ganymede", "io", "io-wsl", "titan"]);
export const privateTarget = target => PRIVATE_HOSTS.has((target ?? "").split("@").at(-1));
export const machineOf = address => address?.includes("/") ? address.split("/")[0] : null;
export function spaceOf(term, allSpaces) {
  const label = term.labels?.["herdr.space"];
  const id = label && (machineOf(term.address) ? `${machineOf(term.address)}/${label}` : label);
  return allSpaces.find(s => s.id === id);
}
// Same identity rules as sidebar projectGroups: full IDs, unique legacy names,
// UAC spaces and live ancestors. An unresolved focus never means all projects.
export function projectContext(allTerms) {
  const live = allTerms.filter(t => t.state === "live" && !Object.hasOwn(t.labels ?? {}, "vega.window") && t.labels?.["vega.role"] !== "uber");
  const short = id => id.replace(/\/$/, "").split("/").at(-1).replace(/\.git$/, "");
  const ids = new Set(live.map(t => t.labels?.["vega.project-id"]).filter(Boolean));
  for (const t of live) {
    const name = t.labels?.["vega.project"];
    if (name && ![...ids].some(id => short(id) === name)) ids.add(name);
  }
  const resolve = t => {
    if (t?.labels?.["vega.project-id"]) return t.labels["vega.project-id"];
    const name = t?.labels?.["vega.project"];
    const matches = [...ids].filter(id => id === name || short(id) === name);
    return matches.length === 1 ? matches[0] : null;
  };
  const spaceId = t => {
    const id = t?.labels?.["herdr.space"];
    return id ? (machineOf(t.address) ? `${machineOf(t.address)}/${id}` : id) : null;
  };
  const bySpace = new Map(), byAddress = new Map(live.map(t => [t.address, t]));
  for (const t of [...live.filter(t => t.labels?.["vega.role"] === "uac"), ...live]) {
    const space = spaceId(t), project = resolve(t);
    if (space && project && !bySpace.has(space)) bySpace.set(space, project);
  }
  const ofTerm = t => {
    const seen = new Set();
    while (t && !seen.has(t.address)) {
      seen.add(t.address);
      const project = bySpace.get(spaceId(t)) || resolve(t);
      if (project) return project;
      t = byAddress.get(t.parent);
    }
    return null;
  };
  for (const t of live) {
    const space = spaceId(t), project = ofTerm(t);
    if (space && project && !bySpace.has(space)) bySpace.set(space, project);
  }
  return {ofTerm, bySpace};
}
export function selectedProject(focus, allTerms) {
  const term = allTerms.find(t => t.state === "live" && t.address === (focus?.terminal ?? focus?.agent));
  if (term?.labels?.["vega.role"] === "uber") return {all: true, project: null};
  const context = projectContext(allTerms);
  return {all: false, project: context.ofTerm(term) ?? context.bySpace.get(focus?.space) ?? null};
}
export function serviceAddress(row, forward) {
  const host = row.host === "::1" ? "[::1]" : row.host;
  const address = row.url || row.address || `${row.scheme || "tcp"}://${host}:${row.port}`;
  if (!forward || !/^(https?|tcp):\/\//i.test(address)) return address;
  return address.replace(/^((?:https?|tcp):\/\/)[^/?#]+/i, (_match, scheme) => `${scheme}127.0.0.1:${forward.local}`);
}
// ps includes comm plus argv; inspect executable/package positions, not arbitrary args.
export function browserProcess(process) {
  const words = (process ?? "").trim().split(/\s+/);
  const executable = words[0]?.split("/").at(-1) ?? "";
  if (/^(?:agent-browser(?:-[a-z0-9-]+)?|chrome|chromium|chromium-browser|chrome-headless-shell|headless_shell)$/i.test(executable)) return true;
  // macOS comm/argv paths can contain spaces, including the executable name.
  if (/^(?:\S*\/)?(?:Google Chrome|Chromium)(?: for Testing)?(?: Helper[^/]*)?(?=\s|$|\/|\.app\/)/.test(process)) return true;
  return /^(?:node|bun)$/.test(executable) && words.slice(1, 3).some(word => /\/agent-browser\/dist\/daemon\.js$/.test(word));
}
export function attribute(snapshot, machine, allTerms, allSpaces, allPanes) {
  const terms = allTerms.filter(t => t.state === "live" && machineOf(t.address) === machine);
  const candidates = terms.map(term => ({term, space: spaceOf(term, allSpaces)})).filter(x => x.space);
  const pids = new Map();
  for (const c of candidates) for (const pid of [c.term.pid, c.term.foreground?.pid]) if (pid) pids.set(pid, c);
  const context = projectContext(allTerms);
  const rows = new Map();
  for (const listener of snapshot.listeners) {
    let owner = null, pid = listener.pid, browser = false;
    const project = listener.registered && listener.project;
    const browserSeen = new Set();
    while (pid > 1 && !browserSeen.has(pid)) {
      browserSeen.add(pid);
      if (browserProcess(snapshot.processes?.[pid] ?? (pid === listener.pid ? listener.name : ""))) { browser = true; break; }
      pid = snapshot.parents[pid];
    }
    if (browser) continue;
    pid = listener.pid;
    const visited = new Set();
    while (pid > 1 && !visited.has(pid)) {
      visited.add(pid);
      if (pids.has(pid)) { owner = pids.get(pid); break; }
      pid = snapshot.parents[pid];
    }
    if (!owner) {
      const cwd = listener.cwd || snapshot.cwds[listener.pid];
      const matches = candidates.flatMap(c => {
        const pane = allPanes.find(p => p.terminal === c.term.address);
        const dir = pane?.cwd || c.term.foreground?.cwd || c.space.folder;
        return dir && dir !== "/" && dir !== snapshot.home && cwd && (cwd === dir || cwd.startsWith(`${dir}/`)) ? [{...c, dir}] : [];
      }).sort((a,b) => b.dir.length - a.dir.length);
      // Equal specificity in different spaces is ambiguous: leave it unassigned.
      if (matches.length && !matches.some(c => c.dir.length === matches[0].dir.length && c.space.id !== matches[0].space.id)) owner = matches[0];
    }
    if (project && owner && context.ofTerm(owner.term) !== project) owner = null;
    if (!owner && !listener.registered) continue;
    const ownerProject = project || (owner && context.ofTerm(owner.term));
    const host = listener.host === "::1" || listener.host === "::" ? "::1" : "127.0.0.1";
    // Only loopback or wildcard listeners are reachable at the loopback destination.
    if (!["127.0.0.1", "::1", "::", "0.0.0.0", "*"].includes(listener.host)) continue;
    const key = JSON.stringify(listener.registered ? [machine, "registered", listener.port] : [machine, listener.pid, listener.port, host]);
    rows.set(key, {...listener, host, key, machine, space: owner?.space.id ?? "",
      spaceName: owner?.space.name ?? "Host registrations", global: !ownerProject && listener.registered,
      project: ownerProject || (listener.registered ? "Global services (owner unknown)" : "Other spaces")});
  }
  return [...rows.values()];
}
