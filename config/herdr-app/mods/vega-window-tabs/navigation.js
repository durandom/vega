export function tabStack(node) {
  if (node?.id === "profile:tabs") return node;
  for (const child of node?.children ?? []) {
    const found = tabStack(child);
    if (found) return found;
  }
  return null;
}

export function targetTab(stack, command) {
  const tabs = stack?.children ?? [];
  if (!tabs.length) return null;
  const current = Math.max(0, tabs.findIndex(tab => tab.id === stack.active));
  let index;
  if (command === "next") index = (current + 1) % tabs.length;
  else if (command === "prev") index = (current + tabs.length - 1) % tabs.length;
  else if (/^select-[1-9]$/.test(command)) index = Number(command.slice(7)) - 1;
  return tabs[index]?.id ?? null;
}
