// Pure attribution: machine boundaries first, ancestry before directory fallback.
export const PRIVATE_HOSTS = new Set(["proteus", "ganymede", "io", "io-wsl", "titan"]);
export const privateTarget = target => PRIVATE_HOSTS.has((target ?? "").split("@").at(-1));
export const machineOf = address => address?.includes("/") ? address.split("/")[0] : null;
export function spaceOf(term, allSpaces) {
  const label = term.labels?.["herdr.space"];
  const id = label && (machineOf(term.address) ? `${machineOf(term.address)}/${label}` : label);
  return allSpaces.find(s => s.id === id);
}
export function attribute(snapshot, machine, allTerms, allSpaces, allPanes) {
  const terms = allTerms.filter(t => t.state === "live" && machineOf(t.address) === machine);
  const candidates = terms.map(term => ({term, space: spaceOf(term, allSpaces)})).filter(x => x.space);
  const pids = new Map();
  for (const c of candidates) for (const pid of [c.term.pid, c.term.foreground?.pid]) if (pid) pids.set(pid, c);
  const project = space => candidates.filter(c => c.space.id === space.id)
    .sort((a,b) => Number(b.term.labels?.["vega.role"] === "uac") - Number(a.term.labels?.["vega.role"] === "uac"))
    .find(c => c.term.labels?.["vega.project-id"])?.term.labels["vega.project-id"];
  const rows = new Map();
  for (const listener of snapshot.listeners) {
    let owner = null, pid = listener.pid;
    const visited = new Set();
    while (pid > 1 && !visited.has(pid)) {
      visited.add(pid);
      if (pids.has(pid)) { owner = pids.get(pid); break; }
      pid = snapshot.parents[pid];
    }
    if (!owner) {
      const cwd = snapshot.cwds[listener.pid];
      const matches = candidates.flatMap(c => {
        const pane = allPanes.find(p => p.terminal === c.term.address);
        const dir = pane?.cwd || c.term.foreground?.cwd;
        return dir && dir !== "/" && dir !== snapshot.home && cwd && (cwd === dir || cwd.startsWith(`${dir}/`)) ? [{...c, dir}] : [];
      }).sort((a,b) => b.dir.length - a.dir.length);
      // Equal specificity in different spaces is ambiguous: leave it unassigned.
      if (matches.length && !matches.some(c => c.dir.length === matches[0].dir.length && c.space.id !== matches[0].space.id)) owner = matches[0];
    }
    if (!owner) continue;
    const host = listener.host === "::1" || listener.host === "::" ? "::1" : "127.0.0.1";
    // Only loopback or wildcard listeners are reachable at the loopback destination.
    if (!["127.0.0.1", "::1", "::", "0.0.0.0", "*"].includes(listener.host)) continue;
    const key = JSON.stringify([machine, listener.pid, listener.port, host]);
    rows.set(key, {...listener, host, key, machine, space: owner.space.id,
      spaceName: owner.space.name, project: project(owner.space) ?? "Other spaces"});
  }
  return [...rows.values()];
}
